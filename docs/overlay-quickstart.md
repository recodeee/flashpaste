# FlashPaste Overlay Quickstart

This quickstart verifies the `flashpaste-mcp` to `flashpaste-overlayd` path for
temporary screen annotations.

## Requirements

- A compositor that advertises `zwlr_layer_shell_v1`, such as Hyprland, Sway,
  wlroots compositors, or KDE Plasma.
- `flashpaste-overlayd` built with Wayland and render support.
- Cairo and PangoCairo development files available at build time. On Debian or
  Ubuntu, install the packages that provide `cairo.pc` and `pangocairo.pc`
  before building the render feature.
- `flashpaste-mcp` built from the same checkout.

GNOME/Mutter does not advertise `zwlr_layer_shell_v1`. The current overlay
daemon is expected to fail there until a GNOME fallback exists.

## Build From Source

```bash
cargo build --manifest-path rs/Cargo.toml -p flashpaste-overlayd --features render
cargo build --manifest-path rs/Cargo.toml -p flashpaste-mcp
```

If your shell is not attached to the graphical session, set the runtime
environment explicitly:

```bash
export XDG_RUNTIME_DIR=/run/user/$(id -u)
export WAYLAND_DISPLAY=wayland-0
```

## Probe The Compositor

Run this before starting the daemon:

```bash
rs/target/debug/flashpaste-overlayd --probe
```

Expected success on a layer-shell compositor:

```text
flashpaste-overlayd --probe: layer-shell surface OK (configured_size=(...))
```

Expected failure on GNOME/Mutter:

```text
flashpaste-overlayd --probe: LayerShellUnavailable { compositor_hint: "Compositor did not advertise zwlr_layer_shell_v1. GNOME/Mutter is expected to fail here; Hyprland, Sway, wlroots compositors, and KDE Plasma should expose layer-shell." }
```

## Start The Overlay Daemon

In one terminal:

```bash
rs/target/debug/flashpaste-overlayd
```

The daemon binds:

```text
$XDG_RUNTIME_DIR/flashpaste-overlay.sock
```

Leave it running while you call the MCP tool.

## Start `flashpaste-mcp`

An MCP client such as Claude Code should launch:

```bash
rs/target/debug/flashpaste-mcp
```

The server speaks JSON-RPC 2.0 over stdio. Logs go to stderr; JSON-RPC responses
go to stdout.

## Call `highlight_region`

Use Claude Code or any MCP client to call:

```text
highlight_region(shape="rect", x=200, y=200, w=300, h=100, color="#ff0000", label="this", ttl_ms=4000)
```

Equivalent raw JSON-RPC request:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"highlight_region","arguments":{"shape":"rect","x":200,"y":200,"w":300,"h":100,"color":"#ff0000","label":"this","ttl_ms":4000}}}
```

Expected tool result:

```json
{"structuredContent":{"ok":true,"responses":[{"ok":true,"id":"..."},{"ok":true,"id":"..."}]}}
```

`flashpaste-mcp` sends two overlay protocol messages: one `draw_rect` and one
`draw_label`.

## Visual Confirmation

On a supported compositor, confirm:

- A red rectangle appears at `x=200`, `y=200`.
- The rectangle is `300` pixels wide and `100` pixels high.
- The label `this` appears at the rectangle's top-left anchor.
- The annotation remains visible for roughly 4 seconds.
- It fades during the final 400 ms, then disappears.

## Local Test Notes

On this machine on 2026-05-20:

- `flashpaste-overlayd --probe` reached the live Wayland socket but failed with
  `LayerShellUnavailable` because the session is GNOME/Mutter.
- Building `flashpaste-overlayd --features render` failed because
  `pkg-config` could not find `cairo.pc`.
- The MCP wire round-trip was verified with a temporary Unix-socket harness.
  `flashpaste-mcp` returned `ok: true` after sending these two JSON messages:

```json
{"color":"#ff0000","h":100,"id":"...","ttl_ms":4000,"type":"draw_rect","w":300,"x":200,"y":200}
{"color":"#ff0000","id":"...","text":"this","ttl_ms":4000,"type":"draw_label","x":200,"y":200}
```

That proves the MCP client constructs and sends the expected overlay requests,
but it is not a pixel-on-screen confirmation. Pixel confirmation requires the
render feature dependencies and a compositor with layer-shell support.

## Record The Demo GIF

The README embed at `assets/overlay-demo.gif` is captured once per release. The
scenario is interactive — a human at the keyboard pastes a screenshot into
Claude Code, Claude responds with `highlight_region`, a red box appears around
the relevant code, and the user clicks straight to it. Target: 10–15 s, under
5 MB.

**Prerequisites:**

- A compositor that advertises `zwlr_layer_shell_v1` — Sway, Hyprland, KDE
  Plasma 6, or any wlroots compositor. The GNOME fallback design lives in
  [`gnome-fallback.md`](gnome-fallback.md); until it ships, GNOME sessions
  cannot record this demo.
- `wf-recorder` for screen capture (`apt install wf-recorder` or
  equivalent).
- `gifski` for the WebM → GIF conversion at a usable size
  (`cargo install gifski` if your distro does not package it). `ffmpeg`
  works as a fallback but produces noticeably larger files for the same
  visual quality.
- `flashpaste-overlayd` running with `--features render`.
- An MCP-aware Claude Code session pointed at `flashpaste-mcp`.

**Capture (wf-recorder → WebM → gifski → GIF):**

```bash
# 1. Pick the output to record. `wlr-randr` or `swaymsg -t get_outputs` lists them.
OUT=eDP-1

# 2. Start recording. Keep it tight: framerate 24, no audio, output to /tmp.
wf-recorder -o "$OUT" -f /tmp/overlay-demo.webm -r 24 --no-audio

# 3. Run the scenario in Claude Code:
#    a. Paste a code screenshot with flashpaste's right-click paste.
#    b. Ask: "where is the bug?"
#    c. Claude calls highlight_region → red box appears.
#    d. Click into the editor at that location.
#    Aim for 12 s start-to-finish, then press Ctrl-C in the wf-recorder terminal.

# 4. Convert. 12 fps and width 960 hits the 5 MB budget on typical clips.
ffmpeg -i /tmp/overlay-demo.webm -vf "fps=12,scale=960:-1:flags=lanczos" \
  -f yuv4mpegpipe - | gifski --fps 12 --quality 85 -o assets/overlay-demo.gif -

# 5. Verify size and length.
ls -lh assets/overlay-demo.gif
ffprobe -v error -show_entries format=duration /tmp/overlay-demo.webm
```

**`peek` alternative.** If `peek` is installed, it captures and converts in
one step — start `peek`, frame the recording rectangle over the relevant
windows, click record, perform the scenario, click stop, and save as
`assets/overlay-demo.gif`. `peek` does not let you tune fps/quality as
precisely as the `wf-recorder` → `gifski` pipeline, but it is fewer steps.

**ffmpeg-only fallback** (no `gifski` available):

```bash
ffmpeg -i /tmp/overlay-demo.webm \
  -vf "fps=12,scale=960:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer:bayer_scale=5" \
  -loop 0 assets/overlay-demo.gif
```

Expect this output to be ~20–30 % larger than the gifski version at the same
visual quality.

**Embed.** After the file lands in `assets/overlay-demo.gif`, uncomment the
`<img src="assets/overlay-demo.gif" …>` tag in the README hero block. Do not
commit the `.webm` source — `assets/overlay-demo.gif` is the only artifact the
repo carries.
