//! Wayland clipboard ownership.
//!
//! We own the Wayland CLIPBOARD selection for the lifetime of the daemon.
//! On every staged-image refresh we tear the previous owner down and start
//! a new `wl_data_source` advertising the staged MIME type + bytes.
//!
//! Why the daemon and not the legacy `wl-copy --paste-once`:
//!   - `--paste-once` exits after serving exactly one receive. Any probe
//!     in the dispatch pipeline drains it before Claude Code reads. The
//!     daemon serves unlimited reads from in-memory bytes.
//!   - Each `wl-copy` spawn registers as a transient client in Mutter's
//!     dock ("Unknown" gear icon). One long-lived client with a stable
//!     `app_id` solves the dock-flash described in the README.
//!
//! Mutter wedge handling:
//!   `wl-clipboard-rs`'s `copy::copy(...)` can fail on surfaceless clients
//!   (Mutter occasionally rejects ownership claims). When it does, we log
//!   and continue. X11 ownership in the sibling module is the backup.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, error, info, warn};
use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, ServeRequests, Source};

use crate::state::SharedState;

/// The Wayland `app_id` the daemon advertises. Combined with a NoDisplay
/// `flashpasted.desktop` file (shipped by `install.sh` in a follow-up
/// phase), this hides the clipboard owner from the GNOME dock instead of
/// showing it as "Unknown".
pub const APP_ID: &str = "org.flashpaste.daemon";

pub fn spawn_owner(state: Arc<SharedState>) {
    // Dedicated task — wl-clipboard-rs's blocking `copy()` is moved onto
    // `spawn_blocking` per call so we never starve the executor.
    tokio::spawn(async move {
        run_owner(state).await;
    });
}

async fn run_owner(state: Arc<SharedState>) {
    let mut rx = state.stage_notifier_rx.clone();
    // Initial state: no image staged. We could try to claim an empty
    // selection but Mutter doesn't like that — wait for the first image.
    info!(app_id = APP_ID, "Wayland clipboard owner ready (waiting for first staged image)");

    loop {
        // Block until the next staged-image revision.
        match rx.changed().await {
            Ok(()) => {}
            Err(_) => {
                warn!("stage notifier closed; Wayland owner exiting");
                return;
            }
        }
        let Some(staged) = state.staged_snapshot().await else {
            continue;
        };
        let bytes = staged.bytes.clone();
        let mime = staged.mime;
        debug!(bytes = bytes.len(), mime, "Wayland: refreshing selection");

        // Move the blocking `copy()` call to a dedicated blocking thread so
        // the executor stays responsive while the Wayland event loop runs.
        // The `wl-clipboard-rs` crate internally forks/threads its own
        // serving loop; we own the lifetime.
        let bytes_clone = (*bytes).clone();
        let result = tokio::task::spawn_blocking(move || {
            install_owner(&bytes_clone, mime)
        })
        .await;

        match result {
            Ok(Ok(())) => {
                debug!(mime, "Wayland: ownership installed");
            }
            Ok(Err(e)) => {
                // Mutter wedge case. Don't crash — X11 owner is our backup.
                warn!(
                    error = %e,
                    "Wayland copy() failed (likely Mutter wedge); continuing with X11 only"
                );
            }
            Err(e) => {
                error!(error = %e, "Wayland blocking task panicked");
            }
        }
    }
}

/// Install ourselves as the Wayland CLIPBOARD owner for `bytes` under `mime`.
///
/// Per wl-clipboard-rs API (v0.9): `Options::copy(source, mime)` is the
/// one-shot helper but it doesn't fit our use case (we want to refresh).
/// `Options::copy_multi(sources)` accepts a `Vec<MimeSource>` and serves
/// each MIME type forever. We use that to advertise the image MIME plus
/// an `image/png` alias so applications that hardcode that query succeed
/// even when the file is actually JPEG.
fn install_owner(bytes: &[u8], mime: &'static str) -> anyhow::Result<()> {
    let mut opts = Options::new();
    // foreground=false: let wl-clipboard-rs daemonize the serving thread
    // internally; our daemon process owns the connection's lifetime via
    // tokio. foreground=true would block this thread forever.
    opts.foreground(false);
    // Serve unlimited receives — the whole point of this daemon.
    opts.serve_requests(ServeRequests::Unlimited);

    // Always advertise the primary MIME. If it's a JPEG, also advertise
    // `image/png` as a duplicate-bytes alias so paste readers that only
    // ask for PNG don't get an empty clipboard. (Mutter passes through
    // whatever MIME a reader asks for; clients that ask for PNG and get
    // JPEG bytes typically detect the mismatch and either reject or
    // re-decode. Either way we don't make things worse.)
    let mut seen: HashSet<&'static str> = HashSet::new();
    let mut sources: Vec<MimeSource> = Vec::new();
    seen.insert(mime);
    sources.push(MimeSource {
        source: Source::Bytes(bytes.to_vec().into()),
        mime_type: MimeType::Specific(mime.to_string()),
    });
    // The wl-clipboard-rs API requires owned `Vec<u8>` per source; the
    // duplicate cost is fine — staged screenshots are 100KB-2MB.
    if mime != "image/png" && seen.insert("image/png") {
        sources.push(MimeSource {
            source: Source::Bytes(bytes.to_vec().into()),
            mime_type: MimeType::Specific("image/png".to_string()),
        });
    }

    // `copy_multi` claims the selection then spawns its own internal worker
    // to serve receives. It returns once ownership is claimed.
    opts.copy_multi(sources)?;

    // Smooth-out: give Mutter a few ms to register the new owner before we
    // potentially refresh again. Without this a rapid burst of inotify
    // events could collide on the same wayland_display.
    std::thread::sleep(Duration::from_millis(10));
    Ok(())
}

/// Wayland-authoritative `has_image` check (fact #3 from the spec).
///
/// The daemon owns the Wayland clipboard, so during normal operation this
/// helper is unused. It's provided for future code paths (e.g. Phase 3 when
/// the daemon needs to decide whether to ignore a paste because the user
/// recently copied text on top of a screenshot) so the bash dispatcher's
/// hard-won policy isn't lost.
#[allow(dead_code)]
pub fn read_has_image() -> bool {
    use wl_clipboard_rs::paste::{get_mime_types, ClipboardType, Seat};
    match get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
        Ok(types) => types.iter().any(|t| t.starts_with("image/")),
        Err(_) => false, // Mutter silent; caller should fall back to X11.
    }
}
