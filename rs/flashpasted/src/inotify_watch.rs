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
            let mime = mime_for(&path);
            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to read new screenshot"
                    );
                    continue;
                }
            };
            let len = bytes.len();
            let staged = StagedImage {
                bytes: Arc::new(bytes),
                mime,
                path: path.clone(),
                captured_at: SystemTime::now(),
            };
            // Cross thread back into tokio for the write. block_on inside
            // spawn_blocking is fine — we're not on a worker.
            let state_clone = state.clone();
            handle.block_on(async move {
                state_clone.set_staged_image(staged).await;
            });
            info!(
                path = %path.display(),
                bytes = len,
                mime = mime,
                "staged screenshot from inotify"
            );
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
