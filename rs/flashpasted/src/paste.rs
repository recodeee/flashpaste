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

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;

use crate::agent::{self, AgentKind};
use crate::state::{now_unix_ms, SharedState, StagedImage, StagedText};
use crate::tmux;
use tokio::io::AsyncWriteExt;

/// Top-level entry from `ipc::handle_paste`.
///
/// `pane` is the tmux pane id (e.g. `%4`).
/// `_staged` is included so the caller can confirm the image is fresh
/// before we burn time on the IPC + tmux dance.
pub async fn dispatch_image_paste(
    state: Arc<SharedState>,
    pane: String,
    staged: StagedImage,
) -> Result<()> {
    let payload_bytes = staged.bytes.len();
    let payload_name = staged
        .path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.trim_start_matches("Screenshot from ").to_string())
        .unwrap_or_else(|| "<no-name>".to_string());

    // Re-claim X11 CLIPBOARD with the staged image bytes so Claude's
    // `wl-paste -t image/png` reads OUR image (and not whatever external
    // app last grabbed the selection). Wayland is mutter-wedged on this
    // box; the X11 owner does the real work.
    //
    // No sleep: the X11 reclaim wakes on the notifier and runs concurrently
    // with the rest of the dispatch (tmux forks, send-keys). By the time
    // Claude actually fires `wl-paste -t image/png`, the reclaim has long
    // since landed — local X11 socket round-trips are sub-millisecond,
    // while the tmux forks below add several ms of scheduling. If a race
    // ever shows up as "no image found" on the first paste after a new
    // screenshot, restore a 2 ms sleep here.
    let _ = state.stage_notifier_tx.send(now_unix_ms());

    let snap = tmux::pane_snapshot(&pane).await;
    let agent = agent::detect(&pane, &snap).await;
    if agent == AgentKind::Aider {
        let image_path = agent::deliver_aider_image(&pane, &staged)
            .await
            .context("aider image delivery")?;
        info!(
            pane,
            kind = "image",
            agent = agent.as_str(),
            payload_bytes,
            payload_name = %payload_name,
            path = %image_path.display(),
            "PASTED image via agent adapter"
        );
        return Ok(());
    }

    // Inject Ctrl-V (0x16) into the pane's pty via `tmux send-keys -l`.
    // `-l` is literal: no keytable, no unbind/rebind dance. Reaches any
    // tmux pane regardless of which terminal hosts the client.
    tmux::dispatch_ctrl_v_to_pane(&pane)
        .await
        .context("batched tmux Ctrl-V dispatch")?;

    info!(
        pane,
        kind = "image",
        agent = agent.as_str(),
        payload_bytes,
        payload_name = %payload_name,
        "PASTED image"
    );
    Ok(())
}

/// Text-paste dispatch. Pipes the staged text bytes into a tmux buffer
/// via `tmux load-buffer -` (stdin), then `tmux paste-buffer -p -t <pane>`
/// writes the buffer bytes directly into the target pane's pty. No
/// clipboard claim, no kitty IPC, no unbind/rebind dance — just two
/// `tmux` forks and Claude Code reads the text as if the user typed it.
///
/// Works for ANY tmux pane regardless of which terminal hosts the tmux
/// client (same property as `tmux send-keys -l ^V` for image paste).
/// User contract (2026-05-19): "if last time was text and no new
/// screenshot was taken, text should be pasted to each terminal" — this
/// is what makes that contract hold across multiple panes.
pub async fn dispatch_text_paste(
    _state: Arc<SharedState>,
    pane: String,
    text: StagedText,
) -> Result<()> {
    let bytes_len = text.bytes.len();

    // Load text into tmux buffer 'fp_text' via stdin.
    let mut load = tokio::process::Command::new("tmux")
        .args(["load-buffer", "-b", "fp_text", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("spawn tmux load-buffer")?;
    {
        let mut stdin = load.stdin.take().context("load-buffer stdin not piped")?;
        stdin
            .write_all(&text.bytes)
            .await
            .context("write load-buffer stdin")?;
    }
    let load_status = load.wait().await.context("load-buffer wait")?;
    if !load_status.success() {
        anyhow::bail!("tmux load-buffer non-zero: {:?}", load_status);
    }

    // Paste into the target pane (bracketed paste for multi-line safety).
    let paste_status = tokio::process::Command::new("tmux")
        .args(["paste-buffer", "-p", "-b", "fp_text", "-t", &pane])
        .status()
        .await
        .context("spawn tmux paste-buffer")?;
    if !paste_status.success() {
        anyhow::bail!("tmux paste-buffer non-zero: {:?}", paste_status);
    }

    info!(pane, kind = "text", bytes = bytes_len, "PASTED text");
    Ok(())
}
