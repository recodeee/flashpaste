//! End-to-end paste dispatch — the hot path.
//!
//! Sequence (mirrors the bash dispatcher's FAST PATH block, modulo:
//! clipboard ownership is already done by the daemon, so we can skip every
//! probe/prestage step):
//!
//!   1. `tmux select-pane -t <pane>`
//!   2. `tmux unbind -n C-v`   (fact #2 — must happen BEFORE send-text)
//!   3. kitty IPC: `send_text` with payload `\x16`  (fact #1)
//!   4. schedule detached `tmux bind -n C-v ...` after 100ms (fact #2)
//!
//! Steps 2 and 3 must run in that order. We `select-pane` first because the
//! caller's tmux paste menu may be on a different pane than the focused
//! one; without selecting we'd send-text into the wrong pane.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::kitty;
use crate::state::{SharedState, StagedImage};
use crate::tmux;

/// How long to wait before re-binding `C-v` in tmux. The bash dispatcher
/// settled on 100ms after observing that anything shorter races the
/// in-flight `\026` byte (tmux still processing it when the rebind lands)
/// and anything longer is visible to the user as "C-v doesn't paste right
/// after a paste".
const TMUX_REBIND_DELAY: Duration = Duration::from_millis(100);

/// Top-level entry from `ipc::handle_paste`.
///
/// `pane` is the tmux pane id (e.g. `%4`).
/// `_staged` is included so the caller can confirm the image is fresh
/// before we burn time on the IPC + tmux dance.
pub async fn dispatch_image_paste(
    state: Arc<SharedState>,
    pane: String,
    _staged: StagedImage,
) -> Result<()> {
    // Resolve the kitty IPC socket. The daemon could cache this at startup,
    // but kitty sometimes restarts (e.g. user reopens the terminal) and the
    // socket name embeds the kitty pid; resolving on each paste is cheap
    // (one readdir) and avoids stale paths.
    let xdg = xdg_runtime_dir();
    let Some(kitty_sock) = kitty::find_kitty_socket(&xdg) else {
        anyhow::bail!(
            "no kitty IPC socket in {} (is kitty running with --listen?)",
            xdg.display()
        );
    };
    debug!(kitty_sock = %kitty_sock.display(), "resolved kitty socket");

    // Step 1: select pane. Best-effort.
    tmux::select_pane(&pane).await;

    // Step 2: unbind -n C-v. Must happen synchronously before the byte
    // reaches tmux.
    tmux::unbind_c_v().await.context("tmux unbind -n C-v")?;

    // Step 3: kitty `send_text` with Ctrl-V byte.
    if let Err(e) = kitty::send_ctrl_v(&kitty_sock, state.kitty_version).await {
        // If kitty IPC fails, the user is wedged. The schedule_rebind below
        // still needs to fire so we don't leave tmux without a C-v binding.
        warn!(error = %e, "kitty send_text failed — restoring tmux binding immediately");
        tmux::schedule_rebind(state.config.tmux_rebind_command.clone(), Duration::from_millis(10));
        return Err(e.context("kitty send_text"));
    }

    // Step 4: schedule the detached rebind. Returns immediately.
    tmux::schedule_rebind(state.config.tmux_rebind_command.clone(), TMUX_REBIND_DELAY);

    info!(pane, "dispatched image paste");
    Ok(())
}

fn xdg_runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let uid = nix::unistd::Uid::current().as_raw();
    let candidate = PathBuf::from(format!("/run/user/{uid}"));
    if candidate.is_dir() {
        return candidate;
    }
    PathBuf::from("/tmp")
}
