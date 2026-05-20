# Click-Through Correctness Manual Test Matrix

Date prepared: 2026-05-20

Detected compositor for this machine:

```text
Session: Wayland
Compositor: GNOME Shell
XWayland: present
```

Source for detection:
- `loginctl show-session 3 -p Type -p State` reported `Type=wayland` and `State=active`.
- Host process list showed `gnome-shell` and `Xwayland :1`.

## Goal

Verify that the flashpaste overlay behaves like a visual overlay, not an input owner:

- Pointer clicks pass through to applications below the overlay.
- Keyboard input is never captured by the overlay.
- The overlay is not presented as an application window.
- Screen capture behavior is understood for compositor-output capture versus window-specific capture.

## Setup

1. Start a terminal window.
2. Start a browser window with a page that has an obvious clickable control, such as a text input, link, or button.
3. Activate the flashpaste overlay using the normal shortcut or command for this build.
4. Keep the overlay visible for all cases unless a case says otherwise.
5. Record the result in the `GNOME Shell Wayland result` column.

Use the same overlay build for every row. If behavior changes after a restart, record the restart and rerun all rows.

## Matrix

| ID | Case | Steps | Expected result | GNOME Shell Wayland result |
| --- | --- | --- | --- | --- |
| CT-01 | Terminal click focuses terminal | With the overlay active, click inside an unfocused terminal. Type `focus-check` after the click. | The terminal receives focus and the typed text appears in the terminal. The overlay does not intercept the click or consume keyboard input. | Not run yet. |
| CT-02 | Terminal click preserves normal selection behavior | With the overlay active, drag across text in the terminal. | The terminal selection changes as it would without the overlay. No overlay drag state starts. | Not run yet. |
| CT-03 | Browser click registers on page | With the overlay active, click a visible browser page button or link. | The page receives the click and performs its normal action. The browser becomes focused if it was not already focused. | Not run yet. |
| CT-04 | Browser text input receives keyboard input | With the overlay active, click a browser text field, then type `flashpaste-input-check`. | The text appears in the browser field. The overlay does not receive or display typed characters. | Not run yet. |
| CT-05 | Keyboard input is never captured by overlay | With the overlay active and a terminal focused, type normal printable text, arrows, Backspace, Enter, and Ctrl+C in a harmless prompt or editor. Repeat with the browser text field focused. | All key events go to the focused application. The overlay does not focus itself, open text UI, trigger shortcuts, or suppress normal application input. | Not run yet. |
| CT-06 | Alt-Tab excludes overlay | With the overlay active, press Alt+Tab and cycle through visible windows. | The overlay is not listed as an Alt-Tab target. Only real application windows appear. | Not run yet. |
| CT-07 | Overview/window switcher excludes overlay | With the overlay active, open GNOME Overview. | The overlay is not shown as a selectable application/window thumbnail. Real windows remain selectable. | Not run yet. |
| CT-08 | Window-specific screenshot excludes overlay | With the overlay active, use a screenshot tool mode that captures a specific window, then capture the terminal or browser window. | The captured image contains the selected application window. It should not include the overlay unless the tool captures compositor output instead of the chosen window. | Not run yet. |
| CT-09 | Full-screen/compositor-output screenshot behavior is classified | With the overlay active, use a screenshot mode that captures the full compositor output. | Record whether the overlay appears. If it appears, classify this tool as compositor-output capture. If it does not, classify it as window/source capture or compositor-filtered capture. | Not run yet. |
| CT-10 | Screen recorder behavior is classified | With the overlay active, record a short clip using the user's normal recorder. Test full-screen/output capture and window capture if both modes exist. | Output/full-screen capture may include the overlay. Window-specific capture should exclude it unless the recorder composites overlays into the selected source. Record the exact tool and mode. | Not run yet. |
| CT-11 | Overlay deactivation restores identical app input | Deactivate the overlay and repeat one terminal click/type and one browser click/type. | Behavior should match the overlay-active results except the overlay is no longer visible. Any input difference indicates the overlay affected pass-through behavior. | Not run yet. |

## Result Notes

Use this section during the manual run.

```text
Overlay command/build:
Terminal app:
Browser:
Screenshot tool and mode:
Screen recorder and mode:

CT-01:
CT-02:
CT-03:
CT-04:
CT-05:
CT-06:
CT-07:
CT-08:
CT-09:
CT-10:
CT-11:
```

## Pass Criteria

The click-through sweep passes on GNOME Shell Wayland only if:

- CT-01 through CT-05 pass without the overlay taking focus or keyboard input.
- CT-06 and CT-07 confirm the overlay is absent from window switching surfaces.
- CT-08 through CT-10 record capture behavior by tool and mode without treating inclusion in compositor-output capture as a click-through failure.
- CT-11 shows no input behavior regression after the overlay is deactivated.

## Failure Triage

If a row fails, record:

- Whether the overlay was using a layer-shell path, xdg fallback, XWayland, or another backend.
- The exact compositor/session from `loginctl show-session`.
- The focused window before and after the click.
- Whether the failure affects pointer input, keyboard input, window identity, or capture semantics.
- Whether the failure reproduces after restarting the overlay.
