//! X11 clipboard ownership.
//!
//! XWayland's CLIPBOARD mirror is the fallback when Mutter wedges the
//! Wayland clipboard (see fact #3 from the spec, and the "wedged data-
//! device" notes in the bash dispatcher).
//!
//! Design:
//!   * One long-lived `x11rb` connection, held forever by a dedicated
//!     blocking thread (spawn_blocking).
//!   * A hidden InputOnly window is the selection owner.
//!   * On every staged-image refresh we call SetSelectionOwner.
//!   * SelectionRequest events arrive on the same connection; we serve the
//!     staged bytes if the requested target is one of TARGETS,
//!     image/png, or image/jpeg.
//!
//! Why a separate thread instead of tokio:
//!   `x11rb::Connection`'s `wait_for_event()` is blocking; integrating it
//!   into tokio's reactor is more complexity than it's worth for an event
//!   stream this sparse. We just park the OS thread on `poll_for_event` /
//!   `wait_for_event`.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, error, info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    SelectionNotifyEvent, SelectionRequestEvent, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

use crate::state::{SharedState, StagedImage};

pub fn spawn_owner(state: Arc<SharedState>) {
    tokio::task::spawn_blocking(move || {
        if let Err(e) = run(state) {
            error!(error = ?e, "X11 owner exited with error");
        }
    });
}

/// Atom handles we look up once at connection time.
struct Atoms {
    clipboard: u32,
    targets: u32,
    timestamp: u32,
    image_png: u32,
    image_jpeg: u32,
    multiple: u32,
    atom_pair: u32,
}

impl Atoms {
    fn intern(conn: &RustConnection) -> Result<Self> {
        let clipboard = conn
            .intern_atom(false, b"CLIPBOARD")?
            .reply()?
            .atom;
        let targets = conn.intern_atom(false, b"TARGETS")?.reply()?.atom;
        let timestamp = conn.intern_atom(false, b"TIMESTAMP")?.reply()?.atom;
        let image_png = conn.intern_atom(false, b"image/png")?.reply()?.atom;
        let image_jpeg = conn.intern_atom(false, b"image/jpeg")?.reply()?.atom;
        let multiple = conn.intern_atom(false, b"MULTIPLE")?.reply()?.atom;
        let atom_pair = conn.intern_atom(false, b"ATOM_PAIR")?.reply()?.atom;
        Ok(Self {
            clipboard,
            targets,
            timestamp,
            image_png,
            image_jpeg,
            multiple,
            atom_pair,
        })
    }
}

fn run(state: Arc<SharedState>) -> Result<()> {
    let (conn, screen_num) = match x11rb::connect(None) {
        Ok(c) => c,
        Err(e) => {
            warn!(
                error = %e,
                "X11 connect failed (no DISPLAY or XWayland down); X11 owner disabled"
            );
            return Ok(());
        }
    };

    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let window = conn.generate_id()?;
    // InputOnly hidden window. Listen for property changes (incremental
    // transfer ack from the requestor — we don't currently need INCR but
    // the mask is harmless to set).
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        root,
        -1,
        -1,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::NONE,
        &CreateWindowAux::default().event_mask(EventMask::PROPERTY_CHANGE),
    )?;
    // Set a recognizable WM_NAME so xwininfo / xprop debugging shows the
    // owner clearly.
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME.into(),
        AtomEnum::STRING.into(),
        b"flashpasted",
    )?;
    conn.flush()?;

    let atoms = Atoms::intern(&conn)?;
    info!(window, "X11 owner connection up");

    // We need to know "the latest staged image" without locking on every
    // SelectionRequest. The async owner refreshes via SetSelectionOwner +
    // a synchronous `staged_snapshot_blocking()` read inside the handler.
    let mut current_revision: u64 = 0;
    let mut rx = state.stage_notifier_rx.clone();

    loop {
        // Poll for X events, then check for staging revisions.
        //
        // We can't use `wait_for_event` because then a new screenshot wouldn't
        // trigger a SetSelectionOwner until the next X event arrives. So we
        // poll with a short timeout via `poll_for_event` + sleep, which is
        // ugly but reliable. x11rb 0.13 doesn't expose `wait_for_event_with_
        // sequence_number` in a way that's compatible with tokio without
        // taking a chunky dep.
        match conn.poll_for_event() {
            Ok(Some(event)) => {
                if let Err(e) = handle_event(&conn, window, &atoms, &state, event) {
                    warn!(error = ?e, "X11 event handling error");
                }
            }
            Ok(None) => {
                // No event; check for a refresh.
                let revision = *rx.borrow();
                if revision != current_revision {
                    current_revision = revision;
                    if state.staged_snapshot_blocking().is_some() {
                        // Take ownership of the CLIPBOARD selection. The
                        // CurrentTime (0) sentinel is acceptable here since
                        // we have no recent X timestamp; XWayland mirrors
                        // it through to Mutter regardless.
                        if let Err(e) = conn.set_selection_owner(
                            window,
                            atoms.clipboard,
                            x11rb::CURRENT_TIME,
                        ) {
                            warn!(error = %e, "set_selection_owner failed");
                        }
                        let _ = conn.flush();
                        debug!(revision, "X11: claimed CLIPBOARD ownership");
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                error!(error = %e, "X11 poll_for_event failed; exiting owner thread");
                return Err(e.into());
            }
        }
    }
}

fn handle_event(
    conn: &RustConnection,
    window: u32,
    atoms: &Atoms,
    state: &Arc<SharedState>,
    event: Event,
) -> Result<()> {
    match event {
        Event::SelectionRequest(req) => handle_selection_request(conn, window, atoms, state, req),
        Event::SelectionClear(clear) => {
            // Someone else claimed CLIPBOARD; that's fine, we'll reclaim on
            // the next staged-image refresh.
            debug!(selection = clear.selection, "X11: SelectionClear received");
            Ok(())
        }
        _ => Ok(()),
    }
}

fn handle_selection_request(
    conn: &RustConnection,
    window: u32,
    atoms: &Atoms,
    state: &Arc<SharedState>,
    req: SelectionRequestEvent,
) -> Result<()> {
    // Build the SelectionNotify we'll send to the requestor. `property == 0`
    // signals "refused" per ICCCM. We set it to the same atom as the
    // request's `property` field on success.
    let mut notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property: 0, // refused by default
    };
    // ICCCM says: if `property == NONE` the requestor wants the data back
    // in `target` itself. (Mostly an obsolete pre-ICCCM convention; we
    // honor it anyway.)
    let property = if req.property == x11rb::NONE {
        req.target
    } else {
        req.property
    };

    // Snapshot the staged image once for this request. None means we don't
    // have anything to serve — reply with property=0.
    let Some(staged) = state.staged_snapshot_blocking() else {
        conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
        conn.flush()?;
        return Ok(());
    };

    let served = serve_target(conn, window, atoms, &staged, req.requestor, property, req.target)
        .with_context(|| format!("serve target atom={}", req.target))?;
    if served {
        notify.property = property;
    }
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
    conn.flush()?;
    Ok(())
}

/// Serve `target` to `requestor` on `property`. Returns true if we wrote
/// data (caller signals success); false means refuse.
fn serve_target(
    conn: &RustConnection,
    _window: u32,
    atoms: &Atoms,
    staged: &StagedImage,
    requestor: u32,
    property: u32,
    target: u32,
) -> Result<bool> {
    if target == atoms.targets {
        // Respond with the list of supported targets.
        let mut supported = vec![atoms.targets, atoms.timestamp];
        // Always advertise BOTH image atoms even if the staged image is
        // a single MIME. We hand back the staged bytes regardless of which
        // one the requestor asks for — pasting tools that hardcode PNG
        // shouldn't get an empty clipboard.
        supported.push(atoms.image_png);
        supported.push(atoms.image_jpeg);
        conn.change_property32(
            PropMode::REPLACE,
            requestor,
            property,
            AtomEnum::ATOM.into(),
            &supported,
        )?;
        return Ok(true);
    }
    if target == atoms.timestamp {
        // ICCCM lets us return CurrentTime (0) for the selection's
        // acquisition time. Real X clipboards return the server timestamp
        // captured when ownership was claimed, but XWayland's mirror is
        // tolerant of CurrentTime.
        conn.change_property32(
            PropMode::REPLACE,
            requestor,
            property,
            AtomEnum::INTEGER.into(),
            &[0u32],
        )?;
        return Ok(true);
    }
    if target == atoms.image_png || target == atoms.image_jpeg {
        // Send the staged bytes as 8-bit data on the requestor's property.
        // For images of 100KB-2MB we don't need INCR — X11 happily ships
        // up to ~1MB per ChangeProperty under XCB. We split anyway to
        // stay below the maximum-request-size ceiling.
        change_property_chunked(conn, requestor, property, target, &staged.bytes)?;
        return Ok(true);
    }
    if target == atoms.multiple {
        // MULTIPLE: the requestor wrote a list of (target, property) atom
        // pairs to its property; we walk them and serve each.
        let reply = conn
            // long_length is in 4-byte units; 65536 = 256KB worth of atom pairs,
            // far more than any sane requestor will use.
            .get_property(false, requestor, property, atoms.atom_pair, 0, 65_536)?
            .reply()?;
        if reply.format != 32 {
            return Ok(false);
        }
        let atoms_payload = reply.value32().ok_or_else(|| {
            anyhow::anyhow!("MULTIPLE property had wrong format")
        })?;
        let pairs: Vec<u32> = atoms_payload.collect();
        let mut any_ok = false;
        for chunk in pairs.chunks(2) {
            if chunk.len() != 2 {
                continue;
            }
            let sub_target = chunk[0];
            let sub_property = chunk[1];
            let ok = serve_target(conn, _window, atoms, staged, requestor, sub_property, sub_target)
                .unwrap_or(false);
            any_ok = any_ok || ok;
        }
        return Ok(any_ok);
    }
    debug!(target, "X11: refusing unknown target");
    Ok(false)
}

/// Split a large byte slice into ChangeProperty(APPEND) calls so we stay
/// well under the X server's max-request-size. For PNG/JPEG screenshots in
/// the 100KB-2MB range this is typically 1-4 chunks.
fn change_property_chunked(
    conn: &RustConnection,
    window: u32,
    property: u32,
    target: u32,
    data: &[u8],
) -> Result<()> {
    // 256KB is comfortably below any X server's max-request-size minus
    // the request header.
    const CHUNK: usize = 256 * 1024;
    let mut mode = PropMode::REPLACE;
    if data.is_empty() {
        conn.change_property8(mode, window, property, target, data)?;
        return Ok(());
    }
    for chunk in data.chunks(CHUNK) {
        conn.change_property8(mode, window, property, target, chunk)?;
        mode = PropMode::APPEND;
    }
    Ok(())
}
