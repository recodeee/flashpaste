//! `flashpaste-shoot` — Phase 3 of the flashpaste plan.
//!
//! A tiny Rust binary that takes a screenshot via the XDG Desktop Portal
//! (`ashpd`) and pushes the resulting PNG into the flashpaste daemon's
//! stage socket. If the daemon isn't running, the PNG still lands in
//! `~/Pictures/Screenshots/` so the existing `.path` auto-pickup pipeline
//! (`bin/tmux-paste-dispatch.sh`, `bin/flashpaste-screenshot-preload.sh`)
//! takes over transparently.
//!
//! Target latency: capture-to-ready ~250ms — replaces the 3-second /
//! 3-4-click GNOME Screenshot UI flow.
//!
//! ## Why portal, not gnome-shell DBus
//!
//! `org.gnome.Shell.Screenshot` works today but is GNOME-specific and is
//! being phased out of mutter. The XDG portal (`org.freedesktop.portal.
//! Screenshot`) is the supported compositor-agnostic API — KDE, GNOME,
//! wlroots compositors all implement it. `ashpd` is the official Rust
//! wrapper.
//!
//! ## Flow (see plan section "Flow")
//!
//! 1. Parse args, init tracing.
//! 2. Open `Screenshot` portal request, await URI.
//! 3. Decode the `file://` URI into a path.
//! 4. Determine output path (--output, else `~/Pictures/Screenshots/...`).
//! 5. Stream the source bytes to the output path. fsync.
//! 6. If --no-daemon is not set, try a 50ms connect to the daemon socket
//!    and send a length-prefixed JSON `{op:stage, mime, path}` message.
//! 7. If --print-path, write the final path to stdout.

use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use ashpd::desktop::screenshot::Screenshot;
use clap::Parser;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::time::timeout;
use tracing::{debug, info, warn};

// ─────────────────────────────────────────────────────────────────────────
// CLI
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(
    name = "flashpaste-shoot",
    about = "Fast XDG-portal screenshot → flashpaste daemon (or PNG file)",
    long_about = None,
)]
struct Cli {
    /// Open the portal's interactive area-picker (default: full-screen).
    #[arg(long)]
    interactive: bool,

    /// Skip the daemon-stage attempt; save to ~/Pictures/Screenshots/ only.
    #[arg(long)]
    no_daemon: bool,

    /// Save the PNG to this path instead of ~/Pictures/Screenshots/.
    /// Daemon staging is still attempted unless --no-daemon is set.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,

    /// Print the final PNG path to stdout (for shell composition).
    #[arg(long)]
    print_path: bool,

    /// Portal request timeout, in milliseconds.
    #[arg(long, default_value_t = 5_000, value_name = "N")]
    timeout_ms: u64,

    /// Enable info-level tracing (sets RUST_LOG=info unless already set).
    #[arg(short, long)]
    verbose: bool,
}

// ─────────────────────────────────────────────────────────────────────────
// Daemon wire format
// ─────────────────────────────────────────────────────────────────────────
//
// The Phase 2 daemon expects, on `$XDG_RUNTIME_DIR/flashpaste.sock`:
//
//   4-byte little-endian u32 length  ||  that many bytes of JSON
//
// We send the `path` form (daemon reads file from disk) — avoids the
// base64 round-trip for large screenshots. Shape:
//
//   {"op":"stage","mime":"image/png","path":"/absolute/path/to/file.png"}

#[derive(Debug, Serialize)]
struct StageMsg<'a> {
    op: &'static str,
    mime: &'a str,
    path: &'a str,
}

const DAEMON_CONNECT_TIMEOUT: Duration = Duration::from_millis(50);
const DAEMON_WRITE_TIMEOUT: Duration = Duration::from_millis(200);

// ─────────────────────────────────────────────────────────────────────────
// Mime detection (header sniff — don't trust the filename)
// ─────────────────────────────────────────────────────────────────────────

fn sniff_mime(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        "image/png"
    } else if bytes.len() >= 3 && &bytes[..3] == b"\xff\xd8\xff" {
        "image/jpeg"
    } else {
        // Fall back to PNG; the portal almost always returns PNG and the
        // downstream auto-pickup pipeline is PNG-first. Worst case the
        // daemon / xclip rejects it, which is no worse than silently
        // lying.
        "image/png"
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Path helpers (duplicated from flashpaste-common per the no-dep constraint)
// ─────────────────────────────────────────────────────────────────────────

fn xdg_runtime_dir() -> PathBuf {
    if let Ok(dir) = env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let uid = nix::unistd::Uid::current().as_raw();
    let candidate = PathBuf::from(format!("/run/user/{uid}"));
    if candidate.is_dir() {
        return candidate;
    }
    PathBuf::from("/tmp")
}

fn daemon_socket_path() -> PathBuf {
    xdg_runtime_dir().join("flashpaste.sock")
}

fn screenshots_dir() -> Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME not set"))?;
    Ok(PathBuf::from(home).join("Pictures").join("Screenshots"))
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ─────────────────────────────────────────────────────────────────────────
// URI → path
// ─────────────────────────────────────────────────────────────────────────
//
// The portal returns a URI. In every implementation we've seen it's a
// `file://` URI pointing at a temp file under `/run/user/<uid>/`, but
// the spec allows any URI. We only support `file://` — anything else we
// surface as an error so the user knows to debug.
//
// Doing this by hand (rather than pulling in `url`) keeps the binary lean.

fn file_uri_to_path(uri: &str) -> Result<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| anyhow!("portal returned non-file URI: {uri}"))?;
    // Strip the optional authority component: `file://localhost/...` or
    // `file:///...`. Both are valid; we want the path part.
    let path_part = match rest.find('/') {
        Some(idx) => &rest[idx..],
        None => rest, // shouldn't happen for absolute paths, but be safe
    };
    // Minimal percent-decoding for the common cases (%20 → space).
    Ok(PathBuf::from(percent_decode(path_part)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (
                hex_val(bytes[i + 1]),
                hex_val(bytes[i + 2]),
            ) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| {
        // Should be valid UTF-8 for any real-world filesystem path.
        String::from_utf8_lossy(e.as_bytes()).into_owned()
    })
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Copy source → output with fsync (streamed, not slurped)
// ─────────────────────────────────────────────────────────────────────────
//
// We stream rather than `fs::read` + `fs::write` so a 30MB 4K-multimon
// screenshot doesn't sit in RAM longer than necessary. The trade-off:
// we also need to sniff the mime, which requires the first 8 bytes. We
// solve that by reading the first 8 bytes from the source, sniffing, then
// streaming the remainder.
//
// The final file is fsync'd so the daemon (or the `.path` watcher) sees
// the bytes on-disk before we tell it the path.

fn copy_with_sniff(src: &Path, dst: &Path) -> Result<&'static str> {
    if let Some(parent) = dst.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create_dir_all({})", parent.display()))?;
        }
    }

    let mut src_f =
        fs::File::open(src).with_context(|| format!("open source {}", src.display()))?;

    let mut head = [0u8; 8];
    let n = read_full(&mut src_f, &mut head)?;
    let mime = sniff_mime(&head[..n]);

    let mut dst_f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(dst)
        .with_context(|| format!("open dest {}", dst.display()))?;
    dst_f
        .write_all(&head[..n])
        .with_context(|| format!("write header to {}", dst.display()))?;

    io::copy(&mut src_f, &mut dst_f)
        .with_context(|| format!("copy body to {}", dst.display()))?;
    dst_f
        .sync_all()
        .with_context(|| format!("fsync {}", dst.display()))?;
    Ok(mime)
}

/// Read up to `buf.len()` bytes, returning how many were filled. EOF is
/// not an error — the source file may be smaller than our header window.
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match r.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(filled)
}

// ─────────────────────────────────────────────────────────────────────────
// Daemon stage (best-effort)
// ─────────────────────────────────────────────────────────────────────────

async fn try_stage_to_daemon(path: &Path, mime: &str) -> Result<()> {
    let sock = daemon_socket_path();
    debug!(socket = %sock.display(), "attempting daemon connect");

    let stream = timeout(DAEMON_CONNECT_TIMEOUT, UnixStream::connect(&sock))
        .await
        .map_err(|_| anyhow!("daemon connect timed out after {:?}", DAEMON_CONNECT_TIMEOUT))?
        .with_context(|| format!("connect to {}", sock.display()))?;

    let msg = StageMsg {
        op: "stage",
        mime,
        path: path.to_str().ok_or_else(|| anyhow!("path is not utf-8"))?,
    };
    let json = serde_json::to_vec(&msg)?;
    let len: u32 = json
        .len()
        .try_into()
        .map_err(|_| anyhow!("stage message too large for u32 frame"))?;

    let mut framed = Vec::with_capacity(4 + json.len());
    framed.extend_from_slice(&len.to_le_bytes());
    framed.extend_from_slice(&json);

    let mut stream = stream;
    timeout(DAEMON_WRITE_TIMEOUT, async {
        stream.write_all(&framed).await?;
        stream.flush().await?;
        stream.shutdown().await?;
        Ok::<_, io::Error>(())
    })
    .await
    .map_err(|_| anyhow!("daemon write timed out after {:?}", DAEMON_WRITE_TIMEOUT))?
    .context("write stage frame to daemon")?;

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────
// Tracing
// ─────────────────────────────────────────────────────────────────────────

fn init_tracing(verbose: bool) {
    use tracing_subscriber::EnvFilter;
    let filter = if verbose && env::var("RUST_LOG").is_err() {
        EnvFilter::new("info")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .try_init();
}

// ─────────────────────────────────────────────────────────────────────────
// Portal capture
// ─────────────────────────────────────────────────────────────────────────

async fn take_portal_screenshot(interactive: bool) -> Result<PathBuf> {
    let response = Screenshot::request()
        .interactive(interactive)
        .modal(false)
        .send()
        .await
        .context("send screenshot portal request")?
        .response()
        .context("read screenshot portal response")?;
    let uri = response.uri();
    debug!(uri = %uri, "portal returned screenshot URI");
    file_uri_to_path(uri.as_str())
}

// ─────────────────────────────────────────────────────────────────────────
// main
// ─────────────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    // 1. Portal capture (with timeout).
    let portal_path = timeout(
        Duration::from_millis(cli.timeout_ms),
        take_portal_screenshot(cli.interactive),
    )
    .await
    .map_err(|_| anyhow!("portal screenshot timed out after {} ms", cli.timeout_ms))??;
    info!(src = %portal_path.display(), "portal screenshot captured");

    // 2. Determine output path.
    let output_path: PathBuf = match cli.output {
        Some(p) => p,
        None => {
            let dir = screenshots_dir()?;
            fs::create_dir_all(&dir)
                .with_context(|| format!("create_dir_all({})", dir.display()))?;
            dir.join(format!("flashpaste-shoot-{}.png", unix_secs()))
        }
    };

    // 3. Stream bytes from portal temp file to output path. Sniff mime
    //    while we're at it.
    let mime = copy_with_sniff(&portal_path, &output_path)
        .context("copy portal output to destination")?;
    info!(dst = %output_path.display(), mime = mime, "wrote screenshot");

    // 4. Best-effort daemon stage. The file is already on disk so even if
    //    this fails, auto-pickup in tmux-paste-dispatch.sh will find it.
    if !cli.no_daemon {
        match try_stage_to_daemon(&output_path, mime).await {
            Ok(()) => info!("daemon stage ok"),
            Err(e) => warn!("daemon stage failed ({e:#}); file on disk is the fallback"),
        }
    } else {
        debug!("--no-daemon set; skipping daemon stage");
    }

    // 5. Optional path emission for shell pipelines.
    if cli.print_path {
        println!("{}", output_path.display());
    }

    Ok(())
}
