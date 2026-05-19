//! Daemon-wide shared state and configuration.
//!
//! `SharedState` is what every subsystem (inotify, Wayland, X11, IPC, paste
//! dispatch) hangs off. The big-ticket item is `latest_image` — an
//! `RwLock<Option<StagedImage>>` that the inotify watcher writes and the
//! clipboard owners + paste dispatcher read.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use tokio::sync::{watch, RwLock};

use crate::Args;

/// How fresh a staged image must be before the daemon will use it for a
/// paste op. Bash dispatcher uses a 30s file-age gate; the daemon is more
/// generous because once we own the clipboard the image stays valid until
/// something supersedes it. 30 minutes covers the common AFK-then-paste
/// case (took a screenshot, switched away, came back) without the daemon
/// silently punting to bash and surprising the user.
pub const STAGED_IMAGE_TTL_SECS: u64 = 1800;

/// Text selections live as long as someone is owning the clipboard. Match
/// typical desktop UX: a copy is "fresh" for the rest of the session.
pub const STAGED_TEXT_TTL_SECS: u64 = 24 * 3600;

/// Bytes of a staged image we hold in memory. PNG/JPEG range is 100KB-2MB
/// typically; we keep the bytes so X11's SelectionRequest handler can serve
/// them on demand without re-reading the file.
#[derive(Debug, Clone)]
pub struct StagedImage {
    pub bytes: Arc<Vec<u8>>,
    pub mime: &'static str,
    pub path: PathBuf,
    /// Captured-at as a `SystemTime` so subsystems running on `spawn_blocking`
    /// (X11) can compute ages without needing a tokio handle.
    pub captured_at: SystemTime,
}

impl StagedImage {
    /// Returns true if the image is still inside the daemon's freshness window.
    pub fn is_fresh(&self) -> bool {
        match self.captured_at.elapsed() {
            Ok(age) => age.as_secs() <= STAGED_IMAGE_TTL_SECS,
            Err(_) => true, // clock skewed; treat as fresh rather than drop
        }
    }
}

/// Bytes of a staged TEXT selection. v1.19+ — the daemon now owns text on
/// the clipboard too, so `clipboard-set.sh` can route there instead of
/// forking `wl-copy` (which surfaced as a phantom dock entry on every
/// copy). Kept as raw bytes (not String) because tmux can pipe non-UTF8.
#[derive(Debug, Clone)]
pub struct StagedText {
    pub bytes: Arc<Vec<u8>>,
    pub captured_at: SystemTime,
}

impl StagedText {
    pub fn is_fresh(&self) -> bool {
        match self.captured_at.elapsed() {
            Ok(age) => age.as_secs() <= STAGED_TEXT_TTL_SECS,
            Err(_) => true,
        }
    }
}

/// What the daemon is currently advertising on the clipboard. At most one
/// variant is live at a time — copying text clobbers a staged image and
/// vice versa, mirroring how real system clipboards work.
#[derive(Debug, Clone)]
pub enum StagedSelection {
    Image(StagedImage),
    Text(StagedText),
}

impl StagedSelection {
    pub fn is_fresh(&self) -> bool {
        match self {
            Self::Image(img) => img.is_fresh(),
            Self::Text(txt) => txt.is_fresh(),
        }
    }
}

/// Cached kitty IPC protocol version. Populated once at startup; never on
/// the paste hot path.
#[derive(Debug, Clone, Copy)]
pub struct KittyVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl KittyVersion {
    /// The version we ship as fallback when `kitty --version` is missing or
    /// produces something we can't parse. 0.21.0 is the oldest kitty that
    /// implements `send_text` over IPC with the modern envelope; any newer
    /// kitty (we've verified on 0.46.2) accepts the same payload.
    pub const FALLBACK: KittyVersion = KittyVersion {
        major: 0,
        minor: 21,
        patch: 0,
    };
}

/// Static configuration resolved at startup.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    pub socket_path: PathBuf,
    pub screenshots_dir: Option<PathBuf>,
    pub kitty_version: KittyVersion,
    pub bash_fallback: PathBuf,
    pub tmux_rebind_command: String,
}

impl DaemonConfig {
    pub fn resolve(args: &Args) -> Result<Self> {
        let socket_path = args
            .socket
            .clone()
            .unwrap_or_else(default_socket_path);
        let screenshots_dir = args
            .screenshots_dir
            .clone()
            .or_else(default_screenshots_dir);
        let kitty_version = detect_kitty_version();
        let bash_fallback = PathBuf::from("/home/deadpool/.local/bin/tmux-paste-dispatch.sh");
        let tmux_rebind_command = default_tmux_rebind_command();

        Ok(Self {
            socket_path,
            screenshots_dir,
            kitty_version,
            bash_fallback,
            tmux_rebind_command,
        })
    }
}

/// Per-process shared state. Cheap to clone (everything is `Arc`).
pub struct SharedState {
    pub config: DaemonConfig,
    pub kitty_version: KittyVersion,
    /// The currently staged selection. `None` until something (a screenshot
    /// via inotify, or a text copy via the stage_text IPC) is staged.
    pub latest_selection: RwLock<Option<StagedSelection>>,
    /// Channel notified every time `latest_selection` is updated. Used by
    /// the Wayland + X11 owners to refresh their selection content.
    pub stage_notifier_tx: watch::Sender<u64>,
    pub stage_notifier_rx: watch::Receiver<u64>,
    /// Recursion guard. Holds the unix-epoch millisecond timestamp of the
    /// last `paste` op the daemon handled. A new paste op within 1500ms is
    /// rejected as deduped (replies `{"ok":true,"deduped":true}`).
    pub last_paste_ms: AtomicU64,
    /// True while a paste dispatch is in flight (including the
    /// `wait_for_pane_idle` hold). Subsequent paste requests during that
    /// window dedupe immediately instead of stacking — without this guard,
    /// 4–5 presses while Claude is mid-generation each spawn their own
    /// wait task and all dispatch \026 simultaneously when Claude becomes
    /// idle.
    pub paste_in_flight: AtomicBool,
}

impl SharedState {
    pub fn new(config: DaemonConfig, kitty_version: KittyVersion) -> Self {
        let (tx, rx) = watch::channel(0u64);
        Self {
            config,
            kitty_version,
            latest_selection: RwLock::new(None),
            stage_notifier_tx: tx,
            stage_notifier_rx: rx,
            last_paste_ms: AtomicU64::new(0),
            paste_in_flight: AtomicBool::new(false),
        }
    }

    /// Replace the staged image and notify subscribers.
    pub async fn set_staged_image(&self, image: StagedImage) {
        self.set_staged_selection(StagedSelection::Image(image)).await;
    }

    /// Replace the staged text and notify subscribers. v1.19+ — the path
    /// through `flashpaste-trigger --stage-text` that lets `clipboard-set.sh`
    /// avoid forking `wl-copy`.
    pub async fn set_staged_text(&self, text: StagedText) {
        self.set_staged_selection(StagedSelection::Text(text)).await;
    }

    /// Replace whatever's currently staged with `sel`. Either variant
    /// clobbers the other — clipboards are a single slot.
    pub async fn set_staged_selection(&self, sel: StagedSelection) {
        {
            let mut guard = self.latest_selection.write().await;
            *guard = Some(sel);
        }
        // Best-effort: ignore send errors (no subscribers is fine).
        let _ = self.stage_notifier_tx.send(now_unix_ms());
    }

    /// Snapshot the staged selection (without holding the lock during use).
    pub async fn staged_snapshot(&self) -> Option<StagedSelection> {
        self.latest_selection.read().await.clone()
    }

    /// Image-only convenience for the paste op (which is image-specific).
    /// Returns `None` if nothing is staged OR if the staged item is text.
    pub async fn staged_image(&self) -> Option<StagedImage> {
        match self.staged_snapshot().await {
            Some(StagedSelection::Image(img)) => Some(img),
            _ => None,
        }
    }

    /// Synchronous variant for subsystems that aren't tokio-aware (e.g. the
    /// X11 owner running on `spawn_blocking`). Uses `blocking_read` because
    /// we are guaranteed to be inside a blocking task.
    pub fn staged_snapshot_blocking(&self) -> Option<StagedSelection> {
        self.latest_selection.blocking_read().clone()
    }
}

fn default_socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("flashpaste.sock");
        }
    }
    let uid = nix::unistd::Uid::current().as_raw();
    let candidate = PathBuf::from(format!("/run/user/{uid}"));
    if candidate.is_dir() {
        return candidate.join("flashpaste.sock");
    }
    PathBuf::from("/tmp").join("flashpaste.sock")
}

fn default_screenshots_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Pictures").join("Screenshots"))
}

/// Mirrors the bash dispatcher's rebind line so a daemon-handled paste
/// leaves tmux in the same state as the legacy script's:
///   tmux bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v <trigger> '#{pane_id}'"
///
/// IMPORTANT: this points at the bash dispatcher, not the trigger binary,
/// to keep the daemon-failure recovery path obvious. If the daemon is
/// running, the FIRST paste sets the binding via this string AND the trigger
/// short-circuits anyway. If the user wants the rebind to point at
/// `flashpaste-trigger` instead, they edit `examples/tmux.conf.snippet`.
fn default_tmux_rebind_command() -> String {
    // Match the user's tmux.conf binding exactly: prefer the daemon trigger
    // and fall back to the bash dispatcher via `||`. Before this, the
    // default rebound to bash-only after every paste, silently demoting
    // Tier 3 → Tier 1 for the rest of the tmux session (until the user
    // re-sourced ~/.tmux.conf).
    String::from(
        "tmux bind -n C-v run-shell -b \"TMUX_PASTE_TRIGGER=ctrl-v \
         flashpaste-trigger '#{pane_id}' 2>/dev/null || \
         TMUX_PASTE_TRIGGER=ctrl-v \
         /home/deadpool/.local/bin/tmux-paste-dispatch.sh '#{pane_id}'\"",
    )
}

/// Detect the kitty IPC protocol version at startup. We run `kitty --version`
/// ONCE. This is process-startup, not paste hot-path, so the cost is free.
fn detect_kitty_version() -> KittyVersion {
    let output = std::process::Command::new("kitty")
        .arg("--version")
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout);
            parse_kitty_version(&s).unwrap_or(KittyVersion::FALLBACK)
        }
        _ => KittyVersion::FALLBACK,
    }
}

fn parse_kitty_version(s: &str) -> Option<KittyVersion> {
    // `kitty 0.46.2 created by Kovid Goyal` — second whitespace-delimited
    // token is the dotted version.
    let token = s.split_whitespace().nth(1)?;
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some(KittyVersion {
        major,
        minor,
        patch,
    })
}

pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_kitty_output() {
        let v = parse_kitty_version("kitty 0.46.2 created by Kovid Goyal").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 46);
        assert_eq!(v.patch, 2);
    }

    #[test]
    fn parses_two_segment_version() {
        let v = parse_kitty_version("kitty 0.42 something else").unwrap();
        assert_eq!(v.minor, 42);
        assert_eq!(v.patch, 0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_kitty_version("").is_none());
        assert!(parse_kitty_version("not a version").is_none());
    }
}
