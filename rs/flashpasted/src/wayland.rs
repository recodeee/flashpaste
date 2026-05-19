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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, error, info, warn};
use wl_clipboard_rs::copy::{MimeSource, MimeType, Options, ServeRequests, Source};

use crate::state::{SharedState, StagedSelection};

/// The Wayland `app_id` the daemon advertises. Combined with a NoDisplay
/// `flashpasted.desktop` file (shipped by `install.sh` in a follow-up
/// phase), this hides the clipboard owner from the GNOME dock instead of
/// showing it as "Unknown".
pub const APP_ID: &str = "org.flashpaste.daemon";

/// Latched once the compositor proves it doesn't speak any data-control
/// protocol (e.g. Mutter on GNOME 46). After that, every stage event would
/// just `spawn_blocking` a doomed `copy_multi` that immediately errors and
/// fills the journal. Latching here lets the paste re-assert path skip the
/// blocking task entirely — only X11 is doing useful work on this box.
static WAYLAND_WEDGED: AtomicBool = AtomicBool::new(false);

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
        let (bytes_clone, mimes): (Vec<u8>, Vec<&'static str>) = match staged {
            StagedSelection::Image(img) => {
                let bytes = (*img.bytes).clone();
                debug!(bytes = bytes.len(), mime = img.mime, "Wayland: refreshing image selection");
                // Always advertise the primary plus `image/png` alias.
                let mut mimes = vec![img.mime];
                if img.mime != "image/png" {
                    mimes.push("image/png");
                }
                (bytes, mimes)
            }
            StagedSelection::Text(txt) => {
                let bytes = (*txt.bytes).clone();
                debug!(bytes = bytes.len(), "Wayland: refreshing text selection");
                // The MIME types real apps query for plain text on the
                // Wayland clipboard. Order is "best first" though Wayland
                // doesn't actually rank; clients query by exact match.
                (
                    bytes,
                    vec![
                        "text/plain;charset=utf-8",
                        "text/plain",
                        "UTF8_STRING",
                        "STRING",
                        "TEXT",
                    ],
                )
            }
        };

        // Mutter on GNOME 46 doesn't implement `ext-data-control` /
        // `wlr-data-control`. Once we've observed that, every further
        // `copy_multi` call is guaranteed to error with the same protocol
        // message — skip it entirely instead of paying spawn_blocking +
        // wl-clipboard-rs setup cost on every paste re-assert.
        if WAYLAND_WEDGED.load(Ordering::Relaxed) {
            debug!("Wayland: skipping copy() — compositor has no data-control protocol");
            continue;
        }

        // Move the blocking `copy()` call to a dedicated blocking thread so
        // the executor stays responsive while the Wayland event loop runs.
        // The `wl-clipboard-rs` crate internally forks/threads its own
        // serving loop; we own the lifetime.
        let result = tokio::task::spawn_blocking(move || {
            install_owner(&bytes_clone, &mimes)
        })
        .await;

        match result {
            Ok(Ok(())) => {
                debug!("Wayland: ownership installed");
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                // Latch only on the "compositor lacks protocol" failure
                // mode — that one is permanent. Transient claim rejections
                // (focus races etc.) should keep retrying on next stage.
                if msg.contains("ext-data-control") || msg.contains("wlr-data-control") {
                    if !WAYLAND_WEDGED.swap(true, Ordering::Relaxed) {
                        warn!(
                            error = %e,
                            "Wayland copy() failed: compositor has no data-control protocol — latching off, X11 owner will handle paste"
                        );
                    }
                } else {
                    warn!(
                        error = %e,
                        "Wayland copy() failed (likely Mutter wedge); continuing with X11 only"
                    );
                }
            }
            Err(e) => {
                error!(error = %e, "Wayland blocking task panicked");
            }
        }
    }
}

/// Install ourselves as the Wayland CLIPBOARD owner for `bytes` advertised
/// under every MIME in `mimes`. wl-clipboard-rs's `copy_multi` claims the
/// selection then spawns its own serving worker; we return as soon as
/// ownership lands.
fn install_owner(bytes: &[u8], mimes: &[&'static str]) -> anyhow::Result<()> {
    let mut opts = Options::new();
    // foreground=false: let wl-clipboard-rs daemonize the serving thread
    // internally; our daemon process owns the connection's lifetime via
    // tokio. foreground=true would block this thread forever.
    opts.foreground(false);
    // Serve unlimited receives — the whole point of this daemon.
    opts.serve_requests(ServeRequests::Unlimited);

    let mut seen: HashSet<&'static str> = HashSet::new();
    let mut sources: Vec<MimeSource> = Vec::new();
    for m in mimes {
        if !seen.insert(*m) {
            continue;
        }
        sources.push(MimeSource {
            source: Source::Bytes(bytes.to_vec().into()),
            mime_type: MimeType::Specific((*m).to_string()),
        });
    }

    // `copy_multi` claims the selection then spawns its own internal worker
    // to serve receives. It returns once ownership is claimed.
    opts.copy_multi(sources)?;

    // Smooth-out: give Mutter a few ms to register the new owner before we
    // potentially refresh again. Without this a rapid burst of stage
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
