//! flashpasted — long-lived clipboard owner + paste dispatcher.
//!
//! The four hard-won facts from `bin/tmux-paste-dispatch.sh` MUST be preserved:
//!
//! 1. `kitty @ send-text` is the only transport that triggers Claude Code's
//!    image-paste handler. We speak the kitty IPC wire protocol directly so
//!    we don't shell out — see `kitty.rs`.
//! 2. Tmux's `bind -n C-v` recurses when `\026` reaches tmux via kitty
//!    send-text. We `tmux unbind -n C-v` BEFORE send-text and schedule a
//!    detached `setsid`-style rebind ~100ms later — see `tmux.rs`.
//! 3. Wayland-authoritative `has_image` policy. The daemon OWNS the Wayland
//!    clipboard, so the question rarely arises — see `wayland.rs`. When we
//!    do need to read the user's pre-existing clipboard (Phase 3+), the
//!    helper in `wayland.rs::read_has_image` applies the same policy.
//! 4. GNOME PrtScr saves to `~/Pictures/Screenshots/` but does NOT copy.
//!    The inotify watcher in `inotify_watch.rs` notices `IN_CLOSE_WRITE`
//!    and auto-stages the bytes into both clipboard owners.

mod inotify_watch;
mod ipc;
mod kitty;
mod paste;
mod state;
mod tmux;
mod wayland;
mod x11;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::signal::unix::{signal, SignalKind};
use tracing::{error, info, warn};

use crate::state::{DaemonConfig, SharedState};

/// Command-line surface. Most knobs default to the production paths.
#[derive(Debug, Parser)]
#[command(
    name = "flashpasted",
    about = "flashpaste daemon: clipboard owner + paste dispatcher (Phase 2)"
)]
pub(crate) struct Args {
    /// Override the unix socket path. Default `$XDG_RUNTIME_DIR/flashpaste.sock`.
    #[arg(long)]
    pub(crate) socket: Option<PathBuf>,
    /// Override the screenshots dir to watch. Default `~/Pictures/Screenshots`.
    #[arg(long)]
    pub(crate) screenshots_dir: Option<PathBuf>,
    /// Disable inotify (useful for testing the IPC + dispatch path alone).
    #[arg(long)]
    pub(crate) no_inotify: bool,
    /// Disable the Wayland clipboard owner.
    #[arg(long)]
    pub(crate) no_wayland: bool,
    /// Disable the X11 clipboard owner.
    #[arg(long)]
    pub(crate) no_x11: bool,
}

fn main() -> Result<()> {
    init_tracing();

    let args = Args::parse();
    let cfg = DaemonConfig::resolve(&args)?;
    info!(
        socket = %cfg.socket_path.display(),
        screenshots_dir = ?cfg.screenshots_dir.as_ref().map(|p| p.display().to_string()),
        kitty_version = ?cfg.kitty_version,
        "flashpasted starting"
    );

    // Cached kitty version — looked up once at startup, never on the hot path.
    let kitty_version = cfg.kitty_version;

    let state = Arc::new(SharedState::new(cfg.clone(), kitty_version));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2) // we don't need many; most work is IO-bound
        .thread_name("flashpasted")
        .build()
        .context("tokio runtime")?;

    rt.block_on(async move {
        if let Err(e) = run(state, args).await {
            error!(error = ?e, "daemon exited with error");
            std::process::exit(1);
        }
    });
    // Bound the runtime drop. The x11, wayland, and inotify owners run on
    // `spawn_blocking` threads with infinite loops and no shutdown check —
    // a natural `rt` drop would wait on them forever. Without this, systemd
    // hangs the unit in `deactivating (stop-sigterm)` for the full 90s
    // TimeoutStopSec, then SIGKILLs. During that window the IPC listener is
    // aborted but the socket file still exists, so `flashpaste-trigger`
    // gets ECONNREFUSED on connect and the user sees paste as "broken"
    // after every restart. 500ms is plenty to flush in-flight work; the
    // blocking threads get torn down by process exit.
    rt.shutdown_timeout(std::time::Duration::from_millis(500));
    Ok(())
}

async fn run(state: Arc<SharedState>, args: Args) -> Result<()> {
    // 1) Bring up the IPC listener first — that way `flashpaste-trigger`
    //    invocations during startup get a connection refused instead of
    //    racing with half-initialized state.
    let ipc_handle = ipc::spawn_listener(state.clone()).await?;

    // 2) Wayland owner. Spawns its own background thread inside the crate.
    //    The wl-clipboard-rs `copy()` call is blocking and forks-internally,
    //    so we run our orchestration on a dedicated tokio task. If Mutter is
    //    wedged we log and continue with X11 only.
    if !args.no_wayland {
        wayland::spawn_owner(state.clone());
    } else {
        warn!("Wayland owner disabled by --no-wayland");
    }

    // 3) X11 owner — a single long-lived connection that holds the CLIPBOARD
    //    selection and serves SelectionRequest events on every refresh.
    if !args.no_x11 {
        x11::spawn_owner(state.clone());
    } else {
        warn!("X11 owner disabled by --no-x11");
    }

    // 4) Inotify watcher on the screenshots dir. spawn_blocking + the sync
    //    `inotify` crate; far simpler than `inotify-stream`.
    if !args.no_inotify {
        inotify_watch::spawn_watcher(state.clone());
    } else {
        warn!("inotify disabled by --no-inotify");
    }

    // 5) Wait for shutdown signal.
    let mut sigterm = signal(SignalKind::terminate()).context("SIGTERM handler")?;
    let mut sigint = signal(SignalKind::interrupt()).context("SIGINT handler")?;
    tokio::select! {
        _ = sigterm.recv() => info!("got SIGTERM, shutting down"),
        _ = sigint.recv() => info!("got SIGINT, shutting down"),
    }

    // IPC listener is the only handle we explicitly wait on; the other
    // subsystems are best-effort and will be torn down when the process
    // exits.
    ipc_handle.abort();
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,flashpasted=debug"));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
