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
use crate::state::{now_unix_ms, SharedState, StagedImage};

/// Recursion guard window. A `paste` op arriving within this many ms of the
/// previous one is treated as the tmux `bind -n C-v` recursion (see fact #2
/// in the spec) and replied as `{"ok":true,"deduped":true}`.
const RECURSION_DEDUPE_MS: u64 = 1500;

/// Cap incoming request size so a confused (or hostile) caller can't make
/// the daemon allocate gigabytes. 16KB is more than enough for the JSON we
/// expect; `stage` with a path is the largest realistic payload.
const MAX_REQUEST_BYTES: u32 = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Request {
    Paste {
        pane: String,
        #[allow(dead_code)]
        ts: Option<String>,
    },
    Stage {
        image_path: String,
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
    // ─── Recursion guard ─────────────────────────────────────────────
    // Fact #2 from the spec: tmux's `bind -n C-v` re-fires when the kitty
    // `send_text \026` byte reaches tmux. The trigger binary calls us
    // again; we dedupe based on the last-paste timestamp.
    let now = now_unix_ms();
    let last = state.last_paste_ms.load(Ordering::Relaxed);
    if now.saturating_sub(last) < RECURSION_DEDUPE_MS {
        debug!(
            delta_ms = now - last,
            "paste dedupe — within recursion window"
        );
        return json!({ "ok": true, "deduped": true });
    }
    // Race-tolerant: store before doing the work so a recursive call's
    // load sees the new timestamp.
    state.last_paste_ms.store(now, Ordering::Relaxed);

    // ─── Freshness check ─────────────────────────────────────────────
    // Daemon-handled paste requires a staged image. Without one we punt
    // back to bash, which handles the text-paste branches via terminal-
    // specific paths.
    let staged = match state.staged_snapshot().await {
        Some(img) if img.is_fresh() => img,
        Some(_) => {
            warn!("staged image too old; daemon punting to bash");
            return json!({
                "ok": false,
                "reason": "stale-image",
                "fallback": "bash",
            });
        }
        None => {
            debug!("no staged image; daemon punting to bash");
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
