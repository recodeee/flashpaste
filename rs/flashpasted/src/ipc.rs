//! Unix-socket IPC. `flashpaste-trigger` connects, sends a length-prefixed
//! JSON request, receives a length-prefixed JSON response.
//!
//! Wire format (matches `flashpaste-trigger/src/main.rs`):
//!   request:  4-byte LE u32 length || JSON bytes
//!   response: 4-byte LE u32 length || JSON bytes
//!
//! Request body:
//!   {"op":"paste","pane":"%4","ts":"..."}
//!   {"op":"stage","image_path":"..."}    // Phase 3 hook
//!
//! Response bodies:
//!   {"ok":true,"latency_ms":7}
//!   {"ok":true,"deduped":true}
//!   {"ok":false,"reason":"no-image","fallback":"bash"}

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::paste;
use crate::state::{now_unix_ms, SharedState, StagedImage, StagedSelection, StagedText};

/// Recursion guard window. A `paste` op arriving within this many ms of the
/// previous one is treated as the tmux `bind -n C-v` recursion (see fact #2
/// in the spec) and replied as `{"ok":true,"deduped":true}`.
const RECURSION_DEDUPE_MS: u64 = 1500;

/// Cap incoming request size. 16KB was enough when `Stage` only carried a
/// path; `StageText` inlines the bytes (base64-encoded by the trigger) so
/// we widen to 8MB — covers a 6MB clipboard payload comfortably. Anything
/// larger should hit the daemon via a path field, not inline.
const MAX_REQUEST_BYTES: u32 = 8 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Paste {
        pane: String,
        #[allow(dead_code)]
        ts: Option<String>,
    },
    Stage {
        image_path: String,
    },
    /// v1.19+: stage raw bytes as a text selection. Payload is base64 so
    /// the JSON envelope is safe for binary text (tmux occasionally pipes
    /// non-UTF8). `from` is informational (e.g. "tmux:%21").
    StageText {
        bytes_b64: String,
        #[allow(dead_code)]
        from: Option<String>,
    },
    Ping,
}

pub async fn spawn_listener(state: Arc<SharedState>) -> Result<JoinHandle<()>> {
    let socket_path = state.config.socket_path.clone();

    // Remove a stale socket from a previous run. We don't `unlink` files
    // that aren't sockets — that'd be how a misconfigured `--socket` flag
    // accidentally nukes real data.
    if socket_path.exists() {
        if is_socket(&socket_path) {
            let _ = std::fs::remove_file(&socket_path);
        } else {
            anyhow::bail!(
                "socket path {} exists but is not a socket — refusing to overwrite",
                socket_path.display()
            );
        }
    }
    // Ensure the parent dir is there.
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create socket parent {}", parent.display()))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("bind {}", socket_path.display()))?;
    // 0600: this is per-user; nobody else should be poking at it.
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 {}", socket_path.display()))?;

    info!(path = %socket_path.display(), "IPC listener up");

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_conn(state, stream).await {
                            // EPIPE is the normal case for the queued-paste
                            // path: we reply `{"queued":true}` synchronously,
                            // then dispatch detached. If the trigger closed
                            // before we finished writing the reply, that's
                            // its 150 ms READ_TIMEOUT — not a bug. Demote so
                            // it doesn't pollute the WARN stream.
                            let msg = e.to_string();
                            if msg.contains("Broken pipe") || msg.contains("os error 32") {
                                debug!(error = %e, "IPC connection closed by client (likely trigger read timeout)");
                            } else {
                                warn!(error = %e, "IPC connection error");
                            }
                        }
                    });
                }
                Err(e) => {
                    error!(error = %e, "IPC accept failed");
                    // Tight loop is fine; if the listener itself is broken
                    // we'd rather see a flood in logs than silently die.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    });

    Ok(handle)
}

async fn handle_conn(state: Arc<SharedState>, mut stream: UnixStream) -> Result<()> {
    let started = Instant::now();

    // Length-prefixed read.
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await.context("read len")?;
    let len = u32::from_le_bytes(len_buf);
    if len == 0 || len > MAX_REQUEST_BYTES {
        write_response(
            &mut stream,
            &json!({ "ok": false, "reason": "bad-length", "fallback": "bash" }),
        )
        .await?;
        anyhow::bail!("bad request length: {len}");
    }
    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body).await.context("read body")?;
    let req: Request = serde_json::from_slice(&body).context("parse request")?;

    let response = match req {
        Request::Paste { pane, .. } => handle_paste(&state, &pane, started).await,
        Request::Stage { image_path } => handle_stage(&state, &image_path).await,
        Request::StageText { bytes_b64, .. } => handle_stage_text(&state, &bytes_b64).await,
        Request::Ping => json!({ "ok": true, "pong": true }),
    };
    write_response(&mut stream, &response).await?;
    Ok(())
}

async fn write_response(stream: &mut UnixStream, value: &Value) -> Result<()> {
    let body = serde_json::to_vec(value)?;
    let len = u32::try_from(body.len()).context("response too large")?;
    stream.write_all(&len.to_le_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await?;
    Ok(())
}

async fn handle_paste(state: &Arc<SharedState>, pane: &str, started: Instant) -> Value {
    info!(pane, "paste: request received");

    // ─── Recursion guard ─────────────────────────────────────────────
    // Fact #2 from the spec: tmux's `bind -n C-v` re-fires when the kitty
    // `send_text \026` byte reaches tmux. The trigger binary calls us
    // again; we dedupe based on the last-paste timestamp.
    let now = now_unix_ms();
    let last = state.last_paste_ms.load(Ordering::Relaxed);
    if now.saturating_sub(last) < RECURSION_DEDUPE_MS {
        // DEBUG (was INFO until perf audit 2026-05-19): the recursive
        // C-v echo from kitty send-text generates ~10 of these per real
        // paste and was flooding journald with no diagnostic value.
        // Switched to debug! so the line stays available under
        // RUST_LOG=debug but doesn't dominate INFO traffic. Keep the
        // payload identical so existing logfile parsers still match.
        debug!(
            pane,
            delta_ms = now - last,
            "paste: dedupe — within recursion window (this is normal for the C-v echo back from kitty send-text)"
        );
        return json!({ "ok": true, "deduped": true });
    }
    // Race-tolerant: store before doing the work so a recursive call's
    // load sees the new timestamp.
    state.last_paste_ms.store(now, Ordering::Relaxed);

    // (v1.25: removed the `clipboard_holds_user_text` short-circuit
    // that was here. Rationale: tmux's `@clip` pipe auto-copies on
    // every mouse-drag-end into a tmux pane — so highlighting any
    // text in scrollback (reading a log line, selecting an error
    // message to look at) immediately overwrote the X11 CLIPBOARD
    // with that text. The next right-click → Paste then saw "X11
    // holds user text" and punted to bash, which delivered the
    // highlighted log junk instead of the user's screenshot.
    // Confirmed in journalctl on this box: `paste: X11 CLIPBOARD
    // now holds user text … punting to bash` fired on every Claude
    // pane paste once any tmux pane had been highlighted.
    //
    // New rule: if the daemon has a fresh staged image, it always
    // wins. The paste step re-claims X11 ownership with the staged
    // image bytes before dispatching, so even an xclip-held text
    // selection gets superseded. Losing the user's incidental
    // highlight on paste is much cheaper than silently delivering
    // log junk into Claude Code.
    //
    // For the "user pressed Ctrl-C in a browser and wants text"
    // case: that's still served via the bash fallback when the
    // daemon has NO fresh staged image — see the freshness branch
    // below.)

    // ─── Intent decision: text vs image ──────────────────────────────
    // User contract (2026-05-19): "if last time was text pasted and no
    // new screenshot was taken the text should be pasted to each
    // terminal". The staged-selection slot is single-valued — set_staged_image
    // replaces text and vice versa — so whatever's in the slot IS the
    // most-recent staged action. We honour that:
    //   1. Slot has fresh TEXT  → text dispatch via tmux paste-buffer
    //   2. Slot has fresh IMAGE → image dispatch via send-keys ^V
    //   3. Slot empty but X11 clipboard has text (user just copied
    //      something but it hasn't been staged in daemon yet) →
    //      read it, stage it, dispatch text
    //   4. None of the above → punt to bash (text-path fallback)
    let mut staged_selection = state.staged_snapshot().await;
    if staged_selection.is_none() || !matches!(&staged_selection, Some(s) if s.is_fresh()) {
        // No fresh staged slot — see if the user has fresh text on the
        // X11 clipboard that we can stage and use.
        if let Some(bytes) = read_clipboard_text_if_present().await {
            let staged = StagedText {
                bytes: Arc::new(bytes),
                captured_at: std::time::SystemTime::now(),
            };
            info!(
                pane,
                bytes = staged.bytes.len(),
                "paste: scraped fresh text from X11 clipboard and staged it"
            );
            state.set_staged_text(staged.clone()).await;
            staged_selection = Some(StagedSelection::Text(staged));
        }
    }

    match staged_selection {
        Some(StagedSelection::Text(text)) => {
            // ─── Text dispatch ────────────────────────────────────────
            // tmux load-buffer + paste-buffer to the target pane. No
            // clipboard claim, no kitty IPC, no unbind/rebind dance —
            // pure tmux pty injection. Works across every Claude pane
            // regardless of which terminal hosts the tmux client.
            let bytes = text.bytes.len();
            info!(pane, bytes, "paste: dispatching staged text via tmux paste-buffer");
            let state_for_task = state.clone();
            let pane_for_task = pane.to_string();
            tokio::spawn(async move {
                if let Err(e) = paste::dispatch_text_paste(
                    state_for_task,
                    pane_for_task.clone(),
                    text,
                )
                .await
                {
                    error!(error = ?e, pane = %pane_for_task, "text paste dispatch failed");
                }
            });
            return json!({
                "ok": true,
                "queued": true,
                "kind": "text",
                "bytes": bytes as u64,
                "ack_ms": started.elapsed().as_millis() as u64,
            });
        }
        Some(StagedSelection::Image(img)) if img.is_fresh() => {
            // ── External-text override ────────────────────────────────────
            // v1.25 made "image always wins when staged_image is fresh."
            // That's correct EXCEPT when the user has just copied text in
            // ANOTHER app (browser, gnome-terminal, vscode, …) — in that
            // case the daemon's staged_image is still in memory but the
            // live X11 CLIPBOARD has been taken over by the other app and
            // advertises only text/* targets. Forcing image dispatch in
            // that state means Claude's `wl-paste -t image/png` queries
            // the external owner, gets nothing, and prints "no image
            // found" while the user's just-copied text is right there.
            //
            // So: probe live X11 TARGETS. If they say "text only, no
            // image", honour the user's recent text-copy by scraping it
            // into the text slot and dispatching as text. Daemon's own
            // staged_image stays in memory for the NEXT paste in case the
            // user goes back to wanting it (taking a new screenshot will
            // overwrite the slot anyway). This costs ~3-5 ms via xclip
            // — acceptable for "text from elsewhere actually pastes."
            if clipboard_holds_user_text().await {
                if let Some(bytes) = read_clipboard_text_if_present().await {
                    let text_staged = StagedText {
                        bytes: Arc::new(bytes),
                        captured_at: std::time::SystemTime::now(),
                    };
                    let n = text_staged.bytes.len();
                    info!(
                        pane,
                        bytes = n,
                        staged_image_bytes = img.bytes.len(),
                        "paste: X11 CLIPBOARD now owned by external app with text — \
                         overriding fresh staged image and dispatching the user's text"
                    );
                    // Don't displace the in-memory staged_image (user can
                    // still re-screenshot to refresh intent), but DO set
                    // staged_text so subsequent rapid pastes find it.
                    state.set_staged_text(text_staged.clone()).await;
                    let state_for_task = state.clone();
                    let pane_for_task = pane.to_string();
                    tokio::spawn(async move {
                        if let Err(e) = paste::dispatch_text_paste(
                            state_for_task,
                            pane_for_task.clone(),
                            text_staged,
                        )
                        .await
                        {
                            error!(error = ?e, pane = %pane_for_task, "text paste dispatch failed");
                        }
                    });
                    return json!({
                        "ok": true,
                        "queued": true,
                        "kind": "text",
                        "bytes": n as u64,
                        "ack_ms": started.elapsed().as_millis() as u64,
                        "source": "x11-external-override",
                    });
                }
            }
            // No external text override → normal image dispatch.
            return dispatch_image_path(state.clone(), pane.to_string(), img, started).await;
        }
        Some(StagedSelection::Image(_)) => {
            warn!(pane, "paste: staged image too old; punting to bash");
            return json!({
                "ok": false,
                "reason": "stale-image",
                "fallback": "bash",
            });
        }
        Some(StagedSelection::Text(_)) | None => {
            info!(
                pane,
                "paste: no fresh staged content (no image, no text); punting to bash"
            );
            return json!({
                "ok": false,
                "reason": "no-content",
                "fallback": "bash",
            });
        }
    }
}

/// Image dispatch entry path, factored out so `handle_paste` can return
/// from the intent-match arm without duplicating the queue-collapse +
/// detached-spawn boilerplate below. (Kept inside this file because the
/// `paste_in_flight` / `pending_paste` state coupling stays here.)
async fn dispatch_image_path(
    state: Arc<SharedState>,
    pane: String,
    staged: StagedImage,
    started: Instant,
) -> Value {
    let pane = pane.as_str();

    // ─── In-flight guard ─────────────────────────────────────────────
    // While a dispatch is waiting on Claude to become idle, additional
    // paste presses must not spawn parallel dispatches — otherwise N
    // queued presses all fire \026 simultaneously when Claude unblocks
    // (observed in journalctl: elapsed_ms=1853, 7600, 16245, 26733 all
    // dispatching within a single second). Acquire here, after we know
    // we'll dispatch (past the punt-to-bash branches).
    //
    // Queue collapse: if already in-flight, BUMP `pending_paste` (the
    // detached task drains it at completion and replays once) instead
    // of dropping. Net effect: N user presses during one Claude
    // generation collapse to ONE extra image attach at the end, not N.
    if state.paste_in_flight.swap(true, Ordering::AcqRel) {
        let prev = state.pending_paste.fetch_add(1, Ordering::AcqRel);
        // Saturate at u8::MAX; over 200 presses without dispatch is
        // pathological and we don't want to wrap to 0.
        if prev == u8::MAX {
            state.pending_paste.store(u8::MAX, Ordering::Release);
        }
        // Remember the MOST RECENT pane that absorbed a press so the
        // replay dispatches there (not back to whichever pane started
        // the in-flight dispatch). Watcher caught the silent-loss bug:
        // press to A starts dispatch → press to B absorbed → replay
        // went to A and B's intent was dropped.
        if let Ok(mut guard) = state.pending_pane.lock() {
            *guard = Some(pane.to_string());
        }
        info!(
            pane,
            pending = prev.saturating_add(1),
            "paste: in-flight dispatch absorbed this press — will replay once at completion"
        );
        return json!({
            "ok": true,
            "queued": true,
            "reason": "in-flight-coalesced",
            "pending": prev.saturating_add(1) as u64,
        });
    }

    // ─── Dispatch (detached) ─────────────────────────────────────────
    // Reply "queued" immediately and run the dispatch on a detached task
    // that releases the in-flight flag on completion. Inline awaiting
    // was added historically because the v1.23 `wait_for_pane_idle` hold
    // could block up to 30 s and exceed the trigger's read timeout —
    // surfacing as `Broken pipe (os error 32)` when we eventually
    // replied. v1.24 dropped the wait, so dispatches now run ~10–20 ms;
    // detaching is no longer required for liveness, but we keep it so
    // the in-flight + pending_paste replay path stays unchanged.
    //
    // Replay loop: after each dispatch we drain `pending_paste`; if any
    // presses were absorbed during the wait, replay ONCE with the
    // latest staged image. The race window between "drain pending" and
    // "release in_flight" is plugged by a re-check + try-reacquire.
    let state_for_task = state.clone();
    let initial_pane = pane.to_string();
    tokio::spawn(async move {
        let mut current_staged = staged;
        let mut current_pane = initial_pane.clone();
        loop {
            let result = paste::dispatch_image_paste(
                state_for_task.clone(),
                current_pane.clone(),
                current_staged,
            )
            .await;
            if let Err(e) = result {
                error!(error = ?e, pane = %current_pane, "paste dispatch failed (detached)");
            }
            // Drain any presses that arrived during the dispatch.
            let absorbed = state_for_task
                .pending_paste
                .swap(0, Ordering::AcqRel);
            if absorbed == 0 {
                // Release the flag, then re-check pending to catch the
                // tiny race where a press arrives between our swap and
                // the release.
                state_for_task
                    .paste_in_flight
                    .store(false, Ordering::Release);
                let late = state_for_task.pending_paste.swap(0, Ordering::AcqRel);
                if late == 0 {
                    return;
                }
                // A late press snuck in. Try to re-acquire the flag.
                if state_for_task
                    .paste_in_flight
                    .swap(true, Ordering::AcqRel)
                {
                    // Someone else already grabbed it; they'll handle
                    // the press. Done.
                    info!(
                        pane = %current_pane,
                        absorbed = late,
                        "queue-collapse: late press handed off to next dispatcher"
                    );
                    return;
                }
                info!(
                    pane = %current_pane,
                    absorbed = late,
                    "queue-collapse: late press caught after release — replaying"
                );
            } else {
                info!(
                    pane = %current_pane,
                    absorbed,
                    "queue-collapse: replaying once for absorbed presses"
                );
            }
            // The replay must dispatch to the LATEST pane that absorbed a
            // press — not to the pane that started the original dispatch.
            // Otherwise pasting to pane A then quickly to pane B during
            // A's wait would silently drop B's intent. See watcher report
            // 2026-05-19 ("absorbed-press pane=%38 → replay pane=%41").
            let replay_pane = state_for_task
                .pending_pane
                .lock()
                .ok()
                .and_then(|mut g| g.take());
            if let Some(target) = replay_pane {
                if target != current_pane {
                    info!(
                        from_pane = %current_pane,
                        to_pane = %target,
                        "queue-collapse: replay re-targeted to most-recent pane"
                    );
                }
                current_pane = target;
            }
            // Fetch the latest staged image for the replay (might be a
            // newer screenshot the user took during the wait).
            match state_for_task.staged_image().await {
                Some(img) if img.is_fresh() => current_staged = img,
                Some(_) => {
                    warn!(
                        pane = %current_pane,
                        "queue-collapse: staged image went stale during replay — dropping"
                    );
                    state_for_task
                        .paste_in_flight
                        .store(false, Ordering::Release);
                    return;
                }
                None => {
                    warn!(
                        pane = %current_pane,
                        "queue-collapse: no staged image at replay time — dropping"
                    );
                    state_for_task
                        .paste_in_flight
                        .store(false, Ordering::Release);
                    return;
                }
            }
        }
    });
    json!({
        "ok": true,
        "queued": true,
        "ack_ms": started.elapsed().as_millis() as u64,
    })
}

async fn handle_stage_text(state: &Arc<SharedState>, bytes_b64: &str) -> Value {
    let bytes = match decode_base64(bytes_b64) {
        Ok(b) => b,
        Err(e) => {
            return json!({
                "ok": false,
                "reason": "bad-base64",
                "detail": format!("{e:#}"),
            });
        }
    };
    let n = bytes.len();
    let staged = StagedText {
        bytes: Arc::new(bytes),
        captured_at: std::time::SystemTime::now(),
    };
    state.set_staged_text(staged).await;
    debug!(bytes = n, "stage_text accepted");
    json!({ "ok": true, "staged": "text", "bytes": n })
}

/// Query the X11 CLIPBOARD selection and decide whether the user has
/// recently overlaid text on top of the daemon's staged image. When the
/// daemon owns the selection with image bytes, `xclip TARGETS` returns
/// just `image/*` — that's the "image is current" case. When the user
/// copies text in any app, the X11 selection ownership transfers and
/// the new owner advertises `text/*` (and usually no `image/*`). We use
/// xclip because:
///   * mutter blocks our Wayland reads (latched in `wayland.rs`),
///   * x11rb would need its own listener thread for ICCCM TARGETS
///     conversion, which is the kind of code we don't need to maintain
///     when xclip already does it correctly,
///   * the call is ~3–5 ms — acceptable for the small benefit of
///     "user's text paste actually pastes their text".
/// Returns true only if text targets are present AND no image targets
/// are present — i.e. the user has affirmatively taken the selection.
async fn clipboard_holds_user_text() -> bool {
    let out = match tokio::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            debug!(error = %e, "clipboard_holds_user_text: xclip TARGETS failed; assuming daemon still owns");
            return false;
        }
    };
    if !out.status.success() {
        // xclip returns non-zero when selection is empty; treat as "no
        // user text" rather than punting unnecessarily.
        return false;
    }
    let targets = String::from_utf8_lossy(&out.stdout);
    let mut has_text = false;
    let mut has_image = false;
    for line in targets.lines() {
        let t = line.trim();
        if t.starts_with("image/") {
            has_image = true;
        } else if t.starts_with("text/")
            || t == "STRING"
            || t == "UTF8_STRING"
            || t == "TEXT"
        {
            has_text = true;
        }
    }
    has_text && !has_image
}

/// Read the text bytes currently on the X11 CLIPBOARD if (and only if)
/// the clipboard advertises a text-only target set (no image targets).
/// Used by `handle_paste` to scrape user-copied text into the daemon's
/// staged-text slot so subsequent pastes can dispatch from staged state
/// without depending on transient X11 selection ownership.
///
/// Returns `None` if:
///   * xclip TARGETS fails (no X server, no selection owner, etc.)
///   * any `image/*` target is advertised (the daemon's own image win)
///   * no text target is advertised
///   * xclip text read returns empty bytes
async fn read_clipboard_text_if_present() -> Option<Vec<u8>> {
    // First pass: TARGETS. Only proceed if it's text-only.
    let targets_out = tokio::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "TARGETS", "-o"])
        .output()
        .await
        .ok()?;
    if !targets_out.status.success() {
        return None;
    }
    let targets = String::from_utf8_lossy(&targets_out.stdout);
    let mut has_text_target: Option<&str> = None;
    let mut has_image = false;
    for line in targets.lines() {
        let t = line.trim();
        if t.starts_with("image/") {
            has_image = true;
            break;
        }
        if has_text_target.is_none() {
            if t == "UTF8_STRING" {
                has_text_target = Some("UTF8_STRING");
            } else if t.starts_with("text/plain") {
                has_text_target = Some(t.split(';').next().unwrap_or("text/plain"));
            } else if t == "STRING" {
                has_text_target = Some("STRING");
            }
        }
    }
    if has_image {
        return None;
    }
    let target = has_text_target?;
    // Read the actual bytes for the chosen target.
    let read_out = tokio::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-t", target, "-o"])
        .output()
        .await
        .ok()?;
    if !read_out.status.success() || read_out.stdout.is_empty() {
        return None;
    }
    Some(read_out.stdout)
}

async fn handle_stage(state: &Arc<SharedState>, image_path: &str) -> Value {
    let path = std::path::PathBuf::from(image_path);
    let mime = match path.extension().and_then(|s| s.to_str()).map(str::to_lowercase) {
        Some(ref ext) if ext == "png" => "image/png",
        Some(ref ext) if ext == "jpg" || ext == "jpeg" => "image/jpeg",
        _ => "image/png",
    };
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) => {
            return json!({
                "ok": false,
                "reason": "read-failed",
                "detail": format!("{e:#}"),
            });
        }
    };
    let staged = StagedImage {
        bytes: Arc::new(bytes),
        mime,
        path,
        captured_at: std::time::SystemTime::now(),
    };
    state.set_staged_image(staged).await;
    json!({ "ok": true, "staged": true })
}

fn is_socket(p: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::symlink_metadata(p) {
        Ok(md) => md.file_type().is_socket(),
        Err(_) => false,
    }
}

/// Minimal standard-base64 decoder. Avoids pulling in the `base64` crate
/// for a single hot use. Accepts whitespace + ignores `=` padding length.
fn decode_base64(input: &str) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for ch in input.chars() {
        let val: u32 = match ch {
            'A'..='Z' => (ch as u32) - ('A' as u32),
            'a'..='z' => (ch as u32) - ('a' as u32) + 26,
            '0'..='9' => (ch as u32) - ('0' as u32) + 52,
            '+' => 62,
            '/' => 63,
            '=' | '\r' | '\n' | ' ' | '\t' => continue,
            _ => anyhow::bail!("invalid base64 char: {ch:?}"),
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1u32 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_basic_base64() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn ignores_whitespace_and_padding() {
        assert_eq!(decode_base64("aGVs\nbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8 ").unwrap(), b"hello");
    }
}
