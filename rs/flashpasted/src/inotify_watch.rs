//! Inotify watcher on `~/Pictures/Screenshots/`.
//!
//! GNOME's PrtScr saves a file but doesn't copy to the clipboard (fact #4
//! from the spec). The bash dispatcher worked around this by polling the
//! dir on every paste. The daemon does it properly: one persistent inotify
//! handle, fires `IN_CLOSE_WRITE` the instant a file is finished writing,
//! reads the bytes into memory, and stages them into both clipboard owners.
//!
//! Why `spawn_blocking` + sync `inotify`:
//!   The sync API is dead simple (an iterator over events). The async API
//!   adds a tokio-stream dependency we don't need. Inotify events are sparse
//!   (one per screenshot), so a dedicated blocking thread is the right shape.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use flashpaste_common::compress;
use inotify::{Inotify, WatchMask};
use tracing::{debug, error, info, warn};

use crate::state::{SharedState, StagedImage};

pub fn spawn_watcher(state: Arc<SharedState>) {
    let Some(dir) = state.config.screenshots_dir.clone() else {
        warn!("no screenshots_dir configured; inotify watcher disabled");
        return;
    };

    // Ensure the directory exists. Don't create it ourselves — that'd hide
    // a config typo. Log and return so the daemon still serves staged data
    // for non-screenshot sources (the `stage` IPC op).
    if !dir.is_dir() {
        warn!(
            path = %dir.display(),
            "screenshots dir doesn't exist; inotify watcher not started"
        );
        return;
    }

    // Startup prestage: scan the dir for the freshest screenshot and stage
    // it into the daemon. Without this, every daemon restart (rebuild,
    // boot, manual `systemctl restart`) leaves the in-memory staged_image
    // empty until the *next* screenshot fires inotify — meaning the user's
    // first paste after restart silently punts to the bash dispatcher
    // (~1-21 s, includes its own claude-idle-check). With this, the daemon
    // owns the fast path immediately on startup.
    //
    // Bounded to files modified in the last 5 minutes so we don't stage a
    // stale screenshot from an earlier session.
    let state_for_prestage = state.clone();
    let dir_for_prestage = dir.clone();
    let handle_for_prestage = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        if let Some((path, bytes, mime)) = find_freshest_image(&dir_for_prestage, 300) {
            let len = bytes.len();
            let staged = StagedImage {
                bytes: Arc::new(bytes),
                mime,
                path: path.clone(),
                captured_at: SystemTime::now(),
            };
            handle_for_prestage.block_on(async move {
                state_for_prestage.set_staged_image(staged).await;
            });
            info!(
                path = %path.display(),
                bytes = len,
                mime = mime,
                "startup prestage: staged latest screenshot"
            );
        } else {
            debug!("startup prestage: no fresh screenshot in dir");
        }
    });

    // The actual blocking loop runs on a spawn_blocking thread because the
    // sync `inotify` iterator parks the OS thread on `read`. We bridge back
    // into tokio via `Handle::current().block_on(...)` for the staging
    // write, which is fine — staging is rare (≤1 per screenshot).
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = run_watcher(state, dir, handle) {
            error!(error = ?e, "inotify watcher exited with error");
        }
    });
}

fn run_watcher(
    state: Arc<SharedState>,
    dir: PathBuf,
    handle: tokio::runtime::Handle,
) -> anyhow::Result<()> {
    let mut inotify = Inotify::init()?;
    // IN_CLOSE_WRITE is the right event: GNOME Screenshot finishes writing
    // the PNG, closes the fd, and we get a single event with the final
    // filename. IN_CREATE would fire too early (before the bytes are flushed).
    // IN_MOVED_TO covers tools that atomic-rename a tempfile into place.
    inotify.watches().add(
        &dir,
        WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO,
    )?;
    info!(
        path = %dir.display(),
        "inotify watcher started on screenshots dir"
    );

    // 64 KiB buffer is overkill for inotify but it's allocated once. Each
    // event is ~16 bytes + the filename.
    let mut buf = [0u8; 65_536];
    loop {
        let events = match inotify.read_events_blocking(&mut buf) {
            Ok(it) => it,
            Err(e) => {
                error!(error = %e, "inotify read failed; retrying in 1s");
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };

        for event in events {
            let Some(name) = event.name else { continue };
            let path = dir.join(name);
            if !is_image_path(&path) {
                debug!(path = %path.display(), "ignoring non-image inotify event");
                continue;
            }
            // Don't recurse on the compressed siblings we drop next to
            // the original. Their filenames embed `.fpc.` (see
            // `make_compressed_tmp_path`).
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".fpc."))
            {
                debug!(path = %path.display(), "ignoring compressed-sibling event");
                continue;
            }

            // ── Fast path: stage RAW bytes immediately, compress later.
            //
            // User feedback (2026-05-19): "screenshot has big delay, can
            // it be lightspeed fast?" — the synchronous compression call
            // below (`compress_for_attach_env`) re-encodes 4K multimon
            // PNGs (12 MB+) to WebP, which can take 500-2000ms on this
            // box. During that window the daemon's staged_image slot
            // still holds whatever was there before (often stale text),
            // so a paste right after PrtScr delivers the OLD content.
            //
            // Fix: read raw bytes from disk, stage them right now (single
            // file read, ~5-20ms), then kick off compression in a
            // background spawn_blocking thread that re-stages with the
            // smaller artifact once it's ready. The brief window where
            // we serve raw bytes is fine — X11 SelectionRequest serves
            // from our in-memory buffer regardless of size.
            let raw_bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to read new screenshot — skipping stage");
                    continue;
                }
            };
            let raw_len = raw_bytes.len();
            let raw_mime = mime_for(&path);
            let staged_raw = StagedImage {
                bytes: Arc::new(raw_bytes),
                mime: raw_mime,
                path: path.clone(),
                captured_at: SystemTime::now(),
            };
            {
                let state_clone = state.clone();
                handle.block_on(async move {
                    state_clone.set_staged_image(staged_raw).await;
                });
            }
            info!(
                path = %path.display(),
                bytes = raw_len,
                mime = raw_mime,
                "staged screenshot from inotify (raw, pre-compress)"
            );

            // Now spawn the compression on a separate blocking thread so
            // the inotify loop can immediately serve the next event. If
            // compression produces smaller bytes, re-stage. If it errors
            // or produces the same bytes, leave the raw stage in place.
            let path_for_compress = path.clone();
            let state_for_compress = state.clone();
            let handle_for_compress = handle.clone();
            std::thread::spawn(move || {
                let result = compress::compress_for_attach_env(&path_for_compress);
                // Reuse the original compression decision tree but only
                // *replace* the stage if it actually wins on bytes — never
                // regress from a smaller raw to a larger compressed result.
                let compressed: Option<(Vec<u8>, &'static str, std::path::PathBuf)> = match result {
                    Ok((b, m)) if m == "image/png" || m == "image/jpeg" => {
                        // Pass-through (no real compression happened, or
                        // shape was already acceptable). Only re-stage
                        // if bytes actually shrank — otherwise the raw
                        // stage we did synchronously above is better.
                        if b.len() < raw_len {
                            Some((b, mime_for_string(&m), path_for_compress.clone()))
                        } else {
                            None
                        }
                    }
                    Ok((b, m)) => {
                        // Re-encoded (likely WebP). Write sibling tmpfile.
                        let tmp = make_compressed_tmp_path(&path_for_compress, &m);
                        match std::fs::write(&tmp, &b) {
                            Ok(()) => {
                                info!(
                                    src = %path_for_compress.display(),
                                    dst = %tmp.display(),
                                    bytes = b.len(),
                                    mime = %m,
                                    "wrote compressed sibling for staging"
                                );
                                Some((b, mime_for_string(&m), tmp))
                            }
                            Err(e) => {
                                warn!(
                                    path = %tmp.display(),
                                    error = %e,
                                    "failed to write compressed sibling — keeping raw stage"
                                );
                                None
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            path = %path_for_compress.display(),
                            error = ?e,
                            "compress_for_attach failed — keeping raw stage"
                        );
                        None
                    }
                };

                if let Some((bytes, mime, staged_path)) = compressed {
                    let len = bytes.len();
                    let staged = StagedImage {
                        bytes: Arc::new(bytes),
                        mime,
                        path: staged_path,
                        captured_at: SystemTime::now(),
                    };
                    handle_for_compress.block_on(async move {
                        state_for_compress.set_staged_image(staged).await;
                    });
                    info!(
                        path = %path_for_compress.display(),
                        bytes = len,
                        mime = mime,
                        raw_bytes = raw_len,
                        "re-staged screenshot after background compression"
                    );
                }
            });
        }
    }
}

fn is_image_path(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|s| s.to_str())
            .map(str::to_lowercase)
            .as_deref(),
        Some("png") | Some("jpg") | Some("jpeg")
    )
}

fn mime_for(p: &Path) -> &'static str {
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

/// `StagedImage::mime` is a `&'static str`, so anything we get back from
/// `compress_for_attach` needs to be promoted to one of the three known
/// constants. Unknown MIMEs are coerced to `image/png` (the most
/// permissive consumer-side).
fn mime_for_string(s: &str) -> &'static str {
    match s {
        "image/jpeg" => "image/jpeg",
        "image/webp" => "image/webp",
        _ => "image/png",
    }
}

/// Compose a sibling path next to `original` with the compressed
/// MIME's extension. E.g. `screenshot.png` + `image/webp` → `screenshot.png.fpc.webp`.
/// The `.fpc.` infix marks it as a flashpaste-compressed sibling so a
/// future cleanup pass can identify (and reap) these files without
/// guessing.
/// Scan `dir` for the most recently-modified PNG/JPEG, return its bytes
/// if it was written within the last `max_age_secs` seconds. Used at
/// startup so the daemon doesn't begin life with an empty staged_image
/// after every restart.
fn find_freshest_image(dir: &Path, max_age_secs: u64) -> Option<(PathBuf, Vec<u8>, &'static str)> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_image_path(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        match &best {
            Some((current, _)) if *current >= mtime => {}
            _ => best = Some((mtime, path)),
        }
    }
    let (mtime, path) = best?;
    let age = SystemTime::now().duration_since(mtime).ok()?;
    if age.as_secs() > max_age_secs {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let mime = mime_for(&path);
    Some((path, bytes, mime))
}

fn make_compressed_tmp_path(original: &Path, mime: &str) -> PathBuf {
    let ext = match mime {
        "image/webp" => "webp",
        "image/jpeg" => "jpg",
        _ => "png",
    };
    let mut owned = original.to_path_buf();
    let file_name = original
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "image".to_string());
    owned.set_file_name(format!("{file_name}.fpc.{ext}"));
    owned
}
