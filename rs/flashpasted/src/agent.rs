//! Terminal-agent detection and agent-specific image delivery.
//!
//! The daemon's default image path sends a raw Ctrl-V byte into the target
//! pane. That is correct for Claude Code and Codex CLI, whose TUIs treat the
//! byte as an image-paste request and then read the clipboard. Aider's image
//! contract is different: the documented, reliable path is `/add <image-path>`
//! from inside the chat. This module keeps that difference explicit without
//! disturbing the existing Claude/Codex path.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::process::Command;
use tracing::{debug, warn};

use crate::state::StagedImage;
use crate::tmux::{self, PaneSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    Aider,
    Llm,
    Generic,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Aider => "aider",
            Self::Llm => "llm",
            Self::Generic => "generic",
        }
    }
}

/// Detect the foreground agent in `pane`.
///
/// This is best-effort and deliberately conservative: unknown or ambiguous
/// panes return Generic, which preserves the legacy Ctrl-V path.
pub async fn detect(_pane: &str, snap: &PaneSnapshot) -> AgentKind {
    if let Some(kind) = forced_agent_from_env() {
        return kind;
    }

    if let Some(kind) = classify_process_args(&snap.current_command) {
        return kind;
    }

    let Some(root_pid) = snap.pane_pid else {
        return AgentKind::Generic;
    };

    let Ok(descendants) = descendant_args(root_pid).await else {
        return AgentKind::Generic;
    };

    detect_from_process_lines(&descendants)
}

/// Deliver to Aider via its in-chat `/add <path>` command.
pub async fn deliver_aider_image(pane: &str, staged: &StagedImage) -> Result<PathBuf> {
    let image_path = materialize_readable_path(staged)
        .await
        .context("prepare readable image path for aider")?;
    let command = format!("/add {}", image_path.display());

    let snap = tmux::pane_snapshot(pane).await;
    tmux::cancel_copy_mode_if_active(pane, &snap).await;
    tmux::select_pane(pane).await;
    tmux::send_literal_then_enter(pane, &command)
        .await
        .context("send aider /add command")?;

    Ok(image_path)
}

fn forced_agent_from_env() -> Option<AgentKind> {
    let value = std::env::var("FLASHPASTE_AGENT").ok()?;
    match normalize_agent_name(&value) {
        Some(kind) => {
            debug!(agent = kind.as_str(), "using FLASHPASTE_AGENT override");
            Some(kind)
        }
        None => {
            warn!(value, "ignoring unknown FLASHPASTE_AGENT override");
            None
        }
    }
}

fn normalize_agent_name(value: &str) -> Option<AgentKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" | "claude_code" => Some(AgentKind::ClaudeCode),
        "codex" | "codex-cli" | "codex_cli" => Some(AgentKind::Codex),
        "aider" => Some(AgentKind::Aider),
        "llm" => Some(AgentKind::Llm),
        "generic" | "none" | "off" => Some(AgentKind::Generic),
        _ => None,
    }
}

fn detect_from_process_lines(lines: &[String]) -> AgentKind {
    // Aider first: `aider --model codex` must be Aider, not Codex.
    let mut saw_codex = false;
    let mut saw_claude = false;
    let mut saw_llm = false;

    for line in lines {
        match classify_process_args(line) {
            Some(AgentKind::Aider) => return AgentKind::Aider,
            Some(AgentKind::Codex) => saw_codex = true,
            Some(AgentKind::ClaudeCode) => saw_claude = true,
            Some(AgentKind::Llm) => saw_llm = true,
            Some(AgentKind::Generic) | None => {}
        }
    }

    if saw_codex {
        AgentKind::Codex
    } else if saw_claude {
        AgentKind::ClaudeCode
    } else if saw_llm {
        AgentKind::Llm
    } else {
        AgentKind::Generic
    }
}

fn classify_process_args(args: &str) -> Option<AgentKind> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let first = tokens.first().copied()?;

    if let Some(kind) = classify_token(first) {
        return Some(kind);
    }

    let first_name = basename_token(first);
    if is_python_like(first_name) {
        return classify_python_module(&tokens);
    }

    if is_wrapper(first_name) {
        return tokens
            .iter()
            .skip(1)
            .find_map(|token| classify_token(token));
    }

    // Node and shell wrappers frequently expose the actual executable as a
    // path argument, e.g. `/usr/bin/node /usr/local/bin/claude`.
    if matches!(first_name, "node" | "bash" | "sh" | "zsh" | "fish") {
        return tokens
            .iter()
            .skip(1)
            .find_map(|token| classify_token(token));
    }

    None
}

fn classify_python_module(tokens: &[&str]) -> Option<AgentKind> {
    for pair in tokens.windows(2) {
        if pair[0] == "-m" {
            if let Some(kind) = classify_token(pair[1]) {
                return Some(kind);
            }
            if is_wrapper(basename_token(pair[1])) {
                return tokens
                    .iter()
                    .skip(2)
                    .find_map(|token| classify_token(token));
            }
            return None;
        }
    }
    None
}

fn classify_token(token: &str) -> Option<AgentKind> {
    match basename_token(token) {
        "claude" | "claude-code" => Some(AgentKind::ClaudeCode),
        "codex" => Some(AgentKind::Codex),
        "aider" => Some(AgentKind::Aider),
        "llm" => Some(AgentKind::Llm),
        _ => None,
    }
}

fn basename_token(token: &str) -> &str {
    let trimmed = token.trim_matches(|c| c == '"' || c == '\'');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

fn is_python_like(name: &str) -> bool {
    name == "python" || name == "python3" || name.starts_with("python3.")
}

fn is_wrapper(name: &str) -> bool {
    matches!(
        name,
        "env" | "npx" | "pnpm" | "yarn" | "bunx" | "uvx" | "pipx"
    )
}

async fn descendant_args(root_pid: u32) -> Result<Vec<String>> {
    let out = Command::new("ps")
        .args(["-e", "-o", "pid=,ppid=,args="])
        .output()
        .await
        .context("spawn ps for pane process tree")?;
    if !out.status.success() {
        anyhow::bail!("ps returned non-zero");
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(descendant_args_from_ps(root_pid, &stdout))
}

fn descendant_args_from_ps(root_pid: u32, ps_output: &str) -> Vec<String> {
    #[derive(Clone)]
    struct ProcLine {
        pid: u32,
        ppid: u32,
        args: String,
    }

    let procs: Vec<ProcLine> = ps_output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            let args = parts.collect::<Vec<_>>().join(" ");
            Some(ProcLine { pid, ppid, args })
        })
        .collect();

    let mut seen = std::collections::HashSet::from([root_pid]);
    let mut changed = true;
    while changed {
        changed = false;
        for proc_line in &procs {
            if seen.contains(&proc_line.ppid) && seen.insert(proc_line.pid) {
                changed = true;
            }
        }
    }

    procs
        .into_iter()
        .filter(|proc_line| proc_line.pid != root_pid && seen.contains(&proc_line.pid))
        .map(|proc_line| proc_line.args)
        .collect()
}

async fn materialize_readable_path(staged: &StagedImage) -> Result<PathBuf> {
    let dir = agent_stage_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create {}", dir.display()))?;

    let ext = match staged.mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => "png",
    };
    let path = dir.join(format!("flashpaste-agent-latest.{ext}"));
    tokio::fs::write(&path, staged.bytes.as_slice())
        .await
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn agent_stage_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir).join("flashpaste-agent");
    }
    std::env::temp_dir().join("flashpaste-agent")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_leaf_commands() {
        assert_eq!(classify_process_args("aider"), Some(AgentKind::Aider));
        assert_eq!(classify_process_args("codex"), Some(AgentKind::Codex));
        assert_eq!(
            classify_process_args("claude-code"),
            Some(AgentKind::ClaudeCode)
        );
    }

    #[test]
    fn detects_wrapped_commands() {
        assert_eq!(
            classify_process_args("python3 -m aider --model codex"),
            Some(AgentKind::Aider)
        );
        assert_eq!(
            classify_process_args("python3 -m pipx run aider"),
            Some(AgentKind::Aider)
        );
        assert_eq!(
            classify_process_args("/usr/bin/node /usr/local/bin/claude --resume"),
            Some(AgentKind::ClaudeCode)
        );
        assert_eq!(classify_process_args("npx codex"), Some(AgentKind::Codex));
    }

    #[test]
    fn avoids_substring_matches() {
        assert_eq!(classify_process_args("myclaude-wrapper"), None);
        assert_eq!(classify_process_args("codexgen-helper"), None);
        assert_eq!(classify_process_args("/tmp/llm-cache/foo"), None);
    }

    #[test]
    fn prefers_aider_over_model_argument() {
        let lines = vec!["python3 -m aider --model codex".to_string()];
        assert_eq!(detect_from_process_lines(&lines), AgentKind::Aider);
    }

    #[test]
    fn extracts_descendants_from_unordered_ps_output() {
        let ps = "\
          30 20 /usr/bin/aider --model sonnet\n\
          10 1 bash\n\
          20 10 python3 -m pipx run aider\n";
        assert_eq!(
            descendant_args_from_ps(10, ps),
            vec![
                "/usr/bin/aider --model sonnet".to_string(),
                "python3 -m pipx run aider".to_string()
            ]
        );
    }
}
