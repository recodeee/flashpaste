# `flashpaste-overlayd` — engineering plan & 30 agent prompts

> **Goal:** ship `flashpaste-overlayd` — a tiny Rust daemon that paints agent-driven
> annotations on a Wayland screen, with an MCP tool on the existing
> `flashpaste-mcp` server that drives it. Five primitives: rectangle, circle,
> arrow, label, clear. JSON over Unix socket. Fade-out animations. Cross-compositor
> (wlroots/KDE via layer-shell, GNOME via xdg-portal fallback).

This document is the canonical roadmap for the overlay work. It is the source the
codex-fleet executes against. **Do not paste all 30 prompts at once.** Each prompt
is a chunk the agent should complete and a human should verify before the next.

## Stack

All MIT/Apache, all studied from existing open-source repos:

- `smithay-client-toolkit` — Wayland client primitives (same crate wayscriber uses)
- `cairo-rs` + `pangocairo` — rendering and text (same as wayscriber)
- `wayland-protocols` for `zwlr_layer_shell_v1`
- `serde` + `serde_json` for the wire protocol
- `tokio` for the socket and animation tick
- `clap` for the CLI

## Reference repos (read, don't fork blindly)

- **[devmobasa/wayscriber](https://github.com/devmobasa/wayscriber)** — layer-shell setup and GNOME portal fallback
- **[bk138/gromit-mpx](https://github.com/bk138/gromit-mpx)** — X11 history and shape-rendering math
- **[wmww/gtk4-layer-shell](https://github.com/wmww/gtk4-layer-shell)** — simplest layer-surface lifecycle
- **flashpaste itself** — existing MCP server pattern and packaging conventions

## Phase map

| Phase | Prompts | Output |
| --- | --- | --- |
| Project setup, protocol spec, repo scaffold | 1–4 | `docs/overlay-protocol.md`, new crate `flashpaste-overlayd` building clean |
| Overlay daemon — layer shell, Cairo rendering, the five primitives | 5–10 | `--demo` renders + fades a red rect |
| IPC socket, message handling, animation/fade lifecycle | 11–14 | Full daemon driven by `nc -U` |
| MCP tool integration into `flashpaste-mcp` | 15–18 | `highlight_region`, `point_at`, `clear_annotations` tools live |
| GNOME portal fallback, multi-monitor, click-through correctness | 19–22 | Works on GNOME and dual-monitor |
| Tests, doctor checks, error paths | 23–26 | `flashpaste-doctor` reports 17 checks; integration tests |
| Packaging, docs, demo, release | 27–30 | `.deb`/`.rpm`, demo GIF, `v1.32` released, protocol proposal thread |

## Rules for the agent

1. Each prompt names exact files and crate names. Vague prompts produce vague code.
2. Each prompt has acceptance criteria so the agent knows when to stop.
3. Each prompt tells the agent what *not* to do — LLMs over-engineer when unconstrained.
4. Prompts that involve studying an external repo say "read X, summarize Y, then apply Z to our repo" — never "copy from X." MIT lets you learn freely; copying gets messy with attribution.
5. Build order: protocol → daemon scaffold → rendering → IPC → MCP integration → fallbacks → polish → ship.

---

## The 30 agent prompts

Copy these one at a time into your agent. Wait for each to complete and verify
before moving on. Don't batch them.

### Phase 1 — setup, protocol, scaffold

#### Prompt 1 — Read the reference repos

Clone `devmobasa/wayscriber`, `bk138/gromit-mpx`, and `wmww/gtk4-layer-shell`
into a `references/` directory at the root of the flashpaste repo (or wherever
you're working). Do not modify these clones. Read each `README.md` and the
top-level `src/` directory listing. Produce a single markdown file
`docs/overlay-references.md` summarizing for each repo: (a) what crates/libraries
it uses for Wayland layer-shell, (b) how it handles GNOME (which doesn't support
layer-shell), (c) how rendering is done (Cairo, OpenGL, etc.), (d) one concrete
pattern we should adopt and one we should avoid. Keep the file under 400 lines.
Do not copy code. This is a study pass only.

#### Prompt 2 — Write the wire protocol spec

Create `docs/overlay-protocol.md`. Define a JSON-over-Unix-socket protocol with
exactly five message types: `draw_rect`, `draw_circle`, `draw_arrow`,
`draw_label`, `clear`. Each message has fields: `id` (uuid v4 string), `ttl_ms`
(int, default 3000, max 30000), `color` (hex string, default `#ffae00`),
`stroke_width` (float pixels, default 2.0). Shape-specific fields: rect/circle
take `x, y, w, h`; arrow takes `x1, y1, x2, y2`; label takes `x, y, text` (max
200 chars). `clear` takes optional `id` to clear one shape, or no field to clear
all. Specify the response format: `{"ok": true, "id": "..."}` or `{"ok": false,
"error": "..."}`. Socket path: `$XDG_RUNTIME_DIR/flashpaste-overlay.sock`. One
JSON object per line, newline-delimited. Include three example messages and
three example responses. Do not add fields beyond what I listed.

#### Prompt 3 — Scaffold the Rust crate

Inside the existing flashpaste workspace (or as a new sibling crate to
`flashpaste-mcp`), create a new binary crate called `flashpaste-overlayd`. Add
it to the workspace `Cargo.toml`. The crate's `Cargo.toml` should declare
dependencies on `smithay-client-toolkit = "0.19"`, `wayland-protocols = "0.32"`,
`wayland-protocols-wlr = "0.3"`, `cairo-rs = "0.20"`, `pangocairo = "0.20"`,
`serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`,
`tokio = { version = "1", features = ["full"] }`,
`clap = { version = "4", features = ["derive"] }`, `anyhow = "1"`,
`tracing = "0.1"`, `tracing-subscriber = "0.3"`,
`uuid = { version = "1", features = ["v4"] }`. Add a minimal `src/main.rs` that
prints `flashpaste-overlayd 0.1.0` and exits. Run
`cargo check -p flashpaste-overlayd` and confirm it builds clean. Report any
unresolved dependency versions and pick the next-latest compatible version.

#### Prompt 4 — Define the message types in Rust

Create `src/protocol.rs` in `flashpaste-overlayd`. Implement
`#[derive(Serialize, Deserialize)]` enums and structs that exactly match the
JSON spec in `docs/overlay-protocol.md`. Use
`#[serde(tag = "type", rename_all = "snake_case")]` for the message enum. Use
`uuid::Uuid` for ids. Default values via `#[serde(default)]` and helper fns. Add
a `Color` newtype that parses `#rrggbb` and `#rrggbbaa` hex strings into an
`(r, g, b, a)` tuple of f64s in 0.0–1.0 range, with an `impl Default` returning
`#ffae00`. Write unit tests in the same file that round-trip each message type
through `serde_json` and verify defaults apply. Run
`cargo test -p flashpaste-overlayd` and confirm all tests pass.

### Phase 2 — overlay daemon: layer shell + Cairo

#### Prompt 5 — Study wayscriber's layer-shell setup

Read `references/wayscriber/src/` and identify the files where it (a) creates
the Wayland connection, (b) configures the `zwlr_layer_surface_v1` (anchor,
layer, keyboard interactivity, exclusive zone), (c) sets up the input region
for click-through, (d) handles the configure/ack cycle. Write a 1-page summary
as `docs/layer-shell-setup-notes.md` that lists the function names and a
one-line description of each. Do not copy their code; this is for our own
reference. Identify any non-obvious details (e.g. why they pick a particular
layer, how they handle output enumeration).

#### Prompt 6 — Implement the layer-shell surface

Create `src/surface.rs`. Implement a `LayerSurface` struct that on construction
(a) connects to the Wayland display, (b) creates a `wl_surface`, (c) wraps it
as a `zwlr_layer_surface_v1` on the `Overlay` layer anchored to all four
edges, (d) sets keyboard-interactivity to `None`, (e) sets the input region to
empty (a zero-sized `wl_region` attached as input region) so the surface is
click-through, (f) commits and waits for the first `configure` event. Use
`smithay-client-toolkit` patterns. If `zwlr_layer_shell_v1` is unavailable on
the compositor, return a clear error like
`LayerShellUnavailable { compositor_hint: String }`. Add a `main.rs` flag
`--probe` that just creates the surface and exits cleanly, reporting success
or the specific error. Test by running `flashpaste-overlayd --probe` on the
user's compositor. Expect success on Hyprland/Sway/KDE, expect
`LayerShellUnavailable` on GNOME.

#### Prompt 7 — Implement the Cairo render context

Create `src/render.rs`. Implement a `RenderCtx` that owns a Cairo `ImageSurface`
of size matching the wl_surface, and exposes methods
`draw_rect(rect: &RectShape)`, `draw_circle(circle: &CircleShape)`,
`draw_arrow(arrow: &ArrowShape)`, `draw_label(label: &LabelShape)`,
`clear_all()`. Each draw method takes the shape struct from `protocol.rs`,
applies stroke color, stroke width, and renders to the Cairo surface. The arrow
should be a line with a triangular arrowhead at `(x2, y2)`, head length = 12px,
head angle = 25 degrees. The label uses `pangocairo` with Anthropic-style
typography defaults (system sans-serif 14px). Add a `current_opacity: f64`
field on each shape used to multiply alpha during fade-out. Write unit tests
that render each shape to an in-memory surface and assert non-empty pixels are
present in the expected bounding box. Do not wire this to Wayland yet —
render-only.

#### Prompt 8 — Connect the renderer to the wl_surface

Wire `RenderCtx` to the `LayerSurface` from prompt 6. On each draw call, the
Cairo `ImageSurface` data should be copied into a `wl_shm` buffer attached to
the `wl_surface`, then `commit()` damages the whole surface. Use
`smithay-client-toolkit`'s `SlotPool` for buffer management — read its docs in
`references/` to confirm the API. Add a quick-test mode
`flashpaste-overlayd --demo` that opens the surface, draws one red 200×100
rectangle at (400, 300), holds it for 3 seconds, then exits. Run it on a
Wayland session and visually confirm a red rectangle appears in the middle of
the screen for 3 seconds, click-through (clicks pass to the window below).

#### Prompt 9 — Implement the shape store and animation tick

Create `src/store.rs`. Implement `ShapeStore` as a `Vec<StoredShape>` where
`StoredShape` contains the protocol shape, `created_at: Instant`, and
`ttl_ms: u32`. Methods: `add(shape)`, `remove(id: Uuid)`, `clear()`,
`tick() -> NeedsRedraw`. The `tick` method (a) removes expired shapes whose
`created_at + ttl_ms` is past, (b) computes `current_opacity` for each shape:
full opacity until the last 400ms of life, then linear fade to zero. Return a
`bool` indicating whether redraw is needed. The store is `Send + Sync` so it
can be shared across the IPC thread and the render thread. Use
`Arc<Mutex<...>>` for sharing. Unit-test the fade math: a shape with
ttl=1000ms should have opacity 1.0 at t=500ms and ~0.5 at t=800ms.

#### Prompt 10 — Animation loop

In `main.rs`, set up a 60Hz tick that calls `store.tick()`. If `NeedsRedraw`
is true, re-render all shapes in the store to the Cairo surface (clear, then
redraw each shape with its current_opacity multiplied into the color alpha)
and commit the wl_surface. Use `tokio::time::interval` at 16ms. Combine the
demo from prompt 8 with this loop: now `--demo` should fade the rectangle out
smoothly in the last 400ms of its 3-second life. Visually verify the fade is
smooth (no stepping, no flicker). If it flickers, the issue is
double-buffering — switch to two `wl_shm` buffers swapping each frame.

### Phase 3 — IPC + message handling

#### Prompt 11 — Implement the Unix socket listener

Create `src/ipc.rs`. Open a Unix socket at
`$XDG_RUNTIME_DIR/flashpaste-overlay.sock` (fall back to `/tmp` if env unset,
log a warning). Set permissions to 0600 (owner only). Accept connections; for
each connection, read newline-delimited JSON, parse with `serde_json` against
the `Message` enum from `protocol.rs`. For each parsed message: (a) `draw_*`
messages → call `store.add(...)`, respond `{"ok": true, "id": "<uuid>"}`,
(b) `clear` with id → `store.remove(id)`, (c) `clear` without id →
`store.clear()`. On parse error respond `{"ok": false, "error": "<message>"}`
and keep the connection open. Use `tokio`'s async UnixListener. Add integration
test: spawn the daemon, connect with a test client, send one `draw_rect`,
assert response is `ok: true`, assert the store contains one shape.

#### Prompt 12 — Wire IPC + store + render loop together

The daemon now has three concurrent parts: the IPC listener (writes to store),
the animation tick (reads store, redraws), the Wayland event loop (handles
configure events, output changes). Set up `tokio::select!` or separate tasks
with `Arc<Mutex<ShapeStore>>` shared between them. The Wayland connection is
not Send by default with smithay-client-toolkit — keep all Wayland calls on a
single thread, send only `RedrawRequest` messages from the tick task via an
mpsc channel. Document the threading model in a doc comment at the top of
`main.rs`. Run the full daemon (no `--demo`), then in another terminal send a
JSON message via `nc -U $XDG_RUNTIME_DIR/flashpaste-overlay.sock`. Verify a
shape appears on screen and fades after the ttl expires.

#### Prompt 13 — Build a CLI client for testing

Add a second binary to the crate: `flashpaste-overlay` (the human/scripting
CLI, distinct from `flashpaste-overlayd` the daemon). Use `clap` derive.
Subcommands: `rect --x --y --w --h [--color] [--ttl-ms]`,
`circle --x --y --w --h [...]`,
`arrow --x1 --y1 --x2 --y2 [...]`,
`label --x --y --text [...]`,
`clear [--id ID]`. Each subcommand builds the JSON message, connects to the
socket, sends it, prints the response. Exit code 0 on `ok: true`, 1 otherwise.
Test all five subcommands manually. This is the tool a shell script (or human)
uses; the agent path will be via MCP.

#### Prompt 14 — Document the protocol and CLI in the repo

Update `docs/overlay-protocol.md` with the final API as implemented. Create
`docs/overlay-cli.md` showing every `flashpaste-overlay` subcommand with an
example. Add a section "Why not gromit-mpx or wayscriber" linking to those
projects, crediting them, and explaining the design difference (programmatic
API vs human-driven). Make sure `LICENSE` notes are present and we credit the
smithay-client-toolkit and Cairo authors in `THIRD_PARTY.md`.

### Phase 4 — MCP integration

#### Prompt 15 — Read the existing flashpaste-mcp code

Read `rs/` (or wherever the MCP server lives in the flashpaste repo). Identify
(a) where tools are registered, (b) the `take_screenshot`, `read_clipboard`,
`copy_text`, `paste_to_pane` definitions, (c) the JSON schema format the
existing tools use. Summarize the patterns in `docs/mcp-tool-patterns.md`. We
will add new tools that follow exactly the same patterns — uniformity matters.

#### Prompt 16 — Add the `highlight_region` MCP tool

Add a new tool `highlight_region` to `flashpaste-mcp` following the patterns
documented in prompt 15. Input schema:
`{ shape: "rect"|"circle", x: int, y: int, w: int, h: int, color?: string, label?: string, ttl_ms?: int }`.
Implementation: open the `$XDG_RUNTIME_DIR/flashpaste-overlay.sock`, send the
corresponding JSON message, return the response. If the socket doesn't exist,
return a structured error suggesting the user install/start
`flashpaste-overlayd`. Add a description string optimized for LLM consumption:
"Draw a temporary highlight box on the user's screen. Use this when pointing
the user to a specific area of their visible UI that doesn't have a text
address (a button, a panel, a region of a running app). Prefer this over
describing the location in words for UI elements."

#### Prompt 17 — Add the `point_at` MCP tool

Add a second tool `point_at` for arrows specifically. Input:
`{ from_x, from_y, to_x, to_y, color?, label?, ttl_ms? }`. Same socket
plumbing, sends a `draw_arrow` message. Tool description: "Draw an arrow on
the user's screen pointing from one location to another. Use when explaining
direction, flow, or 'this comes from that'." Add a `clear_annotations` tool
(no params) that sends `clear` with no id.

#### Prompt 18 — Test the round-trip

Start `flashpaste-overlayd`, then start `flashpaste-mcp`, then have Claude
Code (or any MCP client) call
`highlight_region(shape="rect", x=200, y=200, w=300, h=100, color="#ff0000", label="this", ttl_ms=4000)`.
Confirm a red rectangle with the label appears on screen for 4 seconds and
fades. Document the test workflow in `docs/overlay-quickstart.md`.

### Phase 5 — fallbacks, multi-monitor, click-through

#### Prompt 19 — GNOME portal fallback design

Read `references/wayscriber/` for how it falls back on GNOME (where
`zwlr_layer_shell_v1` is absent). Note that GNOME mutter does not implement
layer-shell and won't. Write `docs/gnome-fallback.md` documenting the plan: a
regular `wl_surface` window with no decorations, transparent background,
always-on-top via `xdg_toplevel` hints, positioned to cover the full output,
click-through via empty input region. Acknowledge the limitations: z-order
against fullscreen apps not guaranteed, possible flicker on enter/leave,
requires the user to permit it once via portal. Do not implement yet — this is
the design doc.

#### Prompt 20 — Implement the GNOME fallback

In `src/surface.rs`, when `LayerShellUnavailable` is returned, automatically
try the `XdgToplevelFallback` path designed in prompt 19. Create a
fullscreen-sized `xdg_toplevel`, set `app_id = "com.recodeee.flashpaste.overlay"`,
decorations off, transparent. Cover the primary output. On GNOME, this will
produce a window the user may need to grant permission for the first time. Add
a `--force-fallback` flag for testing. Verify on GNOME 46+: red rectangle
appears via the fallback path, with the documented caveats.

#### Prompt 21 — Multi-monitor handling

Enumerate `wl_output`s. For each output, optionally create a separate layer
surface (or fallback window) so annotations can span multiple monitors. Add a
`--outputs primary|all|<name>` flag, default `primary`. The protocol stays the
same: coordinates are global screen-space (or per-output, document the choice).
I recommend global coordinates with the daemon routing each shape to the
correct output(s) it intersects. Test with two monitors connected: draw a
rectangle that spans the seam, confirm both halves render correctly.

#### Prompt 22 — Click-through correctness sweep

Write `tests/click-through.md` describing a manual test matrix: with the
overlay active, verify that (a) clicks in a terminal still focus the terminal,
(b) clicks on a browser still register on the page, (c) keyboard input is
never captured by the overlay, (d) the overlay does not appear in
window-switcher (alt-tab), (e) screen recorders/screenshot tools either include
or exclude the overlay depending on whether they capture compositor output or
specific windows. Document each result on the user's compositor.

### Phase 6 — tests, doctor, resilience

#### Prompt 23 — Integration tests

Create `tests/integration.rs`. Spawn `flashpaste-overlayd` in a subprocess
(use `tokio::process`), wait for socket to exist, send a sequence of messages
via the CLI client, parse responses. Test cases: (a) draw a rect, query store,
assert one shape exists; (b) draw five shapes, clear one by id, four remain;
(c) draw a shape with ttl=500ms, wait 700ms, store empty; (d) send malformed
JSON, get error response, socket still alive; (e) send 1000 messages rapidly,
no panics, all processed. CI-friendly: use a virtual Wayland compositor like
`cage` or `weston` headless if available, otherwise skip the visual tests
behind a `#[cfg(feature = "visual-tests")]`.

#### Prompt 24 — `flashpaste-doctor` integration

Add a new check to the existing `flashpaste-doctor` script. The check: (a) is
`flashpaste-overlayd` installed and on PATH, (b) is the daemon running (socket
exists), (c) is the compositor layer-shell capable or falling back to portal,
(d) can a test `draw_rect` ttl=100ms round-trip successfully. Report each as
✅/⚠️/❌ with a remediation hint. Add it to the existing 13 green checks count;
we should now have 17 checks.

#### Prompt 25 — Error paths and resilience

Audit `flashpaste-overlayd` for crash scenarios: compositor disconnect, socket
EPIPE, malformed messages, out-of-memory on huge shape lists, ttl_ms > max,
label text with control characters, coordinate overflow. For each, the daemon
should either log a warning and continue or exit cleanly with a useful error
message — never panic. Add a shape limit: max 100 simultaneous shapes; on
overflow, oldest non-expired shape is evicted. Write tests for each error case.

#### Prompt 26 — Performance check

Measure: what's the latency from MCP `highlight_region` call to pixels-on-screen?
Goal: under 50ms p99 on a modern laptop. Profile with `tracing` spans on the
socket-receive, store-update, render, commit path. If we're slower than that,
identify the slowest span and optimize. Document results in
`docs/overlay-performance.md`. Bonus: measure idle CPU when zero shapes are
active — should be 0% (no tick when nothing to animate).

### Phase 7 — packaging, docs, demo, release

#### Prompt 27 — Packaging: `.deb` and `.rpm`

Following the existing flashpaste packaging conventions in `packaging/`, add
`flashpaste-overlayd` and `flashpaste-overlay` to the package outputs. Add a
systemd user unit `systemd/flashpaste-overlayd.service` that runs the daemon
on login, `Type=notify`, `Restart=on-failure`. Update the AUR PKGBUILD. Update
the bootstrap.sh installer to enable the new service. Test that
`apt install ./flashpaste_all.deb` installs the overlay daemon, the service
starts, and `flashpaste-doctor` reports green.

#### Prompt 28 — README and demo GIF

Update the main README. Add a new section "Agent-driven screen annotation"
with: a one-paragraph pitch, a 3-line MCP example, a link to
`docs/overlay-quickstart.md`. Record a demo GIF: user pastes a screenshot
into Claude Code, Claude responds "the bug is in this function" and a red box
appears around the relevant code, the user clicks straight there. The GIF
should be under 5MB, 10–15 seconds, embedded in the README right after the
existing hero image. Use `peek` or `wf-recorder` + `gifski` for capture.

#### Prompt 29 — `llms.txt` and Schema.org update

Update `llms.txt` and the JSON-LD `SoftwareApplication` block in the README to
mention the new overlay capability and the two new MCP tools
(`highlight_region`, `point_at`, `clear_annotations`). Add keywords:
`screen annotation`, `agent overlay`, `mcp screen pointer`, `linux ai overlay`.
This is what makes the new capability discoverable to AI search and to other
agents browsing for tools.

#### Prompt 30 — Release v1.32

Bump version to 1.32 across `Cargo.toml`, `packaging/`, `CHANGELOG.md`. Write
the changelog entry following Keep-a-Changelog format. Sections: Added (the
overlay daemon, three MCP tools, GNOME fallback, multi-monitor). Changed
(doctor checks went from 13 to 17). Notes (compositor support matrix, fallback
caveats on GNOME). Tag `v1.32`, push, let the GitHub Actions release workflow
build the binaries. Post a short release note. After release, open a discussion
thread titled **"Wayland Overlay Annotation Protocol — proposal for a small
cross-tool spec"** linking to `docs/overlay-protocol.md` and inviting
Gromit-MPX and Wayscriber maintainers to comment.

---

## Operator notes

**Don't paste all 30 at once.** Each prompt is a chunk the agent should
complete and you should verify before the next. Skipping verification is how
you end up at prompt 18 wondering why nothing works because something subtle
broke at prompt 9.

**Some prompts will produce "I don't know" or wrong code.** Especially the
smithay-client-toolkit ones — that crate has changed APIs between 0.18 and
0.19 and the agent's training data may be older. When the agent fails, paste
the actual compile error back and ask it to look at
`references/wayscriber/Cargo.toml` to see which version is in use and adapt.

**The hardest prompt is probably 8 (Cairo → wl_shm → commit).** If the agent
stalls, point it at wayscriber's actual buffer code and ask "produce the
equivalent for our crate." This is the one place the right move is to mimic
working production code closely, with attribution.

**The "study reference repos" prompts (1, 5, 15, 19) are not optional.**
They're the prompts that make the agent stop hallucinating APIs and start
writing code that matches reality. Without them, you'll get 1000 lines of
beautifully-formatted Rust that doesn't compile against actual
smithay-client-toolkit 0.19.

**Prompt 30 is the move that turns FlashPaste from "tool" into "protocol
owner."** That discussion thread is the cheapest, highest-leverage thing in
this whole list. Ship the reference implementation, propose the standard, let
others adopt.

**Total time estimate:** with a competent agent and you verifying each step,
this is a 2–3 week project, not a 2–3 day one. The Wayland and Cairo prompts
especially will take real iteration. Be patient with steps 5–10; they're the
ones that determine whether the final thing works at all.
