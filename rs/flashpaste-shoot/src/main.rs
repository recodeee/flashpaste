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
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
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

    /// Run OCR on the captured image via `tesseract`. With `--print-path`
    /// the output is `<path>\n\n<ocr text>`; without it, just the text.
    /// If `tesseract` isn't installed we log a warning and skip OCR —
    /// the capture itself still succeeds.
    #[arg(long)]
    ocr: bool,

    /// Skip file save: just run OCR on the latest screenshot in
    /// `~/Pictures/Screenshots/` (within 60 s) and print the text. Useful
    /// for `flashpaste-shoot --ocr-only | pbcopy`-style pipelines.
    /// Mutually-exclusive with the capture path; if set, no portal
    /// request is made.
    #[arg(long)]
    ocr_only: bool,

    /// Hand the captured image to an annotation editor before returning
    /// (arrows / highlights / blur). Tries `swappy`, then `satty`. If
    /// neither is installed we log a warning and keep the original file.
    #[arg(long)]
    annotate: bool,

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
// OCR + annotate hooks (process-level integrations)
// ─────────────────────────────────────────────────────────────────────────
//
// Both features are implemented as opt-in shellouts to standard tooling
// rather than baked-in libraries. Rationale:
//
//   * `tesseract` is the de-facto Linux OCR engine; statically linking
//     a Rust alternative (e.g. ocrs) would balloon binary size for a
//     feature most users won't enable.
//   * `swappy` / `satty` already implement the annotation UX users
//     expect from screenshot tools (arrows, blur, text overlays). No
//     point reimplementing that in this binary.
//
// Both call sites are GRACEFUL: missing tool → stderr warning + skip,
// never a hard failure. The capture itself is the contract — these are
// post-processors.

/// `which`-style PATH lookup. Returns true if `bin` is on PATH and is
/// executable. We don't shell out — that's another ~5 ms fork + a
/// dependency on `command -v` being available.
fn is_on_path(bin: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    for dir in env::split_paths(&path) {
        let candidate = dir.join(bin);
        // Cheaper than `metadata().is_file()` — we accept symlinks too,
        // and we don't actually care about the perm bits because Linux
        // checks them at exec time.
        if candidate.is_file() {
            return true;
        }
    }
    false
}

/// Run `tesseract <path> -` (the `-` writes to stdout). On success
/// returns the OCR text trimmed of trailing whitespace. Returns `None`
/// if tesseract isn't on PATH — the caller emits the warning. Any
/// other error (tesseract present but unhappy) is bubbled up so we can
/// log the stderr.
fn run_tesseract(path: &Path) -> Result<Option<String>> {
    if !is_on_path("tesseract") {
        return Ok(None);
    }
    let output = Command::new("tesseract")
        .arg(path.as_os_str())
        .arg("-")
        // -l eng is implicit; we deliberately don't pin it so the user's
        // installed language packs are honoured.
        .output()
        .context("spawn tesseract")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "tesseract exited {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(Some(text.trim_end().to_string()))
}

/// Find the newest image in `~/Pictures/Screenshots/` whose mtime is
/// within `max_age_secs` of now. Mirrors the in-tree helper in
/// `flashpaste-common::screenshot::find_latest` but we duplicate it
/// inline (this crate has no `flashpaste-common` dep — see the header
/// comment on the URI parser).
fn find_latest_screenshot(max_age_secs: u64) -> Result<Option<PathBuf>> {
    let dir = screenshots_dir()?;
    if !dir.is_dir() {
        return Ok(None);
    }
    let entries = fs::read_dir(&dir)
        .with_context(|| format!("read_dir({})", dir.display()))?;
    let now = SystemTime::now();
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_file() {
            continue;
        }
        let p = entry.path();
        if !is_image_ext(p.extension()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        match &best {
            None => best = Some((mtime, p)),
            Some((bm, _)) if mtime > *bm => best = Some((mtime, p)),
            _ => {}
        }
    }
    let Some((mtime, p)) = best else { return Ok(None) };
    if let Ok(age) = now.duration_since(mtime) {
        if age.as_secs() > max_age_secs {
            return Ok(None);
        }
    }
    Ok(Some(p))
}

fn is_image_ext(ext: Option<&OsStr>) -> bool {
    let Some(ext) = ext.and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg")
}

/// Run an annotation editor on `path`, writing back to the same file.
/// Tries `swappy -f <path> -o <path>` first (the GNOME-native UX), then
/// `satty --filename <path> --output-filename <path>`. If neither is
/// installed, prints a warning and returns Ok — the original file is
/// untouched, matching the documented behaviour ("--annotate works
/// without them but degrades").
///
/// Blocks until the editor exits so the caller can hand the path off
/// to the daemon stage / `--print-path` path with confidence the bytes
/// reflect the user's edits.
fn run_annotate(path: &Path) -> Result<()> {
    if is_on_path("swappy") {
        info!(tool = "swappy", path = %path.display(), "launching annotation editor");
        let status = Command::new("swappy")
            .arg("-f")
            .arg(path.as_os_str())
            .arg("-o")
            .arg(path.as_os_str())
            .status()
            .context("spawn swappy")?;
        if !status.success() {
            // Don't fail hard — user might have closed the editor without
            // saving. The file is whatever swappy left behind (usually
            // the original).
            warn!(?status, "swappy exited non-zero; keeping current file bytes");
        }
        return Ok(());
    }
    if is_on_path("satty") {
        info!(tool = "satty", path = %path.display(), "launching annotation editor");
        let status = Command::new("satty")
            .arg("--filename")
            .arg(path.as_os_str())
            .arg("--output-filename")
            .arg(path.as_os_str())
            .status()
            .context("spawn satty")?;
        if !status.success() {
            warn!(?status, "satty exited non-zero; keeping current file bytes");
        }
        return Ok(());
    }
    eprintln!(
        "flashpaste-shoot: --annotate requested but neither `swappy` nor `satty` is on PATH; \
         skipping annotation. Install one with: `apt install swappy` (or `cargo install satty`)."
    );
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

    // ─── --ocr-only short-circuit ───────────────────────────────────────
    // Skip the portal entirely; just OCR the most recent screenshot.
    if cli.ocr_only {
        return run_ocr_only_path().await;
    }

    // Per-phase timing. Emitted as a single SHOT structured line at the
    // end so the user can see exactly where the screenshot-to-ready time
    // is going. User asked (2026-05-19): "can we measure the speed with
    // a cli logs?" — so we always emit the summary, not gated on -v.
    let t_start = std::time::Instant::now();
    let mut t_phase = t_start;
    let mut take_phase = || -> u64 {
        let now = std::time::Instant::now();
        let ms = now.duration_since(t_phase).as_millis() as u64;
        t_phase = now;
        ms
    };

    // 1. Portal capture (with timeout).
    let portal_path = timeout(
        Duration::from_millis(cli.timeout_ms),
        take_portal_screenshot(cli.interactive),
    )
    .await
    .map_err(|_| anyhow!("portal screenshot timed out after {} ms", cli.timeout_ms))??;
    let ms_portal = take_phase();
    info!(src = %portal_path.display(), ms_portal, "portal screenshot captured");

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
    let ms_outpath = take_phase();

    // 3. Stream bytes from portal temp file to output path. Sniff mime
    //    while we're at it.
    let mime = copy_with_sniff(&portal_path, &output_path)
        .context("copy portal output to destination")?;
    let ms_write = take_phase();
    info!(dst = %output_path.display(), mime = mime, ms_write, "wrote screenshot");

    // 4. Optional annotation pass. Runs BEFORE daemon stage so the
    //    daemon sees the annotated bytes, not the raw capture. Blocking
    //    by design — the user is in the editor and we want the final
    //    file before we tell anyone about it.
    let ms_annotate = if cli.annotate {
        if let Err(e) = run_annotate(&output_path) {
            warn!("annotate failed ({e:#}); proceeding with raw capture");
        }
        take_phase()
    } else {
        take_phase()
    };

    // 5. Best-effort daemon stage. The file is already on disk so even if
    //    this fails, auto-pickup in tmux-paste-dispatch.sh will find it.
    let ms_daemon = if !cli.no_daemon {
        match try_stage_to_daemon(&output_path, mime).await {
            Ok(()) => info!("daemon stage ok"),
            Err(e) => warn!("daemon stage failed ({e:#}); file on disk is the fallback"),
        }
        take_phase()
    } else {
        debug!("--no-daemon set; skipping daemon stage");
        take_phase()
    };

    let ms_total = t_start.elapsed().as_millis() as u64;
    // Single SHOT summary line. Goes to stderr (info!) so stdout stays
    // clean for --print-path. Read with `journalctl --user --since="1
    // minute ago" | grep SHOT` if running via systemd-launched keybind.
    info!(
        path = %output_path.display(),
        ms_portal,
        ms_outpath,
        ms_write,
        ms_annotate,
        ms_daemon,
        ms_total,
        "SHOT"
    );

    // 6. Optional OCR. Done AFTER daemon stage so we don't delay the
    //    clipboard becoming usable — OCR can take a few hundred ms even
    //    on modern hardware.
    let ocr_text = if cli.ocr {
        match run_tesseract(&output_path) {
            Ok(Some(text)) => Some(text),
            Ok(None) => {
                eprintln!(
                    "flashpaste-shoot: --ocr requested but `tesseract` isn't on PATH; \
                     skipping OCR. Install with: `apt install tesseract-ocr`."
                );
                None
            }
            Err(e) => {
                warn!("OCR failed: {e:#}");
                None
            }
        }
    } else {
        None
    };

    // 7. Output emission. Combinations:
    //    --print-path             → path
    //    --ocr                    → ocr text
    //    --print-path --ocr       → path\n\nocr text
    match (cli.print_path, ocr_text) {
        (true, Some(text)) => {
            println!("{}", output_path.display());
            println!();
            println!("{text}");
        }
        (true, None) => println!("{}", output_path.display()),
        (false, Some(text)) => println!("{text}"),
        (false, None) => {}
    }

    Ok(())
}

/// `--ocr-only` flow: don't capture, just OCR the latest screenshot.
/// Defaults to a 60s freshness window so a stale screenshot from days
/// ago isn't surprising. Prints the OCR text on stdout; exits non-zero
/// if there's nothing to OCR or tesseract isn't installed (this flag
/// has a stronger contract than `--ocr`: the caller is piping us).
async fn run_ocr_only_path() -> Result<()> {
    let path = find_latest_screenshot(60)?.ok_or_else(|| {
        anyhow!(
            "no screenshot within 60s in {}; capture one first or use --ocr",
            screenshots_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|_| "~/Pictures/Screenshots".to_string())
        )
    })?;
    debug!(path = %path.display(), "running --ocr-only on latest screenshot");
    match run_tesseract(&path)? {
        Some(text) => {
            println!("{text}");
            Ok(())
        }
        None => Err(anyhow!(
            "tesseract is not on PATH; install with `apt install tesseract-ocr`"
        )),
    }
}
