# Overlay Reference Repos

Study pass date: 2026-05-20.

Scope:
- Cloned under `references/` and treated as read-only reference material.
- Read each `README.md`, dependency manifest/build file, and top-level `src/` listing.
- Used targeted source searches only to confirm layer-shell, GNOME, and rendering behavior.
- No code copied.

Reference snapshots:
- `references/wayscriber`: `feccd31`
- `references/gromit-mpx`: `72eab92`
- `references/gtk4-layer-shell`: `2a70cb0`

## devmobasa/wayscriber

Top-level `src/` shape:

```text
about_window, app, backend, capture, config, daemon, draw, input, paths,
session, toolbar_icons, ui, plus main/lib/cli/runtime/support modules
```

Layer-shell crates/libraries:
- Rust native Wayland stack.
- `smithay-client-toolkit` provides `LayerShell`, `XdgShell`, compositor, output, seat, shm, activation, pointer-constraints, and relative-pointer helpers.
- `wayland-client`, `wayland-protocols`, and `wayland-protocols-wlr` provide protocol bindings, including `wlr-layer-shell` and `zwlr_screencopy_manager_v1`.
- Rendering dependencies include `cairo-rs`, `pango`, and `pangocairo`.

GNOME handling:
- README marks GNOME as partial support: portal fallback and windowed overlay.
- Startup tries to bind layer-shell first. If it is unavailable, it logs desktop/session context and falls back to `xdg-shell`.
- The fallback creates an undecorated xdg window, sets app id/title, requests fullscreen or maximized state, and uses xdg activation when available.
- Capture/zoom/freeze paths can fall back to xdg-desktop-portal when compositor-native screencopy is unavailable.
- Shortcut setup has explicit GNOME custom-shortcut support and separate portal shortcut behavior for non-GNOME environments.

Rendering:
- CPU-rendered Cairo into Wayland shared-memory buffers.
- The surface code keeps a `SlotPool`, creates ARGB buffers, wraps buffer memory with `cairo::ImageSurface::create_for_data_unsafe`, draws with Cairo/PangoCairo, flushes, attaches the Wayland buffer, sends damage regions, and commits.
- It uses frame callbacks for vsync and tracks dirty/damage regions to reduce redraw work.
- No OpenGL rendering path was evident in the studied files.

Pattern to adopt:
- Keep a first-class capability split: layer-shell surface when available, xdg-shell fallback when not, and user-visible feature gates for capabilities that only work with layer-shell or screencopy.

Pattern to avoid:
- Avoid importing a whole annotation-app architecture into flashpaste. Wayscriber is broad: daemon, tray, boards, toolbar layers, session persistence, capture, zoom, and multiple UI systems. For flashpaste, keep the overlay path narrow and do not make the paste/screenshot hot path depend on a large in-process drawing app model.

## bk138/gromit-mpx

Top-level `src/` shape:

```text
callbacks, config, coordlist_ops, drawing, input, main headers and C files
```

Layer-shell crates/libraries:
- None. Gromit-MPX is not a native layer-shell implementation.
- It is a C/GTK3/X11 application. `CMakeLists.txt` depends on `gtk+-3.0`, `xi`, `x11`, appindicator/ayatana-appindicator, and `liblz4`.
- Wayland support is via XWayland, not `wlr-layer-shell`.

GNOME handling:
- README says it works on Wayland sessions using XWayland and requires XWayland if it cannot open a display.
- `main.c` explicitly restricts GDK to the X11 backend with `gdk_set_allowed_backends("x11")`.
- Under Wayland, hotkey grabbing does not work reliably through XWayland, so `input.c` detects GNOME and writes/removes GNOME custom shortcuts via GSettings. Those shortcuts invoke `gromit-mpx --toggle`, `--clear`, `--visibility`, `--quit`, `--undo`, and `--redo`.
- Input pass-through vs active drawing is handled by changing the GTK input shape region on the transparent XWayland window.

Rendering:
- Cairo on GTK3/X11.
- Uses a fullscreen `GTK_WINDOW_POPUP`, app-paintable transparent window, Cairo image backbuffers, and GTK draw callbacks.
- Composited desktops use alpha/compositing. If no compositor is present, it falls back to legacy shape-extension behavior, which the README calls potentially slow.
- Undo state is compressed with LZ4.
- No layer-shell or OpenGL rendering path was evident in the studied files.

Pattern to adopt:
- Keep external commands as the control surface for a resident process. A simple `--toggle`/`--clear`/`--quit` style control API maps well to global shortcuts and avoids requiring UI focus.

Pattern to avoid:
- Do not use XWayland as the primary overlay strategy for flashpaste. It works as a compatibility path, but it brings compositor-specific shortcut hacks, input-shape edge cases, and a non-native Wayland model.

## wmww/gtk4-layer-shell

Top-level `src/` shape:

```text
gtk4-layer-shell/session-lock entry points, layer/lock surface plumbing,
registry, xdg surface server, libwayland shim, stubbed surface, preload helper
```

Layer-shell crates/libraries:
- C library for GTK4 applications, exposed through `gtk4-layer-shell-0` pkg-config and GObject introspection.
- Depends on GTK4, `wayland-client`, and `wayland-protocols`.
- Uses generated `wlr-layer-shell-unstable-v1` and `ext-session-lock-v1` client protocol bindings.
- README points Rust users to external safe Rust bindings, but this repo itself is C.

GNOME handling:
- README is explicit: layer-shell is not supported on GNOME-on-Wayland or X11.
- `gtk_layer_init_for_window()` returns with warnings when not on Wayland, when the libwayland shim is unavailable, or when the compositor does not expose layer-shell.
- There is no GNOME overlay fallback. Consumers must provide their own xdg/portal/non-overlay behavior if GNOME matters.

Rendering:
- The library does not implement an app rendering pipeline. It turns GTK windows into layer-shell surfaces and lets GTK4 render normal widgets through its own renderer.
- The source contains no custom Cairo/OpenGL rendering path in the studied top-level `src` files.
- Internally it hooks xdg surface creation, maps it to a layer surface, forwards configure events, manages anchors/margins/exclusive zones/keyboard mode, and exposes GTK-friendly APIs.

Pattern to adopt:
- Isolate protocol-role plumbing behind a small API. The caller should say "make this window/surface a layer surface" and configure anchors, layer, namespace, and keyboard mode without spreading protocol details through app logic.

Pattern to avoid:
- Do not rely on gtk4-layer-shell alone if GNOME support is a requirement. It correctly treats missing layer-shell as unsupported, but flashpaste needs an explicit fallback path rather than a warning-only failure.

## Flashpaste Implications

- The durable overlay design is not "choose layer-shell or GNOME." It is a capability matrix: native layer-shell for wlroots/KWin/etc., xdg/portal-compatible fallback for GNOME, and clear feature gating where pass-through, global shortcuts, or screencopy differ.
- Cairo over Wayland SHM is a reasonable first rendering path for lightweight overlay UI. It matches wayscriber and gromit-mpx and avoids introducing GPU/GL complexity before the overlay needs it.
- Keep overlay process control command-oriented. Global shortcuts should call stable CLI verbs instead of relying on focused overlay windows.
- Keep GTK4 layer-shell as a reference for API shape and protocol isolation, not as a complete answer for GNOME.
