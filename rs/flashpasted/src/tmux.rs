//! tmux command helpers — unbind/select-pane/schedule-rebind.
//!
//! Fact #2 from the spec: tmux's `bind -n C-v` recurses when `\026` reaches
//! tmux via kitty `send-text`. We MUST `tmux unbind -n C-v` BEFORE
//! send-text and rebind ~100ms later, detached so it survives the daemon's
//! current paste finishing.
//!
//! Implementation notes:
//!   * Forking tmux is ~5ms each. That's our biggest single cost on the
//!     hot path. tokio::process::Command keeps us non-blocking.
//!   * The detached rebind uses `setsid` so it survives SIGHUP if the
//!     daemon's pty closes. Bash uses `setsid -f sh -c 'sleep 0.1; tmux
//!     bind ...'`; we replicate that exactly.

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::Command;

/// `tmux select-pane -t <pane>` — same as the bash dispatcher's first
/// action. Best-effort; ignore failures.
pub async fn select_pane(pane: &str) {
    let _ = Command::new("tmux")
        .args(["select-pane", "-t", pane])
        .status()
        .await;
}

/// If `pane` is in copy-mode, exit it. Copy-mode swallows every byte
/// including the \026 we synthesize via kitty `send_text`, so paste appears
/// to do nothing — and Ctrl-C/Ctrl-V also stop working from the user's
/// perspective. Mouse-wheel scrolling auto-enters copy-mode on most tmux
/// setups, so this trap is easy to hit silently. Best-effort: a non-zero
/// exit ("not in a mode") is fine.
pub async fn cancel_copy_mode_if_active(pane: &str) {
    let out = Command::new("tmux")
        .args(["display", "-t", pane, "-p", "#{pane_mode}"])
        .output()
        .await;
    let Ok(out) = out else { return };
    let mode = String::from_utf8_lossy(&out.stdout);
    if mode.trim() == "copy-mode" {
        let _ = Command::new("tmux")
            .args(["send-keys", "-t", pane, "-X", "cancel"])
            .status()
            .await;
        tracing::info!(pane, "exited copy-mode before dispatch");
    }
}

/// True if the pane's running command looks like Claude Code AND the TUI
/// is currently generating a response. Detected by scanning the rendered
/// pane for the live token-counter line (e.g. `↓ 948 tokens` or
/// `↓ 4.6k tokens`) which only appears during generation. Used by
/// `wait_for_pane_idle` so we don't lose pastes to the TUI's input freeze.
async fn claude_is_busy(pane: &str) -> bool {
    let cmd_out = Command::new("tmux")
        .args(["display", "-t", pane, "-p", "#{pane_current_command}"])
        .output()
        .await;
    if let Ok(out) = cmd_out {
        let cmd = String::from_utf8_lossy(&out.stdout);
        let cmd = cmd.trim();
        // claude-code TUI runs under `claude` or `node` depending on build.
        // Bare shells / other apps: skip the wait entirely.
        if cmd != "claude" && cmd != "node" {
            return false;
        }
    }
    let cap = Command::new("tmux")
        .args(["capture-pane", "-t", pane, "-pS", "-10"])
        .output()
        .await;
    let Ok(cap) = cap else { return false };
    let text = String::from_utf8_lossy(&cap.stdout);
    // Match `<num>[<unit>] tokens` somewhere in the recent visible lines.
    // The live indicator is the only place this pattern appears.
    text.lines().any(line_has_token_counter)
}

fn line_has_token_counter(line: &str) -> bool {
    // Find an occurrence of " tokens" preceded (skipping whitespace and
    // one optional unit char like k/K/M) by an ASCII digit.
    let lower = line.as_bytes();
    let needle = b" tokens";
    let mut i = 0;
    while i + needle.len() <= lower.len() {
        if &lower[i..i + needle.len()] == needle {
            // Walk back, skipping optional unit suffix, looking for a digit.
            let mut j = i;
            while j > 0 && matches!(lower[j - 1], b'k' | b'K' | b'M' | b'.' | b' ') {
                j -= 1;
            }
            if j > 0 && lower[j - 1].is_ascii_digit() {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// Block until the Claude TUI is idle (or `timeout` elapses). Claude's TUI
/// does not process keyboard input — including the \026 byte we inject via
/// kitty `send_text` — while it's mid-generation. Without this wait, every
/// paste pressed during generation gets dropped silently. The user's
/// contract is "paste always works": this is how we honour it.
///
/// Polls every 200 ms; gives up after `timeout` and dispatches anyway so a
/// detector false-negative doesn't hang the daemon forever.
pub async fn wait_for_pane_idle(pane: &str, timeout: Duration) {
    let start = std::time::Instant::now();
    let mut waited = false;
    while start.elapsed() < timeout {
        if !claude_is_busy(pane).await {
            if waited {
                tracing::info!(pane, elapsed_ms = start.elapsed().as_millis() as u64,
                    "Claude TUI became idle — dispatching queued paste");
            }
            return;
        }
        if !waited {
            tracing::info!(pane, "Claude TUI busy — holding paste until idle");
            waited = true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    tracing::warn!(pane, timeout_ms = timeout.as_millis() as u64,
        "wait_for_pane_idle: timed out, dispatching anyway");
}

/// `tmux unbind -n C-v` — must run BEFORE kitty `send_text \026` or tmux's
/// root binding consumes the byte (see the bash dispatcher's v1.3 edit-log
/// entry).
pub async fn unbind_c_v() -> Result<()> {
    let status = Command::new("tmux")
        .args(["unbind", "-n", "C-v"])
        .status()
        .await
        .context("spawn tmux unbind")?;
    if !status.success() {
        // tmux is sometimes mid-restart; not fatal.
        tracing::warn!(?status, "tmux unbind -n C-v returned non-zero");
    }
    Ok(())
}

/// Schedule a `tmux bind -n C-v <command>` to fire ~`delay` from now, fully
/// detached from this process (setsid). The bash dispatcher's v1.4 edit log
/// explains why detach matters: a backgrounded subshell catches SIGHUP when
/// the parent dispatch script exits, leaving C-v unbound permanently.
///
/// We don't `await` the rebind — that's the whole point. We `tokio::spawn`
/// it and return immediately so the caller can reply on the IPC socket.
pub fn schedule_rebind(rebind_command: String, delay: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        // Run `setsid -f` to detach into a new session. The actual tmux
        // bind command is passed to a shell because the rebind command
        // contains a quoted `run-shell -b "..."` payload.
        let result = Command::new("setsid")
            .arg("-f")
            .arg("sh")
            .arg("-c")
            .arg(&rebind_command)
            .status()
            .await;
        match result {
            Ok(status) if status.success() => {
                tracing::debug!("tmux rebind fired");
            }
            Ok(status) => tracing::warn!(?status, "tmux rebind returned non-zero"),
            Err(e) => tracing::warn!(error = %e, "tmux rebind spawn failed"),
        }
    });
}
