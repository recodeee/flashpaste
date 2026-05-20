//! Threading model: `flashpaste-overlayd` runs Tokio on the current thread and
//! keeps every smithay-client-toolkit / Wayland object on that same main
//! thread. The IPC listener shares `Arc<tokio::sync::Mutex<ShapeStore>>` and
//! sends redraw notifications over an mpsc channel; it never touches Wayland.
//! The main thread responds by snapshotting the store, rendering into a Cairo
//! buffer, and committing the wl_surface. Animation timers and Wayland polling
//! are only active while shapes exist. When launched by systemd with
//! `Type=notify`, the daemon sends `READY=1` after the IPC socket and initial
//! render are ready.

#[cfg(all(feature = "wayland", feature = "render", target_os = "linux"))]
use std::os::linux::net::SocketAddrExt;
#[cfg(all(feature = "wayland", feature = "render", target_os = "linux"))]
use std::os::unix::net::SocketAddr;
#[cfg(all(feature = "wayland", feature = "render"))]
use std::{env, os::unix::net::UnixDatagram, path::Path};
use std::{process::ExitCode, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
#[cfg(any(feature = "surface", feature = "wayland"))]
use flashpaste_overlayd::surface::{LayerSurface, SurfaceOptions};
use flashpaste_overlayd::{
    ipc,
    store::{ShapeStore, SharedShapeStore},
};
#[cfg(all(feature = "wayland", feature = "render"))]
use flashpaste_overlayd::{
    protocol::{DrawRect, DrawStyle, DEFAULT_CURRENT_OPACITY},
    render::RenderCtx,
};
use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};
#[cfg(all(feature = "wayland", feature = "render"))]
use tokio::time::{Instant, Sleep};
use tracing::error;
#[cfg(all(feature = "wayland", feature = "render"))]
use tracing::{debug_span, info, Instrument};
use tracing_subscriber::{fmt, fmt::format::FmtSpan, EnvFilter};
#[cfg(all(feature = "wayland", feature = "render"))]
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(version, about = "Wayland overlay daemon for flashpaste")]
struct Args {
    /// Create the Wayland layer-shell surface and exit.
    #[arg(long)]
    probe: bool,

    /// Draw one red test rectangle for three seconds, then exit.
    #[arg(long)]
    demo: bool,

    /// Use the GNOME-compatible xdg_toplevel fallback even when layer-shell exists.
    #[arg(long)]
    force_fallback: bool,

    /// Run only IPC and store expiry ticks. Hidden integration-test hook.
    #[arg(long, hide = true)]
    headless_test: bool,
}

#[cfg(all(feature = "wayland", feature = "render"))]
const ANIMATION_FRAME: Duration = Duration::from_millis(16);

#[cfg(all(feature = "wayland", feature = "render"))]
const WAYLAND_POLL: Duration = Duration::from_millis(8);

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    init_tracing();
    let args = Args::parse();

    if args.probe {
        return run_probe(args.force_fallback);
    }

    if args.demo {
        return run_demo(args.force_fallback);
    }

    if args.headless_test {
        return run_headless_test().await;
    }

    match run_daemon(args.force_fallback).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err:#}");
            eprintln!("flashpaste-overlayd: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_headless_test() -> ExitCode {
    match run_headless_test_inner().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!("{err:#}");
            eprintln!("flashpaste-overlayd --headless-test: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_headless_test_inner() -> Result<()> {
    let store = ShapeStore::shared();
    let (redraw_tx, redraw_rx) = mpsc::channel(32);
    let _ipc_server = ipc::spawn_listener_with_redraw(store.clone(), redraw_tx).await?;
    let tick_task = tokio::spawn(store_expiry_tick(store, redraw_rx));

    tokio::signal::ctrl_c().await.context("wait for ctrl-c")?;
    tick_task.abort();
    Ok(())
}

async fn store_expiry_tick(store: SharedShapeStore, mut redraw_rx: mpsc::Receiver<()>) {
    let mut interval = time::interval(Duration::from_millis(16));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut active = false;

    loop {
        tokio::select! {
            request = redraw_rx.recv() => {
                if request.is_none() {
                    break;
                }
                while redraw_rx.try_recv().is_ok() {}
                active = true;
            }
            _ = interval.tick(), if active => {
                let mut store = store.lock().await;
                store.tick();
                active = !store.is_empty();
            }
        }
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,flashpaste_overlayd=debug"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .try_init();
}

#[cfg(all(feature = "wayland", feature = "render"))]
fn run_demo(force_fallback: bool) -> ExitCode {
    match run_demo_inner(force_fallback) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("flashpaste-overlayd --demo: {err:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(all(feature = "wayland", feature = "render"))]
fn run_demo_inner(force_fallback: bool) -> Result<()> {
    let mut surface = LayerSurface::with_options(SurfaceOptions { force_fallback })
        .context("create Wayland overlay surface")?;
    let (width, height) = surface.configured_size();
    let mut renderer = RenderCtx::new(width, height)?;
    renderer.clear_all()?;
    renderer.draw_rect(&DrawRect {
        style: DrawStyle {
            id: Uuid::new_v4(),
            ttl_ms: 3_000,
            color: "#ff0000".parse()?,
            stroke_width: 8.0,
            current_opacity: DEFAULT_CURRENT_OPACITY,
        },
        x: 400.0,
        y: 300.0,
        w: 200.0,
        h: 100.0,
    })?;
    surface
        .commit_render(&mut renderer)
        .context("commit demo rectangle")?;
    std::thread::sleep(Duration::from_secs(3));
    Ok(())
}

#[cfg(not(all(feature = "wayland", feature = "render")))]
fn run_demo(_force_fallback: bool) -> ExitCode {
    eprintln!("flashpaste-overlayd --demo requires the wayland and render features");
    ExitCode::from(2)
}

#[cfg(all(feature = "wayland", feature = "render"))]
async fn run_daemon(force_fallback: bool) -> Result<()> {
    let store = ShapeStore::shared();
    let (redraw_tx, redraw_rx) = mpsc::channel(32);
    let _ipc_server = ipc::spawn_listener_with_redraw(store.clone(), redraw_tx).await?;

    run_wayland_loop(store, redraw_rx, force_fallback).await
}

#[cfg(not(all(feature = "wayland", feature = "render")))]
async fn run_daemon(_force_fallback: bool) -> Result<()> {
    anyhow::bail!("flashpaste-overlayd daemon mode requires the wayland and render features")
}

#[cfg(all(feature = "wayland", feature = "render"))]
async fn run_wayland_loop(
    store: SharedShapeStore,
    mut redraw_rx: mpsc::Receiver<()>,
    force_fallback: bool,
) -> Result<()> {
    let mut surface = LayerSurface::with_options(SurfaceOptions { force_fallback })
        .context("create Wayland overlay surface")?;
    let mut renderer: Option<RenderCtx> = None;
    let mut wayland_poll = time::interval(WAYLAND_POLL);
    wayland_poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut animation_sleep = Box::pin(time::sleep(ANIMATION_FRAME));

    let mut has_active_shapes = render_store(&mut surface, &store, &mut renderer).await?;
    schedule_animation_sleep(&mut animation_sleep, has_active_shapes);
    info!(
        socket = %ipc::default_socket_path().display(),
        surface_kind = ?surface.surface_kind(),
        "flashpaste-overlayd running"
    );
    notify_systemd_ready();

    loop {
        tokio::select! {
            request = redraw_rx.recv() => {
                if request.is_none() {
                    break;
                }
                while redraw_rx.try_recv().is_ok() {}
                has_active_shapes = render_store(&mut surface, &store, &mut renderer).await?;
                schedule_animation_sleep(&mut animation_sleep, has_active_shapes);
            }
            _ = &mut animation_sleep, if has_active_shapes => {
                has_active_shapes = render_store(&mut surface, &store, &mut renderer).await?;
                schedule_animation_sleep(&mut animation_sleep, has_active_shapes);
            }
            _ = wayland_poll.tick(), if has_active_shapes => {
                surface.dispatch_pending().context("pump Wayland events")?;
                if surface.take_needs_redraw() {
                    has_active_shapes = render_store(&mut surface, &store, &mut renderer).await?;
                    schedule_animation_sleep(&mut animation_sleep, has_active_shapes);
                }
            }
            signal = tokio::signal::ctrl_c() => {
                signal.context("wait for ctrl-c")?;
                info!("flashpaste-overlayd shutting down");
                break;
            }
        }
    }

    Ok(())
}

#[cfg(all(feature = "wayland", feature = "render"))]
fn schedule_animation_sleep(animation_sleep: &mut std::pin::Pin<Box<Sleep>>, active: bool) {
    if active {
        animation_sleep
            .as_mut()
            .reset(Instant::now() + ANIMATION_FRAME);
    }
}

#[cfg(all(feature = "wayland", feature = "render"))]
async fn render_store(
    surface: &mut LayerSurface,
    store: &SharedShapeStore,
    renderer: &mut Option<RenderCtx>,
) -> Result<bool> {
    let _span = debug_span!("render_store").entered();
    {
        let _span = debug_span!("surface_dispatch").entered();
        surface
            .dispatch_pending()
            .context("pump Wayland events before render")?;
    }
    let (width, height) = surface.configured_size();

    let recreate = renderer
        .as_ref()
        .map(|renderer| renderer.width() != width as i32 || renderer.height() != height as i32)
        .unwrap_or(true);
    if recreate {
        let _span = debug_span!("renderer_recreate", width, height).entered();
        *renderer = Some(RenderCtx::new(width, height)?);
    }

    let snapshot = async {
        let mut store = store.lock().await;
        store.tick();
        store.snapshot()
    }
    .instrument(debug_span!("store_snapshot"))
    .await;
    let has_active_shapes = !snapshot.is_empty();
    let renderer = renderer
        .as_mut()
        .context("renderer was not initialized before rendering")?;
    {
        let _span = debug_span!("render", shapes = snapshot.len()).entered();
        renderer.render_shapes(&snapshot)?;
    }
    {
        let _span = debug_span!("commit").entered();
        surface.commit_render(renderer)?;
    }
    Ok(has_active_shapes)
}

#[cfg(any(feature = "surface", feature = "wayland"))]
fn run_probe(force_fallback: bool) -> ExitCode {
    match LayerSurface::with_options(SurfaceOptions { force_fallback }) {
        Ok(surface) => {
            println!(
                "flashpaste-overlayd --probe: {:?} surface OK (configured_size={:?})",
                surface.surface_kind(),
                surface.configured_size()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("flashpaste-overlayd --probe: {err}");
            if err.is_layer_shell_unavailable() {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(not(any(feature = "surface", feature = "wayland")))]
fn run_probe(_force_fallback: bool) -> ExitCode {
    eprintln!("flashpaste-overlayd --probe requires the surface or wayland feature");
    ExitCode::from(2)
}

#[cfg(all(feature = "wayland", feature = "render"))]
fn notify_systemd_ready() {
    if let Err(err) = sd_notify("READY=1\nSTATUS=flashpaste-overlayd running") {
        error!("failed to notify systemd readiness: {err:#}");
    }
}

#[cfg(all(feature = "wayland", feature = "render"))]
fn sd_notify(message: &str) -> Result<()> {
    let Some(socket) = env::var_os("NOTIFY_SOCKET") else {
        return Ok(());
    };
    if socket.is_empty() {
        return Ok(());
    }

    let client = UnixDatagram::unbound()?;
    let socket = socket.to_string_lossy();
    if let Some(name) = socket.strip_prefix('@') {
        #[cfg(target_os = "linux")]
        {
            let addr = SocketAddr::from_abstract_name(name.as_bytes())?;
            client.connect_addr(&addr)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            return Ok(());
        }
    } else {
        client.connect(Path::new(socket.as_ref()))?;
    }
    client.send(message.as_bytes())?;
    Ok(())
}
