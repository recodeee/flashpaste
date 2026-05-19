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
use crate::state::{now_unix_ms, SharedState, StagedImage, StagedText};
use crate::tmux;
use tokio::io::AsyncWriteExt;

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
    staged: StagedImage,
) -> Result<()> {
    // Pull out the "what are we actually pasting" payload-summary fields
    // up front so every log line on this code path can include them. The
    // user complained that the dispatched-paste log line was too abstract
    // ("dispatched image paste pane=%X") and didn't say WHAT was being
    // pasted — which mattered when an unexpected screenshot from minutes
    // ago kept getting pasted because the daemon's slot was stale.
    let payload_bytes = staged.bytes.len();
    let payload_mime  = staged.mime;
    let payload_path  = staged.path.display().to_string();
    // Strip the directory and the "Screenshot from " prefix that GNOME
    // always emits so the log line stays scannable.
    let payload_name = staged.path.file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.trim_start_matches("Screenshot from ").to_string())
        .unwrap_or_else(|| "<no-name>".to_string());
    // Resolve the kitty IPC socket. The daemon could cache this at startup,
    // but kitty sometimes restarts (e.g. user reopens the terminal) and the
    // socket name embeds the kitty pid; resolving on each paste is cheap
    // (one readdir) and avoids stale paths.
    // Per-phase timing — emitted as a single summary line at the end so the
    // user can see where dispatch latency is going. Set RUST_LOG=debug for
    // intermediate step logs in addition to the summary.
    let t_start = std::time::Instant::now();
    let mut t_phase = t_start;
    let mut take_phase = || -> u64 {
        let now = std::time::Instant::now();
        let ms = now.duration_since(t_phase).as_millis() as u64;
        t_phase = now;
        ms
    };

    let xdg = xdg_runtime_dir();
    let Some(kitty_sock) = kitty::find_kitty_socket(&xdg) else {
        anyhow::bail!(
            "no kitty IPC socket in {} (is kitty running with --listen?)",
            xdg.display()
        );
    };
    let ms_socket = take_phase();
    debug!(kitty_sock = %kitty_sock.display(), ms_socket, "resolved kitty socket");

    // Step 0: re-assert clipboard ownership.
    //
    // Why: between two pastes, the user can have copied text (the v1.19
    // OSC 52 path makes kitty the live Wayland selection owner with
    // text/plain bytes). The daemon's `latest_image` is still cached in
    // memory, but the *live* clipboard owner has changed. When we
    // send-text \026 and Claude calls `wl-paste -t image/png`, kitty
    // serves the (text) selection — Claude reads 0 image bytes and
    // silently does nothing. Symptom: "right-click → Paste doesn't
    // paste the image; Ctrl+V right after a screenshot does."
    //
    // Bumping the stage notifier wakes the wayland.rs + x11.rs owner
    // tasks, which re-claim the selection with the staged image bytes.
    // The brief sleep lets the round-trip land before we send-text.
    // On mutter where the Wayland claim is rejected outright (no
    // ext-data-control / wlr-data-control), the X11 re-claim still
    // succeeds and the wl-paste shim's xclip fallback picks it up.
    // Identity of the staged image we're about to dispatch — the
    // `captured_at` SystemTime converted to ms since epoch. If this
    // matches `state.last_claim_request_image_ms`, the Wayland + X11
    // owners already claimed THIS image and there's no need to re-fire
    // the notifier (no SetSelectionOwner storm, no 8 ms sleep wasted).
    let staged_id_ms = staged_image_id_ms(&staged);
    let prev_claim_id = state
        .last_claim_request_image_ms
        .load(std::sync::atomic::Ordering::Acquire);
    let ms_reassert = if prev_claim_id == staged_id_ms && staged_id_ms != 0 {
        // Skip the re-assert entirely — the staged image hasn't
        // changed since we last asked the owners to claim it.
        info!(
            pane,
            image_id_ms = staged_id_ms,
            "paste: skipping re-assert (staged image unchanged since last claim)"
        );
        take_phase()
    } else {
        info!(
            pane,
            image_id_ms = staged_id_ms,
            "paste: re-asserting clipboard ownership before dispatch"
        );
        let _ = state.stage_notifier_tx.send(now_unix_ms());
        state
            .last_claim_request_image_ms
            .store(staged_id_ms, std::sync::atomic::Ordering::Release);
        // X11 selection claim over the local socket is microseconds — 40 ms
        // was conservative padding "in case". 8 ms is a single ~16 ms frame
        // worth of slack which still survives any plausible scheduler hiccup
        // and shaves the bulk of Tier-3 dispatch latency.
        tokio::time::sleep(Duration::from_millis(8)).await;
        take_phase()
    };

    // Step 1: select pane. Best-effort.
    tmux::select_pane(&pane).await;
    let ms_select = take_phase();

    // Step 1.4: snapshot pane state (mode + current_command) in ONE
    // `tmux display` fork. Before this, the copy-mode check and the
    // pane-idle check each forked their own `tmux display` call — two
    // forks × ~5 ms = ~10 ms wasted per dispatch.
    let pane_snap = tmux::pane_snapshot(&pane).await;
    let ms_snapshot = take_phase();

    // Step 1.5: if the user wheel-scrolled the pane into copy-mode, the
    // \026 byte we're about to send would be swallowed by copy-mode's key
    // handler and silently lost. Cancel it first.
    tmux::cancel_copy_mode_if_active(&pane, &pane_snap).await;
    let ms_copymode = take_phase();

    // (v1.23 had a `wait_for_pane_idle` step here that polled
    // `capture-pane` for the Claude TUI's `↓ N tokens` indicator and held
    // the dispatch until generation ended. In practice the detector hit
    // any scrollback line containing "<digit> tokens" — release notes,
    // chat history, "Saved 200 tokens", etc. — so it timed out on every
    // press into a Claude pane and added the full timeout (5 s default)
    // as pure latency before dispatching anyway. Removed in v1.24; the
    // honest contract is "paste fires immediately; retry if the TUI
    // drops the byte." That cost is far below 5 s of guaranteed hang.)

    // Step 2: inject the Ctrl-V byte directly into the pane's pty via
    // `tmux send-keys -l`. Before v1.26 this was `kitty @ send-text` +
    // unbind/rebind theatrics; that approach only reached panes whose
    // tmux client was attached to kitty (session 18 on this box),
    // silently dropping every paste to other Claude Code panes on
    // gnome-terminal etc. `tmux send-keys -l` writes the literal byte
    // straight to the pane's pty regardless of attached client, and
    // because `-l` bypasses the keytable we no longer need the
    // unbind-C-v / schedule-rebind dance either.
    if let Err(e) = tmux::send_ctrl_v_to_pane(&pane).await {
        let ms_kitty = take_phase();
        let ms_total = t_start.elapsed().as_millis() as u64;
        warn!(
            error = %e, pane,
            ms_socket, ms_reassert, ms_select, ms_snapshot, ms_copymode,
            ms_kitty, ms_total,
            "tmux send-keys -l ^V failed — paste byte did not reach pane"
        );
        return Err(e.context("tmux send-keys -l ^V"));
    }
    let ms_kitty = take_phase();
    let ms_total = t_start.elapsed().as_millis() as u64;

    info!(
        pane,
        kind = "image",
        payload_bytes,
        payload_mime,
        payload_name = %payload_name,
        payload_path = %payload_path,
        ms_socket, ms_reassert, ms_select, ms_snapshot, ms_copymode,
        ms_kitty, ms_total,
        "PASTED image"
    );
    Ok(())
}

/// Stable per-screenshot identity used to detect "same image as last
/// claim" in the notifier-skip path. Falls back to 0 if the SystemTime
/// is before the Unix epoch (impossible in practice but the type forces
/// us to handle it).
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
    let t_start = std::time::Instant::now();
    let bytes_len = text.bytes.len();

    // 1. load-buffer -b fp_text - (stdin)
    let mut load = tokio::process::Command::new("tmux")
        .args(["load-buffer", "-b", "fp_text", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("spawn tmux load-buffer")?;
    {
        let mut stdin = load
            .stdin
            .take()
            .context("tmux load-buffer stdin not piped")?;
        stdin
            .write_all(&text.bytes)
            .await
            .context("write text to tmux load-buffer stdin")?;
        // Drop closes stdin.
    }
    let load_status = load.wait().await.context("tmux load-buffer wait")?;
    if !load_status.success() {
        anyhow::bail!(
            "tmux load-buffer returned non-zero: {:?} (bytes={})",
            load_status,
            bytes_len
        );
    }
    let ms_load = t_start.elapsed().as_millis() as u64;

    // 2. paste-buffer -p -t <pane> -b fp_text
    //    -p: bracketed paste (Claude Code's TUI prefers this).
    //    Buffer is consumed but not deleted (default behaviour), which
    //    is fine — next dispatch overwrites it.
    let t_paste = std::time::Instant::now();
    let paste_status = tokio::process::Command::new("tmux")
        .args(["paste-buffer", "-p", "-b", "fp_text", "-t", &pane])
        .status()
        .await
        .context("spawn tmux paste-buffer")?;
    let ms_paste = t_paste.elapsed().as_millis() as u64;
    let ms_total = t_start.elapsed().as_millis() as u64;

    if !paste_status.success() {
        warn!(
            pane,
            bytes = bytes_len,
            ms_load,
            ms_paste,
            ms_total,
            "tmux paste-buffer returned non-zero — text may not have reached the pane"
        );
        anyhow::bail!("tmux paste-buffer returned non-zero: {:?}", paste_status);
    }

    info!(
        pane,
        bytes = bytes_len,
        ms_load,
        ms_paste,
        ms_total,
        "dispatched text paste"
    );
    Ok(())
}

fn staged_image_id_ms(img: &StagedImage) -> u64 {
    img.captured_at
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
