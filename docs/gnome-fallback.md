# GNOME Fallback Design — `flashpaste-overlayd`

- **Status:** Design (no code yet)
- **Date:** 2026-05-20
- **Scope:** Phase 5, Prompt 19 of `docs/flashpaste-overlayd-plan.md`
- **Reference:** `references/wayscriber/src/backend/wayland` (commit `feccd31`)
- **Implements (later):** Prompt 20 (rendering), Prompt 21 (multi-monitor), Prompt 22 (click-through)

## Problem

`flashpaste-overlayd` is built on `zwlr_layer_shell_v1`. Mutter — the GNOME
compositor — does not implement layer-shell and the upstream position is that
it will not. On a GNOME Wayland session, binding `LayerShell` returns
`BindError::NotPresent` and the daemon currently has no surface to paint on.

We need a fallback that produces a usable, click-through, full-output overlay
on GNOME without requiring a non-default mutter build.

This document defines what the fallback will look like. It does **not** ship
code; implementation lands in a later prompt once the design is reviewed.

## Approach

When `LayerShell::bind` fails, fall through to `XdgShell` and create a single
`xdg_toplevel` per output that mimics an overlay as closely as the protocol
permits. Concretely:

1. **Surface.** Create a regular `wl_surface` via `wl_compositor.create_surface`,
   then attach an `xdg_surface` + `xdg_toplevel` from `XdgShell`. One surface
   per `wl_output` (matches the layer-shell path).
2. **No decorations.** Use `WindowDecorations::None`. Where
   `zxdg_decoration_manager_v1` is available, additionally request
   `ServerSide` mode set to `None` so a stray titlebar from kwin-style server
   decoration is impossible. (Mutter does not advertise this global today, but
   the fallback might be exercised on other non-layer-shell compositors.)
3. **Transparent background.** ARGB8888 `wl_shm` buffer with all-zero alpha
   outside drawn shapes. Cairo `set_operator(OPERATOR_SOURCE)` on the clear
   pass keeps premultiplication correct. This is the same pixel format the
   layer-shell path uses; no GNOME-specific change.
4. **Cover the output.** On `xdg_toplevel.configure`, prefer
   `set_fullscreen(Some(&output))` so the compositor sizes the window to the
   exact output geometry. Wayscriber found that mutter renders fullscreen
   xdg_toplevels as **opaque** (the compositor draws an opaque black backdrop
   regardless of the buffer's alpha) and therefore falls back to
   `set_maximized()` on GNOME — see
   `references/wayscriber/src/backend/wayland/backend/state_init/output.rs:34`.
   We adopt the same default: `maximized` on GNOME, `fullscreen` elsewhere
   when the fallback is exercised. An env override
   (`FLASHPASTE_OVERLAY_XDG_FULLSCREEN=1`) mirrors wayscriber's escape hatch
   for users who want fullscreen anyway.
5. **Always-on-top hints.** xdg-shell has no portable "always-on-top" request
   in the protocol; we use the strongest hints available:
   - `xdg_toplevel.set_app_id("flashpaste-overlay")` — gives the user a
     stable identifier to pin via GNOME tweaks / extensions if they want to.
   - `xdg_toplevel.set_title("flashpaste overlay")` — short, recognizable.
   - Optionally bind `zwp_keyboard_shortcuts_inhibit_v1` so user shortcuts
     keep firing through the overlay (no effect on z-order, but cheap).
   - On compositors that ship the KDE-specific `kde-screen-edge-v1` /
     `org_kde_plasma_window_management` we could ask to pin above; we will
     **not** implement those in the first cut. The fallback explicitly
     targets the GNOME case and stays vanilla xdg-shell there.
6. **Click-through.** After the surface is mapped, call
   `wl_surface.set_input_region(Some(empty))` where `empty` is a `wl_region`
   created with no `add` calls. The compositor then routes pointer/touch to
   whatever is underneath. This is the same primitive wayscriber uses
   (`references/wayscriber/src/backend/wayland/overlay_passthrough.rs`)
   and it works identically on mutter and wlroots.
7. **Configure lifecycle.** Implement `WindowHandler::configure` to:
   - Read the proposed size from the configure event; if `None`, fall back to
     the matching `wl_output` logical size.
   - Reassert `set_fullscreen(Some(&output))` (or `set_maximized()`) on every
     configure — mutter has been observed to drop fullscreen across mode
     switches. Pattern from
     `references/wayscriber/src/backend/wayland/handlers/xdg.rs:71-82`.
   - Mark the surface configured and damage everything.
8. **Per-output tracking.** Maintain
   `Map<output_id, FallbackSurface>` parallel to the layer-shell
   `Map<output_id, LayerSurface>`. On `wl_output` add/remove, create or drop
   the matching fallback surface.

## Detection

We pick a path per session:

```text
if let Ok(layer_shell) = LayerShell::bind(&globals, &qh) {
    Path::LayerShell(layer_shell)
} else if let Ok(xdg_shell) = XdgShell::bind(&globals, &qh) {
    Path::XdgFallback(xdg_shell)
} else {
    return Err("compositor exposes neither zwlr_layer_shell_v1 nor xdg_shell");
}
```

We log `XDG_CURRENT_DESKTOP`, `XDG_SESSION_DESKTOP`, and the bind error so
users opening a bug report on GNOME see the path chosen at startup.

`flashpaste-doctor` will gain a check (later, in Prompt 24) that reports the
selected path so users can confirm the fallback is in use on GNOME.

## What the user has to do

On GNOME the first launch will trigger an xdg-desktop-portal permission flow
because the overlay socket and a screenshot-class portal call (used by Prompt
22's click-through self-test) cross the portal trust boundary. The user must
click "Allow" once. We do **not** require any GNOME extension and we do **not**
ship a tweak that forces always-on-top. If the user wants the overlay pinned
above other windows, they install a GNOME extension of their choice that
targets `app_id="flashpaste-overlay"`; this is documented but not
auto-installed.

## Limitations (must be in the user-facing docs at ship time)

- **Z-order against fullscreen apps is not guaranteed.** With xdg-shell there
  is no protocol mechanism that pins a normal toplevel above another
  client's fullscreen window. If the user is in a fullscreen browser,
  fullscreen game, or full-screen video player, the overlay will be hidden
  underneath. On layer-shell compositors this is solved by `Layer::Overlay`;
  on GNOME there is no equivalent and we will not pretend otherwise.
- **Flicker on enter/leave.** Mapping/unmapping an xdg_toplevel triggers
  mutter's normal window-show animation. Users may see a brief slide-in
  on first show and a fade-out on hide. We will keep the surface mapped for
  the lifetime of the daemon (drawing nothing when there are no annotations)
  to suppress per-shape flicker, but the initial map cannot be skipped. The
  layer-shell path has no such animation.
- **Portal permission required once.** First launch prompts via
  `xdg-desktop-portal` (org.freedesktop.portal.Background or similar,
  depending on what the click-through verifier ends up using). The daemon
  must surface this clearly; a silent failure here is the worst-case bug.
- **Fullscreen is opaque on mutter.** Documented above; we default to
  maximized on GNOME for that reason.
- **Light passthrough mode is disabled.** Wayscriber disables its
  "passthrough while keeping some keyboard shortcuts" mode on the xdg
  fallback because keyboard routing through an xdg_toplevel cannot be made
  reliable. `flashpaste-overlayd` does not have a passthrough-with-keyboard
  mode in v1, so this is informational only.
- **Single-output `set_fullscreen` is best-effort.** When the user has
  multiple monitors, mutter sometimes picks the focused output instead of
  the requested one. The configure handler reasserts the preferred output
  on each event; Prompt 21 (multi-monitor) covers this in more depth.

## What we deliberately do NOT do

- **No X11 fallback.** flashpaste targets Wayland; an X11 path is out of
  scope.
- **No GNOME extension.** Shipping a `.zip` GNOME extension that pins the
  overlay above fullscreen apps would solve the z-order limitation, but it
  is a distribution-class undertaking (review process, per-version
  manifests, signing) and ties us to GNOME's extension API churn.
- **No mutter patch or out-of-tree compositor build.**
- **No "fake fullscreen by polling output geometry and resizing a regular
  window."** We rely on `set_maximized` / `set_fullscreen` instead. Polling
  is fragile under DPI changes and hot-plug.
- **No layer-shell shim via `gtk4-layer-shell`.** That crate is GTK-only and
  internally requires layer-shell support from the compositor; it does not
  provide a fallback on GNOME.
- **No silent permission grant.** If the portal call is declined, the
  daemon logs the decline at `WARN` and continues without the verifier;
  we do not retry behind the user's back.

## File map (for the future implementation prompt)

When the implementation lands, the changes are confined to:

- `rs/flashpaste-overlayd/src/backend/wayland/setup.rs` — detect path,
  bind `XdgShell` alongside `LayerShell`.
- `rs/flashpaste-overlayd/src/backend/wayland/surface.rs` — branch on
  bound globals; new `create_xdg_fallback_surface` mirroring the existing
  `create_layer_surface`.
- `rs/flashpaste-overlayd/src/backend/wayland/handlers/xdg.rs` — new file,
  implements `WindowHandler::configure` and `request_close`.
- `rs/flashpaste-overlayd/src/backend/wayland/input_region.rs` — small
  helper that takes a `&CompositorState` + `&WlSurface` and applies an empty
  `wl_region`. Used by both paths.
- `rs/flashpaste-overlayd/src/config.rs` — add
  `overlay.xdg_fullscreen: bool` (default `false` on GNOME, `true`
  elsewhere) and the matching env override.
- `docs/install.md` and `docs/troubleshooting.md` — document the GNOME
  caveats listed above.

No changes to the wire protocol (`docs/overlay-protocol.md`). The fallback is
purely a backend concern; the five primitives (`draw_rect`, `draw_circle`,
`draw_arrow`, `draw_label`, `clear`) render identically on either surface.

## Acceptance criteria for the implementation prompt (Prompt 20+)

When this design is built, it is "done" when:

1. On GNOME 45+, `flashpaste-overlayd --demo` paints the demo rectangle and
   it fades out without crashing.
2. `wl-paste`, terminal scrolling, and clicking through the overlay all
   work — the overlay does not steal pointer input.
3. `flashpaste-doctor` reports the selected backend
   (`layer-shell` or `xdg-fallback`) and the desktop environment.
4. On a layer-shell-capable compositor (Sway, KWin), behavior is bit-for-bit
   identical to today; the fallback code path is never entered.
5. The limitations section above is mirrored verbatim into
   `docs/troubleshooting.md` under a "GNOME fallback" heading.
