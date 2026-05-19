//! Daemon-wide shared state and configuration.
//!
//! `SharedState` is what every subsystem (inotify, Wayland, X11, IPC, paste
//! dispatch) hangs off. The big-ticket item is `latest_image` — an
//! `RwLock<Option<StagedImage>>` that the inotify watcher writes and the
//! clipboard owners + paste dispatcher read.

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use tokio::sync::{watch, RwLock};

use crate::Args;

/// How fresh a staged image must be before the daemon will use it for a
/// paste op. Matches the bash dispatcher's 30s screenshot age gate but is
/// more generous (2 minutes) because once the daemon owns the clipboard the
/// image stays valid until something supersedes it.
pub const STAGED_IMAGE_TTL_SECS: u64 = 120;

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
    /// The currently staged image. `None` until inotify sees the first
    /// screenshot, then `Some` for the rest of the daemon's life.
    pub latest_image: RwLock<Option<StagedImage>>,
    /// Channel notified every time `latest_image` is updated. Used by the
    /// Wayland + X11 owners to refresh their selection content.
    pub stage_notifier_tx: watch::Sender<u64>,
    pub stage_notifier_rx: watch::Receiver<u64>,
    /// Recursion guard. Holds the unix-epoch millisecond timestamp of the
    /// last `paste` op the daemon handled. A new paste op within 1500ms is
    /// rejected as deduped (replies `{"ok":true,"deduped":true}`).
    pub last_paste_ms: AtomicU64,
}

impl SharedState {
    pub fn new(config: DaemonConfig, kitty_version: KittyVersion) -> Self {
        let (tx, rx) = watch::channel(0u64);
        Self {
            config,
            kitty_version,
            latest_image: RwLock::new(None),
            stage_notifier_tx: tx,
            stage_notifier_rx: rx,
            last_paste_ms: AtomicU64::new(0),
        }
    }

    /// Replace the staged image and notify subscribers.
    pub async fn set_staged_image(&self, image: StagedImage) {
        let revision = {
            let mut guard = self.latest_image.write().await;
            *guard = Some(image);
            now_unix_ms()
        };
        // Best-effort: ignore send errors (no subscribers is fine).
        let _ = self.stage_notifier_tx.send(revision);
    }

    /// Snapshot the staged image (without holding the lock during use).
    pub async fn staged_snapshot(&self) -> Option<StagedImage> {
        self.latest_image.read().await.clone()
    }

    /// Synchronous variant for subsystems that aren't tokio-aware (e.g. the
    /// X11 owner running on `spawn_blocking`). Uses `blocking_read` because
    /// we are guaranteed to be inside a blocking task.
    pub fn staged_snapshot_blocking(&self) -> Option<StagedImage> {
        self.latest_image.blocking_read().clone()
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
    // Single quotes inside double quotes inside the shell command — keep this
    // identical to the bash script so the user can A/B between daemon and
    // bash without resourcing tmux.conf.
    String::from(
        "tmux bind -n C-v run-shell -b \"TMUX_PASTE_TRIGGER=ctrl-v \
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
