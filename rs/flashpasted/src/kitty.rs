//! Kitty IPC over the unix socket — wire protocol, no shell-out.
//!
//! Fact #1 from the spec: `kitty @ send-text` is the only transport that
//! triggers Claude Code's image-paste handler. The bash dispatcher achieves
//! this by literally execing `kitty @ ... send-text` (~3-5ms fork-exec
//! cost). We speak the wire protocol directly to save that cost on the hot
//! path.
//!
//! Wire format (per kitty source — `kitty/remote.py`'s `RemoteCommand`):
//!
//!   <ESC>P@kitty-cmd JSON-PAYLOAD<ESC>\
//!
//! Where ESC is 0x1b. The DCS envelope (`<ESC>P` ... `<ESC>\`) is wrapped
//! around the JSON. The JSON payload looks like:
//!
//!   {
//!     "cmd": "send_text",
//!     "version": [MAJOR, MINOR, PATCH],
//!     "payload": {
//!       "data": "<base64 bytes>",
//!       "match": "state:focused"
//!     }
//!   }
//!
//! The `data` field is the bytes we want sent into the focused kitty
//! window's child process (the tmux server in our case). For our use we
//! always send a single `\x1b\x16` no — actually just `\x16` (Ctrl-V).
//! kitty base64-encodes the payload itself starting in some versions, but
//! the JSON `data` field is plain text in versions ≥0.21 (we send `""`).
//!
//! NOTE: kitty changed its IPC encoding several times across versions; the
//! `version` field in the envelope lets the receiver decide which decoder
//! to use. Sending the daemon's detected `kitty --version` (cached at
//! startup) means we don't have to maintain a compatibility matrix here.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::state::KittyVersion;

/// Kitty's DCS envelope opener: ESC P @kitty-cmd<space>
const ENVELOPE_OPEN: &[u8] = b"\x1bP@kitty-cmd ";
/// Kitty's ST (string terminator): ESC \
const ENVELOPE_CLOSE: &[u8] = b"\x1b\\";

/// The byte we send: Ctrl-V (0x16). When kitty's `send_text` injects it
/// into the focused window's pty, Claude Code's TUI catches the keystroke
/// and fires its `wl-paste -t image/png` reader.
pub const CTRL_V: &str = "\u{0016}";

/// Send a single Ctrl-V byte into the focused kitty window. `socket` is the
/// kitty IPC socket discovered at startup (e.g. `/run/user/1000/kitty-main-...`).
pub async fn send_ctrl_v(socket: &Path, version: KittyVersion) -> Result<()> {
    let payload = json!({
        "cmd": "send_text",
        "version": [version.major, version.minor, version.patch],
        "payload": {
            "data": CTRL_V,
            "match": "state:focused",
        },
    });
    let json_bytes = serde_json::to_vec(&payload)?;

    let mut framed = Vec::with_capacity(ENVELOPE_OPEN.len() + json_bytes.len() + ENVELOPE_CLOSE.len());
    framed.extend_from_slice(ENVELOPE_OPEN);
    framed.extend_from_slice(&json_bytes);
    framed.extend_from_slice(ENVELOPE_CLOSE);

    // Connect + write with a tight budget. Kitty's IPC is on a unix socket
    // so this is microseconds in practice; the timeout exists to guarantee
    // forward progress if kitty has wedged.
    let mut stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connect kitty IPC at {}", socket.display()))?;
    let write_fut = async {
        stream.write_all(&framed).await?;
        stream.flush().await?;
        // We DO NOT read kitty's response. send_text fires-and-forgets:
        // kitty acks via the same DCS stream, but reading the ack costs us
        // ~5ms while the paste is already in flight to tmux. The bash
        // dispatcher also ignores the ack.
        Ok::<_, std::io::Error>(())
    };
    timeout(Duration::from_millis(30), write_fut)
        .await
        .context("kitty IPC write timeout")?
        .context("kitty IPC write")?;
    Ok(())
}

/// Glob `$XDG_RUNTIME_DIR/kitty-main-*` and pick the first socket. Same
/// algorithm as the bash dispatcher (`for sock_path in /run/user/$(id -u)/kitty-main-*`).
pub fn find_kitty_socket(xdg_runtime_dir: &Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(xdg_runtime_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        if !name_str.starts_with("kitty-main-") {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        use std::os::unix::fs::FileTypeExt;
        if ft.is_socket() {
            return Some(entry.path());
        }
    }
    None
}
