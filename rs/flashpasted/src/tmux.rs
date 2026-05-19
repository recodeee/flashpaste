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
