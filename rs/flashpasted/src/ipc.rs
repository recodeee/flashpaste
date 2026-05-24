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

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::paste;
use crate::state::{now_unix_ms, SharedState, StagedImage, StagedSelection, StagedText};

/// Cap incoming request size. 16KB was enough when `Stage` only carried a
/// path; `StageText` inlines the bytes (base64-encoded by the trigger) so
/// we widen to 8MB — covers a 6MB clipboard payload comfortably. Anything
/// larger should hit the daemon via a path field, not inline.
const MAX_REQUEST_BYTES: u32 = 8 * 1024 * 1024;
const PASTE_DEDUP_WINDOW_MS: u64 = 350;

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

    // ─── Eager fresh-screenshot pickup ────────────────────────────────
    // GNOME's screenshot tool writes the PNG via a tempfile, atomically
    // renames it into place, but then *keeps the file descriptor open*
    // for ~3–5 seconds while it renders its in-shell "Screenshot saved"
    // notification. Inotify's `CLOSE_WRITE` doesn't fire until that fd
    // closes — so a paste right after PrtScr sees the old staged_text
    // and dispatches that instead of the brand-new screenshot.
    //
    // Defence in depth: on every paste, stat the screenshots dir for the
    // freshest PNG/JPG. If its mtime is newer than what the daemon has
    // staged, read it now and stage it. The dir scan is ~1ms on a
    // typical screenshots folder; the file read is ~5-20ms for a 500KB
    // PNG. Cost is acceptable; correctness is critical.
    if should_scan_screenshots(state) {
        if let Some((fresh_path, fresh_mtime)) = state
            .config
            .screenshots_dir
            .as_ref()
            .and_then(|dir| newest_image_in(dir))
        {
            let need_pickup = match state.staged_snapshot().await {
                Some(StagedSelection::Image(img)) => fresh_mtime > img.captured_at,
                Some(StagedSelection::Text(txt)) => fresh_mtime > txt.captured_at,
                _ => fresh_mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() > now_unix_secs().saturating_sub(60))
                    .unwrap_or(false),
            };
            if need_pickup {
                if let Some(text) = read_wayland_text_if_present().await {
                    let s = StagedText {
                        bytes: Arc::new(text),
                        captured_at: std::time::SystemTime::now(),
                    };
                    info!(
                        pane,
                        path = %fresh_path.display(),
                        bytes = s.bytes.len(),
                        "paste: fresh Wayland text suppressed eager screenshot pickup"
                    );
                    state.set_staged_text(s).await;
                } else if let Ok(bytes) = std::fs::read(&fresh_path) {
                    let len = bytes.len();
                    let new_img = StagedImage {
                        bytes: Arc::new(bytes),
                        mime: mime_for_path(&fresh_path),
                        path: fresh_path.clone(),
                        captured_at: fresh_mtime,
                    };
                    info!(
                        pane,
                        path = %fresh_path.display(),
                        bytes = len,
                        "paste: eagerly picked up fresh screenshot (inotify hadn't fired yet)"
                    );
                    state.set_staged_image(new_img).await;
                }
            }
        }
    }

    // ─── Intent: text or image (most-recent staged wins) ──────────────
    // The staged-selection slot is single-valued (set_staged_image
    // replaces text and vice versa), so whatever's in the slot is the
    // most-recent staged action. If the slot is empty / stale, fall back
    // to scraping the live X11 clipboard for fresh text. Otherwise punt
    // to bash. Dispatch itself still has a short duplicate-trigger guard
    // so one physical paste gesture cannot insert the same payload twice.
    let mut staged = state.staged_snapshot().await;
    if !matches!(&staged, Some(s) if s.is_fresh()) {
        let external_text = if should_probe_external_text(state) {
            read_clipboard_text_if_present().await
        } else {
            None
        };
        if let Some(bytes) = external_text {
            let s = StagedText {
                bytes: Arc::new(bytes),
                captured_at: std::time::SystemTime::now(),
            };
            info!(
                pane,
                bytes = s.bytes.len(),
                "paste: scraped X11 text → staged"
            );
            state.set_staged_text(s.clone()).await;
            staged = Some(StagedSelection::Text(s));
        }
    } else if let Some(StagedSelection::Image(img)) = &staged {
        // External-text override: if some other app now owns the live X11
        // CLIPBOARD with text-only targets, the user has copied text since
        // the daemon staged its image. Honour the user's intent.
        //
        // BUT skip the probe entirely if the image is *very fresh* (just
        // staged via inotify in the last 3 seconds). On Mutter the daemon's
        // X11 re-claim is asynchronous — wakes up `x11.rs` via the stage
        // notifier and re-issues SetSelectionOwner — and can lag behind
        // the staging event by 100-300ms in the average case (more for
        // 4K screenshots that the compressor was working on). Without
        // this gate the probe sees the *old* text targets (because the
        // re-claim hasn't propagated yet), returns true, and overrides
        // the brand-new screenshot back to stale text. Symptom: "I took
        // a screenshot and pasted right after, but it pasted the URL I
        // had on the clipboard before."
        //
        // Heuristic: any explicit user text-copy after the screenshot
        // is going to take at least ~1 second of human action (move
        // mouse, drag-select, release). 3 s is generous; longer than
        // any X11 re-claim race but shorter than any deliberate workflow.
        let age = img
            .captured_at
            .elapsed()
            .unwrap_or(std::time::Duration::ZERO);
        let wayland_text = read_wayland_text_if_present().await;
        if let Some(bytes) = wayland_text {
            let s = StagedText {
                bytes: Arc::new(bytes),
                captured_at: std::time::SystemTime::now(),
            };
            info!(
                pane,
                bytes = s.bytes.len(),
                image_age_ms = age.as_millis() as u64,
                "paste: live Wayland text overrides staged image"
            );
            state.set_staged_text(s.clone()).await;
            staged = Some(StagedSelection::Text(s));
        } else if age > std::time::Duration::from_secs(3) && should_probe_external_text(state) {
            if let Some(bytes) = read_clipboard_text_if_present().await {
                let s = StagedText {
                    bytes: Arc::new(bytes),
                    captured_at: std::time::SystemTime::now(),
                };
                info!(
                    pane,
                    bytes = s.bytes.len(),
                    image_age_s = age.as_secs(),
                    "paste: external X11 text overrides staged image (image old enough that user text-copy probably came after it)"
                );
                state.set_staged_text(s.clone()).await;
                staged = Some(StagedSelection::Text(s));
            }
        } else {
            debug!(
                pane,
                image_age_ms = age.as_millis() as u64,
                "paste: skipping external-text override — staged image is fresh, X11 re-claim may still be propagating"
            );
        }
    } else if let Some(StagedSelection::Text(existing)) = &staged {
        // Stale-text override: the daemon's `staged_text` only refreshes
        // when something calls `flashpaste-trigger --stage-text` (i.e.
        // clipboard-set.sh via tmux's `@clip` pipe on mouse-highlight).
        // When the user copies via kitty's `copy_and_clear_or_interrupt`
        // (Ctrl+C with a live selection) or any non-tmux app, kitty
        // writes the bytes straight to the X11 CLIPBOARD and never tells
        // the daemon. Result: every paste delivers the OLD `staged_text`
        // even though the live clipboard holds bytes the user just
        // copied. Probe live X11 — if it differs, prefer X11.
        let external_text = if should_probe_external_text(state) {
            read_clipboard_text_if_present().await
        } else {
            None
        };
        if let Some(bytes) = external_text {
            if bytes.as_slice() != existing.bytes.as_slice() {
                let s = StagedText {
                    bytes: Arc::new(bytes),
                    captured_at: std::time::SystemTime::now(),
                };
                info!(
                    pane,
                    live_bytes = s.bytes.len(),
                    staged_bytes = existing.bytes.len(),
                    "paste: live X11 text differs from staged_text — using live X11 (kitty Ctrl+C or external app updated the clipboard)"
                );
                state.set_staged_text(s.clone()).await;
                staged = Some(StagedSelection::Text(s));
            }
        }
    }

    match staged {
        Some(StagedSelection::Text(text)) => {
            if !claim_paste_slot(state) {
                return deduped_response(pane, started);
            }
            let bytes = text.bytes.len();
            if let Err(e) = paste::dispatch_text_paste(state.clone(), pane.to_string(), text).await
            {
                error!(error = ?e, pane, "text paste dispatch failed");
                return json!({ "ok": false, "reason": "dispatch-failed", "fallback": "bash" });
            }
            json!({
                "ok": true,
                "kind": "text",
                "bytes": bytes as u64,
                "latency_ms": started.elapsed().as_millis() as u64,
            })
        }
        Some(StagedSelection::Image(img)) if img.is_fresh() => {
            if !claim_paste_slot(state) {
                return deduped_response(pane, started);
            }
            if let Err(e) = paste::dispatch_image_paste(state.clone(), pane.to_string(), img).await
            {
                error!(error = ?e, pane, "image paste dispatch failed");
                return json!({ "ok": false, "reason": "dispatch-failed", "fallback": "bash" });
            }
            json!({
                "ok": true,
                "kind": "image",
                "latency_ms": started.elapsed().as_millis() as u64,
            })
        }
        Some(StagedSelection::Image(_)) => {
            warn!(pane, "paste: staged image too old; punting to bash");
            json!({ "ok": false, "reason": "stale-image", "fallback": "bash" })
        }
        None => {
            info!(
                pane,
                "paste: nothing staged and clipboard has no text; punting to bash"
            );
            json!({ "ok": false, "reason": "no-content", "fallback": "bash" })
        }
    }
}

fn deduped_response(pane: &str, started: Instant) -> Value {
    info!(pane, "paste: duplicate trigger absorbed");
    json!({
        "ok": true,
        "deduped": true,
        "latency_ms": started.elapsed().as_millis() as u64,
    })
}

fn claim_paste_slot(state: &SharedState) -> bool {
    let now = now_unix_ms();
    loop {
        let last = state.last_paste_ms.load(Ordering::Relaxed);
        if is_duplicate_paste(now, last, PASTE_DEDUP_WINDOW_MS) {
            return false;
        }
        if state
            .last_paste_ms
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
    }
}

fn is_duplicate_paste(now_ms: u64, last_ms: u64, window_ms: u64) -> bool {
    last_ms != 0 && now_ms.saturating_sub(last_ms) < window_ms
}

fn should_scan_screenshots(state: &SharedState) -> bool {
    throttle_ms(
        &state.last_screenshot_scan_ms,
        crate::tmux::HOT_PATH_PROBE_THROTTLE_MS,
    )
}

fn should_probe_external_text(state: &SharedState) -> bool {
    throttle_ms(
        &state.last_external_text_probe_ms,
        crate::tmux::HOT_PATH_PROBE_THROTTLE_MS,
    )
}

fn throttle_ms(slot: &std::sync::atomic::AtomicU64, min_interval_ms: u64) -> bool {
    let now = now_unix_ms();
    let last = slot.load(Ordering::Relaxed);
    if now.saturating_sub(last) < min_interval_ms {
        return false;
    }
    slot.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
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

/// Read text from the live Wayland clipboard only when Wayland clearly
/// advertises text and no image target. This is intentionally separate from
/// the X11 fallback: on Mutter, X11 can lag behind a fresh screenshot claim,
/// while Wayland is the authoritative signal that a browser text copy really
/// happened after the screenshot.
async fn read_wayland_text_if_present() -> Option<Vec<u8>> {
    let task = tokio::task::spawn_blocking(|| {
        use wl_clipboard_rs::paste::{get_contents, get_mime_types, ClipboardType, MimeType, Seat};

        let types = get_mime_types(ClipboardType::Regular, Seat::Unspecified).ok()?;
        if types.iter().any(|t| t.starts_with("image/")) {
            return None;
        }
        if !types.iter().any(|t| is_text_target(t)) {
            return None;
        }

        let (mut pipe, _mime) =
            get_contents(ClipboardType::Regular, Seat::Unspecified, MimeType::Text).ok()?;
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes).ok()?;
        if bytes.is_empty() {
            None
        } else {
            Some(bytes)
        }
    });

    tokio::time::timeout(Duration::from_millis(150), task)
        .await
        .ok()?
        .ok()
        .flatten()
}

fn is_text_target(target: &str) -> bool {
    matches!(target, "UTF8_STRING" | "STRING" | "TEXT") || target.starts_with("text/plain")
}

async fn handle_stage(state: &Arc<SharedState>, image_path: &str) -> Value {
    let path = std::path::PathBuf::from(image_path);
    let mime = match path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_lowercase)
    {
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
/// Find the most recently-modified PNG/JPEG file in `dir`. Returns the
/// path and its mtime. Used by `handle_paste` to eagerly pick up
/// screenshots that GNOME has written but whose `CLOSE_WRITE` event
/// hasn't fired yet (the screenshot tool keeps the fd open while
/// rendering its in-shell "saved" toast).
fn newest_image_in(dir: &std::path::Path) -> Option<(std::path::PathBuf, std::time::SystemTime)> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in entries.flatten() {
        let p = entry.path();
        let lower = p
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_lowercase);
        match lower.as_deref() {
            Some("png") | Some("jpg") | Some("jpeg") => {}
            _ => continue,
        }
        // Skip our own compressed siblings (`.fpc.<ext>`).
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(".fpc."))
        {
            continue;
        }
        let mtime = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        match &best {
            Some((cur, _)) if *cur >= mtime => {}
            _ => best = Some((mtime, p)),
        }
    }
    best.map(|(t, p)| (p, t))
}

fn mime_for_path(p: &std::path::Path) -> &'static str {
    match p
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        _ => "image/png",
    }
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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

    #[test]
    fn duplicate_paste_window_absorbs_only_recent_repeats() {
        assert!(!is_duplicate_paste(1_000, 0, PASTE_DEDUP_WINDOW_MS));
        assert!(is_duplicate_paste(1_100, 1_000, PASTE_DEDUP_WINDOW_MS));
        assert!(!is_duplicate_paste(1_400, 1_000, PASTE_DEDUP_WINDOW_MS));
    }

    #[test]
    fn text_target_detection_matches_clipboard_aliases() {
        assert!(is_text_target("text/plain;charset=utf-8"));
        assert!(is_text_target("text/plain"));
        assert!(is_text_target("UTF8_STRING"));
        assert!(!is_text_target("text/html"));
        assert!(!is_text_target("image/png"));
    }
}
