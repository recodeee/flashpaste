#![allow(dead_code)]

//! tmux command helpers — select-pane, copy-mode handling, and paste dispatch.
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

/// Minimum time between expensive paste-time fallbacks. Kept here with the
/// tmux helpers because the paste hot path is dominated by tmux subprocesses.
pub const HOT_PATH_PROBE_THROTTLE_MS: u64 = 250;

/// `tmux select-pane -t <pane>` — same as the bash dispatcher's first
/// action. Best-effort; ignore failures.
pub async fn select_pane(pane: &str) {
    let _ = Command::new("tmux")
        .args(["select-pane", "-t", pane])
        .status()
        .await;
}

/// Snapshot of the pane attributes the paste path needs before dispatching.
/// One `tmux display` fork per dispatch instead of two (was: separate calls
/// for `#{pane_mode}` and `#{pane_current_command}`).
#[derive(Debug, Clone, Default)]
pub struct PaneSnapshot {
    pub pane_pid: Option<u32>,
    pub mode: String,
    pub current_command: String,
}

impl PaneSnapshot {
    pub fn is_copy_mode(&self) -> bool {
        self.mode == "copy-mode"
    }
}

/// One-shot tmux call that grabs `pane_pid`, `pane_mode`, and
/// `pane_current_command`. The `|` separator is safe — neither mode nor
/// current_command can contain a literal `|` in practice (mode is one of a
/// fixed enum, current_command is a basename).
pub async fn pane_snapshot(pane: &str) -> PaneSnapshot {
    let out = Command::new("tmux")
        .args([
            "display",
            "-t",
            pane,
            "-p",
            "#{pane_pid}|#{pane_mode}|#{pane_current_command}",
        ])
        .output()
        .await;
    let Ok(out) = out else {
        return PaneSnapshot::default();
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    let mut parts = s.splitn(3, '|');
    let pane_pid = parts.next().and_then(|p| p.trim().parse().ok());
    let mode = parts.next().unwrap_or("").trim().to_string();
    let cmd = parts.next().unwrap_or(s).trim().to_string();
    PaneSnapshot {
        pane_pid,
        mode,
        current_command: cmd,
    }
}

/// If the snapshot says copy-mode, exit it. Copy-mode swallows every byte
/// including the \026 we synthesize via kitty `send_text`, so paste appears
/// to do nothing — and Ctrl-C/Ctrl-V also stop working from the user's
/// perspective. Mouse-wheel scrolling auto-enters copy-mode on most tmux
/// setups, so this trap is easy to hit silently.
pub async fn cancel_copy_mode_if_active(pane: &str, snap: &PaneSnapshot) {
    if snap.is_copy_mode() {
        let status = Command::new("tmux")
            .args(["send-keys", "-t", pane, "-X", "cancel"])
            .status()
            .await;
        match status {
            Ok(s) if s.success() => {
                tracing::warn!(
                    pane,
                    "copy-mode was active — cancelled before paste \
                     (mouse wheel auto-enters copy-mode and swallows paste bytes)"
                );
            }
            Ok(s) => {
                tracing::warn!(
                    pane,
                    exit = ?s,
                    "tried to cancel copy-mode but tmux returned non-zero"
                );
            }
            Err(e) => {
                tracing::error!(
                    pane,
                    error = %e,
                    "failed to exec `tmux send-keys -X cancel` — paste byte will be eaten"
                );
            }
        }
    }
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

/// Inject the Ctrl-V byte (0x16) directly into `pane`'s pty via
/// `tmux send-keys -t <pane> -l <byte>`. The `-l` (literal) flag tells
/// tmux to write the bytes as raw input rather than interpreting them
/// via the keytable — so we DON'T need the unbind/rebind dance around
/// `bind -n C-v`, and the byte reaches the pane regardless of which
/// terminal emulator hosts the tmux client. This is the critical
/// difference from `kitty @ send-text` which only reaches kitty's own
/// client; on this box that's session 18 only, leaving every other
/// Claude pane silently empty. (User report 2026-05-19: "I could paste
/// only to one Claude Code chat — the rest doesn't get my img.")
pub async fn send_ctrl_v_to_pane(pane: &str) -> Result<()> {
    // 0x16 is Ctrl-V (SYN). We send the literal byte; Claude Code's TUI
    // reads it from stdin and fires its `wl-paste -t image/png` handler.
    let status = Command::new("tmux")
        .args(["send-keys", "-t", pane, "-l", "\x16"])
        .status()
        .await
        .context("spawn tmux send-keys -l ^V")?;
    if !status.success() {
        anyhow::bail!("tmux send-keys -l ^V returned non-zero: {:?}", status);
    }
    Ok(())
}

/// Send literal text to a pane and press Enter.
pub async fn send_literal_then_enter(pane: &str, text: &str) -> Result<()> {
    let status = Command::new("tmux")
        .args([
            "send-keys",
            "-t",
            pane,
            "-l",
            text,
            ";",
            "send-keys",
            "-t",
            pane,
            "Enter",
        ])
        .status()
        .await
        .context("spawn tmux send literal then Enter")?;
    if !status.success() {
        anyhow::bail!("tmux send literal then Enter returned non-zero: {status:?}");
    }
    Ok(())
}

/// Select the pane, leave copy-mode if needed, and write Ctrl-V to the pane
/// in a single tmux invocation.
///
/// The previous daemon path forked tmux for `select-pane`, forked again to
/// inspect pane mode, and forked a third time to send the byte. This command
/// chain lets tmux evaluate `#{pane_in_mode}` internally and keeps the common
/// image paste path to one tmux subprocess.
pub async fn dispatch_ctrl_v_to_pane(pane: &str) -> Result<()> {
    let cancel = format!("send-keys -t {pane} -X cancel");
    let status = Command::new("tmux")
        .args([
            "if-shell",
            "-F",
            "-t",
            pane,
            "#{pane_in_mode}",
            &cancel,
            ";",
            "select-pane",
            "-t",
            pane,
            ";",
            "send-keys",
            "-t",
            pane,
            "-l",
            "\x16",
        ])
        .status()
        .await
        .context("spawn batched tmux Ctrl-V dispatch")?;
    if !status.success() {
        anyhow::bail!("batched tmux Ctrl-V dispatch returned non-zero: {status:?}");
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
