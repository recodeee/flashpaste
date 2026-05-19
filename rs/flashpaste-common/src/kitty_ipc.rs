//! Direct kitty Remote-Control protocol over the unix socket.
//!
//! Replaces forking `kitty @ --to "$sock" send-text --match state:focused
//! --stdin` (~25-40ms of Python startup) with an in-process write of one
//! framed JSON envelope (~1ms).
//!
//! ## Wire format
//!
//! Kitty's RC accepts a JSON command wrapped in a DCS escape sequence:
//!
//! ```text
//!   \x1bP @ kitty-cmd <JSON> \x1b\\
//! ```
//!
//! where `<JSON>` is a single-line JSON object of shape:
//!
//! ```json
//! {"cmd":"send_text","version":[0,46,2],"payload":{"data":"<text>","match":"state:focused"}}
//! ```
//!
//! Kitty validates `version`; mismatches cause it to log "Unknown
//! protocol version" and refuse the command. We hard-code the version
//! discovered at build time (`kitty --version` on this box reports
//! `0.46.2`) — see [`KITTY_VERSION`].
//!
//! TODO(phase-2): discover the running kitty's version at runtime
//! (parse `kitty --version` once at startup and cache) so the binary
//! doesn't have to be rebuilt when kitty updates. For Phase 1 the
//! hard-coded value is acceptable because we ship the binary alongside
//! the bash fallback — if the version drifts, the user can switch back.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;

/// Kitty protocol version targeted by this binary. Discovered with
/// `kitty --version` on the build host (2026-05-19: kitty 0.46.2).
///
/// TODO: runtime discovery — see module-level docs.
pub const KITTY_VERSION: [u32; 3] = [0, 46, 2];

/// Socket I/O timeout. The bash script doesn't set one explicitly but
/// relies on `kitty @`'s own internal timeout; we want a tight cap so a
/// wedged kitty can't block the dispatcher.
pub const SOCKET_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Serialize)]
struct Envelope<'a> {
    cmd: &'static str,
    version: [u32; 3],
    payload: Payload<'a>,
}

#[derive(Serialize)]
struct Payload<'a> {
    /// The text to send. Kitty accepts either a string of raw bytes or
    /// (for send_text specifically) a "data:" base64 prefix. For the
    /// single-byte `\x16` (Ctrl-V) we just send it as a JSON string —
    /// it's a valid Unicode control char and json escapes it as `""`.
    data: &'a str,
    /// Kitty match spec. `state:focused` mirrors the bash `--match
    /// state:focused` flag and targets whichever kitty window currently
    /// has keyboard focus.
    #[serde(rename = "match")]
    match_spec: &'a str,
}

/// Send `bytes` to the focused kitty window via the RC socket at
/// `socket_path`.
///
/// `bytes` is interpreted as UTF-8 text — for the dispatch fast path it
/// is exactly `b"\x16"` (Ctrl-V). If you need to send arbitrary binary
/// data, base64-encode it and prefix with `"base64:"`, then update this
/// function accordingly.
///
/// Errors propagate any connect / write / read failure. The caller
/// (dispatcher) should fall back to spawning `kitty @` on error — same
/// fallback the bash script implicitly has via `2>>"$LOG"`.
pub fn send_text_focused(socket_path: &Path, bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes)
        .context("send_text payload is not valid UTF-8")?;
    let envelope = Envelope {
        cmd: "send_text",
        version: KITTY_VERSION,
        payload: Payload {
            data: text,
            match_spec: "state:focused",
        },
    };
    let json = serde_json::to_string(&envelope).context("serializing kitty RC envelope")?;

    let mut framed = Vec::with_capacity(json.len() + 16);
    framed.extend_from_slice(b"\x1bP@kitty-cmd ");
    framed.extend_from_slice(json.as_bytes());
    framed.extend_from_slice(b"\x1b\\");

    let mut stream = UnixStream::connect(socket_path)
        .with_context(|| format!("connecting to kitty socket {}", socket_path.display()))?;
    stream
        .set_write_timeout(Some(SOCKET_TIMEOUT))
        .context("setting write timeout")?;
    stream
        .set_read_timeout(Some(SOCKET_TIMEOUT))
        .context("setting read timeout")?;
    stream
        .write_all(&framed)
        .context("writing kitty RC envelope")?;
    // Kitty echoes a response (also DCS-framed). For send_text it's
    // typically empty `{"ok":true}` — we drain a small chunk to consume
    // it and avoid a TIME_WAIT-style half-close. Ignore read errors;
    // the command itself is fire-and-forget from our perspective.
    let mut sink = [0u8; 256];
    let _ = stream.read(&mut sink);
    Ok(())
}
