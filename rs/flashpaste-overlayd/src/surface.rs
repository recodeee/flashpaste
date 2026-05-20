#[cfg(any(test, feature = "render"))]
use std::io::ErrorKind;
use std::{error::Error, fmt};

#[cfg(feature = "render")]
use crate::render::RenderCtx;
#[cfg(feature = "render")]
use anyhow::{Context, Result as AnyResult};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_xdg_shell,
    delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::client::{
        globals::{registry_queue_init, BindError},
        protocol::{wl_output, wl_surface},
        Connection, EventQueue, QueueHandle,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler,
            LayerSurface as SctkLayerSurface, LayerSurfaceConfigure,
        },
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
};
#[cfg(feature = "render")]
use smithay_client_toolkit::{
    delegate_shm,
    reexports::client::backend::WaylandError,
    reexports::client::protocol::wl_shm,
    shm::{slot::SlotPool, Shm, ShmHandler},
};
#[cfg(feature = "render")]
use tracing::debug_span;

const NAMESPACE: &str = "flashpaste-overlayd";
const APP_ID: &str = "com.recodeee.flashpaste.overlay";
const FALLBACK_WIDTH: u32 = 1_280;
const FALLBACK_HEIGHT: u32 = 720;
#[cfg(feature = "render")]
const BUFFER_COUNT: usize = 2;

#[derive(Debug)]
pub struct LayerSurface {
    connection: Connection,
    _event_queue: EventQueue<SurfaceState>,
    state: SurfaceState,
}

impl LayerSurface {
    pub fn new() -> Result<Self, LayerSurfaceError> {
        Self::with_options(SurfaceOptions::default())
    }

    pub fn with_options(options: SurfaceOptions) -> Result<Self, LayerSurfaceError> {
        let connection = Connection::connect_to_env()
            .map_err(|err| LayerSurfaceError::WaylandConnect(err.to_string()))?;

        let (globals, mut event_queue) = registry_queue_init::<SurfaceState>(&connection)
            .map_err(|err| LayerSurfaceError::RegistryInit(err.to_string()))?;
        let queue_handle = event_queue.handle();

        let compositor = CompositorState::bind(&globals, &queue_handle)
            .map_err(|err| LayerSurfaceError::CompositorUnavailable(err.to_string()))?;

        #[cfg(feature = "render")]
        let shm = Shm::bind(&globals, &queue_handle)
            .map_err(|err| LayerSurfaceError::ShmUnavailable(err.to_string()))?;

        let wl_surface = compositor.create_surface(&queue_handle);
        let input_region = Region::new(&compositor)
            .map_err(|err| LayerSurfaceError::WaylandObject(err.to_string()))?;

        let mut state = SurfaceState {
            registry_state: RegistryState::new(&globals),
            output_state: OutputState::new(&globals, &queue_handle),
            #[cfg(feature = "render")]
            shm,
            #[cfg(feature = "render")]
            pool: None,
            role: SurfaceRole::Unmapped,
            _input_region: input_region,
            configured_size: None,
            needs_redraw: true,
            closed: false,
        };

        state.role = create_surface_role(
            &globals,
            &queue_handle,
            &mut event_queue,
            &mut state,
            wl_surface,
            options.force_fallback,
        )?;

        connection
            .flush()
            .map_err(|err| LayerSurfaceError::WaylandFlush(err.to_string()))?;

        while state.configured_size.is_none() {
            event_queue
                .blocking_dispatch(&mut state)
                .map_err(|err| LayerSurfaceError::Dispatch(err.to_string()))?;

            if state.closed {
                return Err(LayerSurfaceError::ClosedBeforeConfigure);
            }
        }

        Ok(Self {
            connection,
            _event_queue: event_queue,
            state,
        })
    }

    pub fn configured_size(&self) -> (u32, u32) {
        self.state
            .configured_size
            .unwrap_or((FALLBACK_WIDTH, FALLBACK_HEIGHT))
    }

    pub fn surface_kind(&self) -> SurfaceKind {
        self.state.role.kind()
    }

    #[cfg(feature = "render")]
    pub fn dispatch_pending(&mut self) -> AnyResult<()> {
        let _span = debug_span!("wayland_dispatch").entered();
        self._event_queue
            .dispatch_pending(&mut self.state)
            .context("failed to dispatch pending Wayland events")?;
        self._event_queue
            .flush()
            .context("failed to flush Wayland event queue")?;

        if let Some(guard) = self._event_queue.prepare_read() {
            match guard.read() {
                Ok(_) => {
                    self._event_queue
                        .dispatch_pending(&mut self.state)
                        .context("failed to dispatch Wayland events")?;
                }
                Err(WaylandError::Io(err)) if err.kind() == ErrorKind::WouldBlock => {}
                Err(WaylandError::Io(err)) if is_compositor_disconnect(err.kind()) => {
                    anyhow::bail!("Wayland compositor disconnected: {err}")
                }
                Err(err) => anyhow::bail!("failed to read Wayland events: {err}"),
            }
        }

        anyhow::ensure!(
            !self.state.closed,
            "overlay surface was closed by the compositor"
        );
        Ok(())
    }

    #[cfg(feature = "render")]
    pub fn take_needs_redraw(&mut self) -> bool {
        std::mem::take(&mut self.state.needs_redraw)
    }

    #[cfg(feature = "render")]
    pub fn commit_render(&mut self, renderer: &mut RenderCtx) -> AnyResult<()> {
        let _span = debug_span!("commit_wayland").entered();
        {
            let _span = debug_span!("commit_dispatch").entered();
            self.dispatch_pending()?;
        }

        let (width, height) = self.configured_size();
        anyhow::ensure!(width > 0 && height > 0, "layer surface is not configured");
        anyhow::ensure!(
            renderer.width() == width as i32 && renderer.height() == height as i32,
            "renderer size {}x{} does not match configured surface {}x{}",
            renderer.width(),
            renderer.height(),
            width,
            height
        );

        let width_i32 = width.min(i32::MAX as u32) as i32;
        let height_i32 = height.min(i32::MAX as u32) as i32;
        let stride = width_i32
            .checked_mul(4)
            .context("surface stride overflow")?;

        let buffer = {
            let frame_bytes = (stride as usize)
                .checked_mul(height_i32 as usize)
                .context("surface buffer size overflow")?;
            let (buffer, canvas) = {
                let _span = debug_span!("commit_buffer").entered();
                let pool = ensure_pool(&mut self.state.pool, &self.state.shm, frame_bytes)?;
                pool.create_buffer(width_i32, height_i32, stride, wl_shm::Format::Argb8888)
                    .context("failed to create wl_shm buffer")?
            };
            {
                let _span = debug_span!("commit_copy").entered();
                renderer.copy_to(canvas)?;
            }
            buffer
        };

        {
            let _span = debug_span!("commit_attach_damage_flush").entered();
            let wl_surface = self
                .state
                .role
                .wl_surface()
                .context("surface role closed before commit")?;
            buffer
                .attach_to(wl_surface)
                .context("failed to attach wl_shm buffer")?;
            wl_surface.damage_buffer(0, 0, width_i32, height_i32);
            self.state
                .role
                .commit()
                .context("surface role closed before commit")?;
            self.connection
                .flush()
                .context("failed to flush Wayland commit")?;
        }

        Ok(())
    }
}

impl Drop for LayerSurface {
    fn drop(&mut self) {
        let _ = self.connection.flush();
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceOptions {
    pub force_fallback: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    LayerShell,
    XdgToplevelFallback,
}

#[derive(Debug)]
pub enum LayerSurfaceError {
    LayerShellUnavailable {
        compositor_hint: String,
    },
    XdgShellUnavailable(String),
    WaylandConnect(String),
    RegistryInit(String),
    CompositorUnavailable(String),
    #[cfg(feature = "render")]
    ShmUnavailable(String),
    WaylandObject(String),
    WaylandFlush(String),
    Dispatch(String),
    ClosedBeforeConfigure,
}

impl LayerSurfaceError {
    pub fn is_layer_shell_unavailable(&self) -> bool {
        matches!(self, Self::LayerShellUnavailable { .. })
    }
}

impl fmt::Display for LayerSurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LayerShellUnavailable { compositor_hint } => {
                write!(
                    f,
                    "LayerShellUnavailable {{ compositor_hint: {:?} }}",
                    compositor_hint
                )
            }
            Self::XdgShellUnavailable(err) => write!(f, "xdg-shell is unavailable: {err}"),
            Self::WaylandConnect(err) => write!(f, "failed to connect to Wayland display: {err}"),
            Self::RegistryInit(err) => write!(f, "failed to read Wayland globals: {err}"),
            Self::CompositorUnavailable(err) => write!(f, "wl_compositor is unavailable: {err}"),
            #[cfg(feature = "render")]
            Self::ShmUnavailable(err) => write!(f, "wl_shm is unavailable: {err}"),
            Self::WaylandObject(err) => write!(f, "failed to create Wayland object: {err}"),
            Self::WaylandFlush(err) => write!(f, "failed to flush Wayland requests: {err}"),
            Self::Dispatch(err) => write!(f, "failed while dispatching Wayland events: {err}"),
            Self::ClosedBeforeConfigure => {
                write!(f, "overlay surface closed before the first configure event")
            }
        }
    }
}

impl Error for LayerSurfaceError {}

#[cfg(any(test, feature = "render"))]
fn is_compositor_disconnect(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::BrokenPipe
            | ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::NotConnected
            | ErrorKind::UnexpectedEof
    )
}

fn create_surface_role(
    globals: &smithay_client_toolkit::reexports::client::globals::GlobalList,
    queue_handle: &QueueHandle<SurfaceState>,
    event_queue: &mut EventQueue<SurfaceState>,
    state: &mut SurfaceState,
    wl_surface: wl_surface::WlSurface,
    force_fallback: bool,
) -> Result<SurfaceRole, LayerSurfaceError> {
    if !force_fallback {
        match bind_layer_shell(globals, queue_handle) {
            Ok(layer_shell) => {
                return Ok(create_layer_shell_surface(
                    layer_shell,
                    queue_handle,
                    wl_surface,
                    state.input_region(),
                ));
            }
            Err(err) if err.is_layer_shell_unavailable() => {}
            Err(err) => return Err(err),
        }
    }

    let xdg_shell = XdgShell::bind(globals, queue_handle)
        .map_err(|err| LayerSurfaceError::XdgShellUnavailable(err.to_string()))?;
    let output = primary_output(event_queue, state)?;
    Ok(create_xdg_fallback_surface(
        xdg_shell,
        queue_handle,
        wl_surface,
        state.input_region(),
        output.as_ref(),
    ))
}

fn create_layer_shell_surface(
    layer_shell: LayerShell,
    queue_handle: &QueueHandle<SurfaceState>,
    wl_surface: wl_surface::WlSurface,
    input_region: &smithay_client_toolkit::reexports::client::protocol::wl_region::WlRegion,
) -> SurfaceRole {
    let layer = layer_shell.create_layer_surface(
        queue_handle,
        wl_surface,
        Layer::Overlay,
        Some(NAMESPACE),
        None,
    );
    layer.set_anchor(Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM | Anchor::LEFT);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_exclusive_zone(0);
    layer.set_size(0, 0);
    layer.wl_surface().set_input_region(Some(input_region));
    layer.commit();
    SurfaceRole::Layer(layer)
}

fn create_xdg_fallback_surface(
    xdg_shell: XdgShell,
    queue_handle: &QueueHandle<SurfaceState>,
    wl_surface: wl_surface::WlSurface,
    input_region: &smithay_client_toolkit::reexports::client::protocol::wl_region::WlRegion,
    output: Option<&wl_output::WlOutput>,
) -> SurfaceRole {
    let window = xdg_shell.create_window(wl_surface, WindowDecorations::None, queue_handle);
    window.set_title("flashpaste overlay");
    window.set_app_id(APP_ID);
    window.set_fullscreen(output);
    window.wl_surface().set_input_region(Some(input_region));
    window.commit();
    SurfaceRole::Xdg(window)
}

fn primary_output(
    event_queue: &mut EventQueue<SurfaceState>,
    state: &mut SurfaceState,
) -> Result<Option<wl_output::WlOutput>, LayerSurfaceError> {
    for _ in 0..2 {
        if let Some(output) = state.output_state.outputs().next() {
            return Ok(Some(output));
        }
        event_queue
            .blocking_dispatch(state)
            .map_err(|err| LayerSurfaceError::Dispatch(err.to_string()))?;
    }

    Ok(None)
}

fn bind_layer_shell<State>(
    globals: &smithay_client_toolkit::reexports::client::globals::GlobalList,
    queue_handle: &QueueHandle<State>,
) -> Result<LayerShell, LayerSurfaceError>
where
    State: smithay_client_toolkit::reexports::client::Dispatch<
            smithay_client_toolkit::reexports::protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1,
            smithay_client_toolkit::globals::GlobalData,
            State,
        > + LayerShellHandler
        + 'static,
{
    match LayerShell::bind(globals, queue_handle) {
        Ok(layer_shell) => Ok(layer_shell),
        Err(BindError::NotPresent) => Err(LayerSurfaceError::LayerShellUnavailable {
            compositor_hint: "Compositor did not advertise zwlr_layer_shell_v1. GNOME/Mutter is expected to fail here; Hyprland, Sway, wlroots compositors, and KDE Plasma should expose layer-shell.".to_string(),
        }),
        Err(BindError::UnsupportedVersion) => Err(LayerSurfaceError::LayerShellUnavailable {
            compositor_hint: "Compositor advertised zwlr_layer_shell_v1, but not a supported version for this client.".to_string(),
        }),
    }
}

#[derive(Debug)]
#[cfg_attr(not(feature = "render"), allow(dead_code))]
enum SurfaceRole {
    Unmapped,
    Layer(SctkLayerSurface),
    Xdg(Window),
}

impl SurfaceRole {
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    fn kind(&self) -> SurfaceKind {
        match self {
            Self::Unmapped | Self::Layer(_) => SurfaceKind::LayerShell,
            Self::Xdg(_) => SurfaceKind::XdgToplevelFallback,
        }
    }

    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    fn wl_surface(&self) -> Option<&wl_surface::WlSurface> {
        match self {
            Self::Unmapped => None,
            Self::Layer(layer) => Some(layer.wl_surface()),
            Self::Xdg(window) => Some(window.wl_surface()),
        }
    }

    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    fn commit(&self) -> Option<()> {
        match self {
            Self::Unmapped => None,
            Self::Layer(layer) => {
                layer.commit();
                Some(())
            }
            Self::Xdg(window) => {
                window.commit();
                Some(())
            }
        }
    }
}

#[derive(Debug)]
struct SurfaceState {
    registry_state: RegistryState,
    output_state: OutputState,
    #[cfg(feature = "render")]
    shm: Shm,
    #[cfg(feature = "render")]
    pool: Option<SlotPool>,
    role: SurfaceRole,
    _input_region: Region,
    configured_size: Option<(u32, u32)>,
    needs_redraw: bool,
    closed: bool,
}

impl SurfaceState {
    fn input_region(
        &self,
    ) -> &smithay_client_toolkit::reexports::client::protocol::wl_region::WlRegion {
        self._input_region.wl_region()
    }

    fn primary_output_size(&self) -> Option<(u32, u32)> {
        let output = self.output_state.outputs().next()?;
        output_size_from_info(self.output_state.info(&output)?)
    }
}

impl CompositorHandler for SurfaceState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for SurfaceState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for SurfaceState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &SctkLayerSurface) {
        self.closed = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _layer: &SctkLayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let configured_size = (
            non_zero_or(configure.new_size.0, FALLBACK_WIDTH),
            non_zero_or(configure.new_size.1, FALLBACK_HEIGHT),
        );
        if self.configured_size != Some(configured_size) {
            self.configured_size = Some(configured_size);
            #[cfg(feature = "render")]
            {
                self.pool = None;
            }
            self.needs_redraw = true;
        }
    }
}

impl WindowHandler for SurfaceState {
    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _window: &Window) {
        self.closed = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let fallback_size = self
            .primary_output_size()
            .unwrap_or((FALLBACK_WIDTH, FALLBACK_HEIGHT));
        let configured_size = (
            configure
                .new_size
                .0
                .map(|width| width.get())
                .unwrap_or(fallback_size.0),
            configure
                .new_size
                .1
                .map(|height| height.get())
                .unwrap_or(fallback_size.1),
        );

        if self.configured_size != Some(configured_size) {
            self.configured_size = Some(configured_size);
            #[cfg(feature = "render")]
            {
                self.pool = None;
            }
            self.needs_redraw = true;
        }
    }
}

#[cfg(feature = "render")]
fn ensure_pool<'a>(
    pool: &'a mut Option<SlotPool>,
    shm: &Shm,
    frame_bytes: usize,
) -> AnyResult<&'a mut SlotPool> {
    let target_size = frame_bytes
        .checked_mul(BUFFER_COUNT)
        .context("double-buffer pool size overflow")?;

    match pool {
        Some(existing) if existing.len() >= target_size => {}
        _ => {
            *pool =
                Some(SlotPool::new(target_size, shm).context("failed to create wl_shm slot pool")?);
        }
    }

    pool.as_mut()
        .context("wl_shm slot pool was not initialized")
}

#[cfg(feature = "render")]
impl ShmHandler for SurfaceState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(SurfaceState);
delegate_output!(SurfaceState);
#[cfg(feature = "render")]
delegate_shm!(SurfaceState);
delegate_layer!(SurfaceState);
delegate_xdg_shell!(SurfaceState);
delegate_xdg_window!(SurfaceState);
delegate_registry!(SurfaceState);

impl ProvidesRegistryState for SurfaceState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

fn non_zero_or(value: u32, fallback: u32) -> u32 {
    if value == 0 {
        fallback
    } else {
        value
    }
}

fn output_size_from_info(info: smithay_client_toolkit::output::OutputInfo) -> Option<(u32, u32)> {
    if let Some((width, height)) = info.logical_size {
        return Some((width.max(1) as u32, height.max(1) as u32));
    }

    info.modes
        .iter()
        .find(|mode| mode.current || mode.preferred)
        .or_else(|| info.modes.first())
        .map(|mode| {
            (
                mode.dimensions.0.max(1) as u32,
                mode.dimensions.1.max(1) as u32,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compositor_disconnect_error_kinds_are_classified() {
        assert!(is_compositor_disconnect(ErrorKind::BrokenPipe));
        assert!(is_compositor_disconnect(ErrorKind::ConnectionReset));
        assert!(is_compositor_disconnect(ErrorKind::UnexpectedEof));
        assert!(!is_compositor_disconnect(ErrorKind::WouldBlock));
    }

    #[test]
    fn closed_before_configure_has_useful_message() {
        assert_eq!(
            LayerSurfaceError::ClosedBeforeConfigure.to_string(),
            "overlay surface closed before the first configure event"
        );
    }
}
