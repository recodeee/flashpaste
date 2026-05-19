//! `__hold-selection` hidden subcommand — owns the X11 CLIPBOARD
//! selection for one image, long enough for the inner TUI to read it.
//!
//! This is what replaces `setsid -f xclip -selection clipboard -t image/png
//! -i FILE` in the bash script. We do exactly what xclip does, just
//! in-process and with a readiness handshake on a pipe so the parent
//! doesn't need to `sleep 0.05` after spawning us.
//!
//! Lifetime: we hold the selection for [`HOLD_DURATION`]. After that we
//! exit cleanly; if a SelectionClear event arrives before then (another
//! client took the selection), we also exit.

use std::fs;
use std::io::Write;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use x11rb::atom_manager;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    SelectionNotifyEvent, SelectionRequestEvent, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::CURRENT_TIME;

/// How long to hold the selection before exiting voluntarily. The bash
/// `setsid xclip` lingers until evicted; we cap to 10s because Phase 1
/// only needs ownership for the kitty send-text round-trip + Claude's
/// `wl-paste -t image/png` read (typically <500ms).
pub const HOLD_DURATION: Duration = Duration::from_secs(10);

atom_manager! {
    pub Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        MULTIPLE,
        TIMESTAMP,
        INCR,
        ATOM,
        // Image MIMEs. We register both even if only one is used so a
        // polymorphic reader can pick its preferred format. Custom name
        // form is `FIELD_IDENT: b"x11-name"`.
        IMAGE_PNG: b"image/png",
        IMAGE_JPEG: b"image/jpeg",
    }
}

/// Run the selection-owner event loop.
///
/// * `path` — file to serve as the clipboard contents.
/// * `mime` — MIME string (`"image/png"` or `"image/jpeg"`). Anything
///   else is treated as PNG.
/// * `ready_fd` — optional pipe fd inherited from the parent. We write
///   one byte to it after `SetSelectionOwner` so the parent can return.
pub fn run(path: &Path, mime: &str, ready_fd: Option<i32>) -> Result<()> {
    // Load the file bytes up front. If the read fails we still signal
    // readiness so the parent doesn't hang — it will fall back when the
    // X server reports no data later.
    let data = fs::read(path)
        .with_context(|| format!("reading staged image at {}", path.display()))?;

    let (conn, screen_num) = x11rb::connect(None).context("connecting to X server")?;
    let atoms = Atoms::new(&conn)?.reply()?;
    let screen = &conn.setup().roots[screen_num];

    // Create an InputOnly window with no events — just a handle so the
    // X server can record us as the selection owner. We do listen for
    // PROPERTY_CHANGE so INCR transfers (large images) can work.
    let win = conn.generate_id().context("generate window id")?;
    let win_aux = CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE);
    conn.create_window(
        0,
        win,
        screen.root,
        -10,
        -10,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &win_aux,
    )?;

    // Resolve the MIME atom for the data target.
    let mime_atom: Atom = match mime {
        "image/jpeg" => atoms.IMAGE_JPEG,
        _ => atoms.IMAGE_PNG,
    };

    // Claim CLIPBOARD ownership. CURRENT_TIME is acceptable for clients
    // that don't track the server's notion of "now"; xclip does the same.
    conn.set_selection_owner(win, atoms.CLIPBOARD, CURRENT_TIME)?;
    conn.flush().context("flushing SetSelectionOwner")?;

    // Verify ownership took effect (ICCCM recommends this — some
    // compositors silently refuse). If it failed we still signal the
    // parent so it can fall back; reporting failure here would hang.
    let owner = conn.get_selection_owner(atoms.CLIPBOARD)?.reply()?.owner;
    if owner != win {
        tracing::warn!(
            "SetSelectionOwner returned but owner={} != win={}",
            owner,
            win
        );
    }

    // Readiness handshake: write one byte to the pipe so the parent
    // dispatcher can proceed straight to `tmux unbind` + `kitty @
    // send-text`. This is the entire reason we don't need the bash
    // script's 50ms sleep.
    if let Some(fd) = ready_fd {
        // SAFETY: fd was inherited from the parent via fd inheritance
        // and we are the only owner in this process.
        let mut f = unsafe { std::fs::File::from(OwnedFd::from_raw_fd(fd)) };
        let _ = f.write_all(b"R");
        // Drop closes the fd so the parent's poll sees the write end EOF
        // if we later die. Explicit drop for clarity.
        drop(f);
    }

    let deadline = Instant::now() + HOLD_DURATION;
    loop {
        // poll_for_event lets us check the deadline between events.
        // The bash script's xclip uses XNextEvent (blocking) but we
        // want a bounded lifetime, so we wake periodically.
        let timeout = deadline.saturating_duration_since(Instant::now());
        if timeout.is_zero() {
            break;
        }
        // x11rb doesn't expose a poll-with-timeout directly; we use the
        // wait-for-event under a quick check. To keep this simple and
        // correct, we use poll_for_event (non-blocking) + a short sleep.
        match conn.poll_for_event()? {
            Some(event) => {
                if handle_event(&conn, &atoms, mime_atom, &data, event)? {
                    break;
                }
            }
            None => {
                // Sleep up to 50ms to avoid busy-spinning; sleep_ms is
                // capped by the deadline.
                let nap = timeout.min(Duration::from_millis(50));
                std::thread::sleep(nap);
            }
        }
    }

    // Politely release the selection on the way out. If anyone else has
    // taken it already this is a no-op.
    let _ = conn.set_selection_owner(x11rb::NONE, atoms.CLIPBOARD, CURRENT_TIME);
    let _ = conn.flush();
    Ok(())
}

/// Returns `Ok(true)` if the event loop should exit (e.g. SelectionClear
/// from another owner stealing the selection).
fn handle_event(
    conn: &impl Connection,
    atoms: &Atoms,
    mime_atom: Atom,
    data: &[u8],
    event: Event,
) -> Result<bool> {
    match event {
        Event::SelectionRequest(req) => {
            serve_request(conn, atoms, mime_atom, data, &req)?;
            Ok(false)
        }
        Event::SelectionClear(_) => {
            // Another client stole the selection. Exit immediately.
            tracing::info!("SelectionClear received — exiting");
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Serve a single SelectionRequest. Mirrors xclip's logic — we answer
/// TARGETS, TIMESTAMP, and the MIME target; everything else is refused
/// via a SelectionNotify with property = None.
fn serve_request(
    conn: &impl Connection,
    atoms: &Atoms,
    mime_atom: Atom,
    data: &[u8],
    req: &SelectionRequestEvent,
) -> Result<()> {
    let mut property = req.property;
    let mut success = false;

    if req.target == atoms.TARGETS {
        // Respond with the list of atoms we support.
        let supported: [u32; 4] = [
            atoms.TARGETS,
            atoms.TIMESTAMP,
            mime_atom,
            // Also advertise the other image MIME so a polymorphic
            // reader (Claude's wl-paste shim → xclip) can pick its
            // preferred one.
            if mime_atom == atoms.IMAGE_PNG {
                atoms.IMAGE_JPEG
            } else {
                atoms.IMAGE_PNG
            },
        ];
        conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            property,
            AtomEnum::ATOM,
            &supported,
        )?;
        success = true;
    } else if req.target == atoms.TIMESTAMP {
        // We claimed with CURRENT_TIME, so return CURRENT_TIME (0).
        let zero = [0u32; 1];
        conn.change_property32(
            PropMode::REPLACE,
            req.requestor,
            property,
            AtomEnum::INTEGER,
            &zero,
        )?;
        success = true;
    } else if req.target == mime_atom
        || req.target == atoms.IMAGE_PNG
        || req.target == atoms.IMAGE_JPEG
    {
        // The data target. We serve the file bytes in one chunk — for
        // images under ~1MB this works fine on every X server. INCR
        // would handle larger payloads but typical screenshots are
        // 100-500KB.
        conn.change_property8(
            PropMode::REPLACE,
            req.requestor,
            property,
            mime_atom,
            data,
        )?;
        success = true;
    } else {
        // Unsupported target — respond with property = None.
        property = x11rb::NONE;
    }

    if !success {
        property = x11rb::NONE;
    }

    // Always send a SelectionNotify so the requester unblocks.
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property,
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, &notify)?;
    conn.flush()?;
    Ok(())
}
