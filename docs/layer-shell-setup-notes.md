# Wayscriber Layer-Shell Setup Notes

Source studied: `references/wayscriber/src` from `devmobasa/wayscriber` at commit `feccd312e7e6397de463630101590a9c628e914e`.

## Wayland Connection And Globals

- `backend/wayland/backend/run.rs::run_backend` - top-level Wayland backend entrypoint; calls setup, initializes runtime state, creates the overlay surface, then enters the event loop.
- `backend/wayland/backend/setup.rs::setup_wayland` - connects with `Connection::connect_to_env`, initializes the registry/event queue with `registry_queue_init`, binds compositor, layer-shell, xdg-shell fallback, shm, output, seat, pointer helpers, and optional screencopy.
- `backend/wayland/backend/state_init/mod.rs::init_state` - folds the bound globals and config into `WaylandState`, records compositor capabilities, and disables unsupported frozen mode when layer-shell is missing.
- `backend/wayland/backend/state_init/output.rs::resolve` - resolves output/fallback preferences from env/config and decides whether the main layer surface should use `Overlay` instead of `Top`.

## Main Layer Surface Configuration

- `backend/wayland/backend/surface.rs::create_overlay_surface` - creates the primary `wl_surface`, maps it through layer-shell when available, anchors it to all screen edges, requests full-screen size with `(0, 0)`, sets keyboard interactivity from state, and sets exclusive zone to `-1`.
- `backend/wayland/state/core/accessors.rs::main_surface_layer` - returns `Layer::Overlay` for niri/sway-style environments that can otherwise stack fullscreen windows over top-layer surfaces; returns `Layer::Top` elsewhere.
- `backend/wayland/state/toolbar/visibility/sync.rs::desired_keyboard_interactivity` - returns `None` while the overlay is pass-through, otherwise delegates to the toolbar-aware policy.
- `backend/wayland/state/toolbar/visibility/mod.rs::desired_keyboard_interactivity_for` - uses `OnDemand` when layer-shell toolbars are visible and not inline; otherwise uses `Exclusive` so the drawing overlay can receive keyboard shortcuts.
- `backend/wayland/state/toolbar/visibility/sync.rs::refresh_keyboard_interactivity` - updates the existing layer surface when toolbar visibility or pass-through state changes.
- `backend/wayland/state/core/output.rs::recreate_layer_surface_for_output` - rebuilds the main layer surface for a selected output, repeating the same anchor, size, keyboard, and exclusive-zone settings but passing `Some(output)`.

## Toolbar Layer Surfaces

- `backend/wayland/toolbar/main/lifecycle.rs::ToolbarSurfaceManager::ensure_created` - ensures top and side toolbar layer surfaces exist for the active output and current scale.
- `backend/wayland/toolbar/surfaces/lifecycle.rs::ToolbarSurface::ensure_created` - creates each toolbar layer surface in `Layer::Overlay`, applies its anchor, `OnDemand` keyboard interactivity, exclusive zone `-1`, margins, optional fixed size, and initial commit.
- `backend/wayland/toolbar/main/lifecycle.rs::ToolbarSurfaceManager::handle_configure` - routes a layer-shell configure event to the matching toolbar surface.
- `backend/wayland/toolbar/surfaces/lifecycle.rs::ToolbarSurface::handle_configure` - records toolbar dimensions from the compositor, drops stale buffers when size changes, and marks the toolbar configured/dirty.

## Input Region And Click-Through

- `backend/wayland/overlay_passthrough.rs::set_surface_clickthrough` - sets an empty input region when click-through is requested, or clears the region when the surface should receive input again, then commits the `wl_surface`.
- `backend/wayland/state/core/overlay.rs::set_overlay_clickthrough` - applies click-through to the main overlay surface and mirrors the suppression state to toolbar surfaces.
- `backend/wayland/state/core/overlay.rs::sync_overlay_interactivity` - recomputes click-through and keyboard interactivity from overlay suppression and light-mode pass-through state.
- `backend/wayland/toolbar/surfaces/state.rs::ToolbarSurface::set_suppressed` - applies the same empty input-region behavior to toolbar layer surfaces and clears hit regions while suppressed.

## Configure And Ack Cycle

- `backend/wayland/handlers/mod.rs::delegate_layer!(WaylandState)` - wires smithay-client-toolkit layer-shell dispatch into `WaylandState`; Wayscriber has no direct `ack_configure` call in its source, relying on the toolkit delegate before the handler callback.
- `backend/wayland/handlers/layer.rs::LayerShellHandler::configure` - handles primary layer-surface configure events: updates logical size, invalidates buffers on resize, updates input geometry, refreshes active output labels, marks the surface configured, and syncs toolbar placement.
- `backend/wayland/surface.rs::SurfaceState::set_configured` - records that the main surface has completed its initial configure.
- `backend/wayland/surface.rs::SurfaceState::is_configured` - gates rendering/event-loop behavior until the compositor has configured the surface.
- `backend/wayland/backend/event_loop/mod.rs::run_event_loop` - blocks rendering while the surface is unconfigured and only renders once configure has populated dimensions.

## Output Enumeration Details

- `backend/wayland/handlers/output.rs::OutputHandler::new_output` - refreshes the active-output label when a monitor appears.
- `backend/wayland/handlers/output.rs::OutputHandler::update_output` - refreshes labels and active capture/zoom geometry when the compositor updates an output.
- `backend/wayland/handlers/output.rs::OutputHandler::output_destroyed` - clears stale current-output state and label data when a monitor disappears.
- `backend/wayland/handlers/compositor.rs::CompositorHandler::surface_enter` - treats surface-enter as the authoritative active-output signal, updates scale/geometry/transform, pins toolbars to that output, and loads per-output session state.
- `backend/wayland/state/core/output.rs::preferred_fullscreen_output` - picks a configured/env-preferred output by identity, otherwise uses the current surface output, otherwise the first known output.
- `backend/wayland/state/core/output.rs::sorted_known_outputs` - sorts known outputs by smithay output id so next/previous monitor cycling is stable.
- `backend/wayland/state/core/output.rs::handle_output_focus_action` - cycles outputs, persists the previous output session, and either recreates the layer surface for the target output or reasserts xdg fullscreen on fallback.

## Non-Obvious Choices

- The initial main layer surface passes `None` for output so the compositor chooses placement; Wayscriber later learns the active output via `surface_enter`, and explicit output switching recreates the layer surface with `Some(output)`.
- The main surface normally uses `Layer::Top`, but niri/sway detection moves it to `Layer::Overlay` because fullscreen windows can cover top-layer surfaces there.
- Toolbars always use `Layer::Overlay` so they can stack above the drawing surface, even when the main drawing surface stays on `Top`.
- Exclusive zone is `-1` for both main and toolbar layer surfaces, so the overlay does not reserve layout space or push other surfaces around.
- Click-through is implemented with an empty input region rather than hiding/destroying the surface, which lets capture/frozen/zoom suppression keep rendering state while allowing pointer events to pass through.
- Output identity is built from output name, make, model, or a fallback id; this drives per-output session persistence and preferred-output matching.
