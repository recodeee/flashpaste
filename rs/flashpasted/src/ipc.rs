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
use crate::state::{now_unix_ms, SharedState, StagedImage, StagedText};

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
                            warn!(error = %e, "IPC connection error");
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
        // INFO not DEBUG: this fires every paste (the recursive C-v echo
        // is normal) and v1.20 user-reported "right-click Paste does
        // nothing" debugging needs this visible in journalctl.
        info!(
            pane,
            delta_ms = now - last,
            "paste: dedupe — within recursion window (this is normal for the C-v echo back from kitty send-text)"
        );
        return json!({ "ok": true, "deduped": true });
    }
    // Race-tolerant: store before doing the work so a recursive call's
    // load sees the new timestamp.
    state.last_paste_ms.store(now, Ordering::Relaxed);

    // In-flight guard: while a dispatch is waiting on Claude to become
    // idle, additional paste presses must not spawn parallel dispatches
    // — otherwise 4 queued presses all fire \026 simultaneously when
    // Claude unblocks (observed in journalctl with elapsed_ms=1853,
    // 7600, 16245, 26733 all dispatching within a single second). Skip
    // here; the actively-waiting dispatch will serve the user's intent.
    if state
        .paste_in_flight
        .swap(true, Ordering::AcqRel)
    {
        info!(pane, "paste: another dispatch in flight — dropping duplicate");
        return json!({ "ok": true, "deduped": true, "reason": "in-flight" });
    }

    // ─── Freshness check ─────────────────────────────────────────────
    // Daemon-handled paste requires a staged IMAGE. Text on the
    // clipboard is fine — kitty's own paste_from_clipboard handles it via
    // the tier-1 bash path — so we punt back to bash for that case too.
    let staged = match state.staged_image().await {
        Some(img) if img.is_fresh() => img,
        Some(_) => {
            warn!(pane, "paste: staged image too old; punting to bash");
            return json!({
                "ok": false,
                "reason": "stale-image",
                "fallback": "bash",
            });
        }
        None => {
            info!(
                pane,
                "paste: no staged image (clipboard empty or holds text); punting to bash"
            );
            return json!({
                "ok": false,
                "reason": "no-image",
                "fallback": "bash",
            });
        }
    };

    // ─── Dispatch ────────────────────────────────────────────────────
    match paste::dispatch_image_paste(state.clone(), pane.to_string(), staged).await {
        Ok(()) => json!({
            "ok": true,
            "latency_ms": started.elapsed().as_millis() as u64,
        }),
        Err(e) => {
            error!(error = ?e, "paste dispatch failed; punting to bash");
            json!({
                "ok": false,
                "reason": "dispatch-failed",
                "fallback": "bash",
                "detail": format!("{e:#}"),
            })
        }
    }
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
