# Overlay Performance

Goal: MCP `highlight_region` call to visible annotation under 50 ms p99 on a modern laptop.

## Status On This Machine

Full MCP-to-pixels latency was not measurable in this Codex sandbox on 2026-05-20.

Two local blockers prevented a real pixel-path run:

- `cargo run --manifest-path rs/Cargo.toml -p flashpaste-overlayd --features wayland --bin flashpaste-overlayd -- --probe` could not build the render path because `cairo-sys-rs` could not find `cairo.pc` through `pkg-config`.
- Standalone Unix-domain socket probes failed with `Operation not permitted`; the ignored performance test therefore reports `skipping performance probe: Unix listener bind denied by environment`.

No p99-to-pixels claim is made from this environment. The 50 ms target still needs to be verified on a desktop session with Cairo development files installed and a live Wayland compositor.

## Instrumentation Added

`flashpaste-overlayd` now emits tracing spans around the path that matters:

- `socket_receive` - read one newline-delimited JSON request from the overlay socket.
- `socket_parse` - parse request JSON into the overlay protocol enum.
- `store_update` - mutate the annotation store for draw or clear requests.
- `render_store` - top-level render pass.
- `surface_dispatch` and `wayland_dispatch` - process pending Wayland events before rendering or committing.
- `store_snapshot` - lock and snapshot the store for rendering.
- `render` and `render_cairo` - clear the Cairo surface and draw active shapes.
- `commit`, `commit_wayland`, `commit_dispatch`, `commit_buffer`, `commit_copy`, and `commit_attach_damage_flush` - copy Cairo pixels into wl_shm, attach/damage the surface, commit, and flush Wayland.

The tracing subscriber is configured with span close events, so running with `RUST_LOG=flashpaste_overlayd=debug` reports per-span busy/idle timing.

## Optimization Applied

The pre-check daemon path had an avoidable scheduling floor:

- New shapes were noticed by a 16 ms animation tick instead of waking render immediately.
- The Wayland loop polled every 8 ms even when there were no active shapes.

The daemon now sends a redraw notification directly from IPC store updates. The render loop consumes that notification immediately, drains duplicate redraw requests, and only enables animation sleeps plus Wayland polling while at least one shape exists. When the store is empty, there is no animation tick and no Wayland poll branch.

The hidden headless test mode uses the same wake-driven behavior, so idle CPU probes exercise the no-active-shapes path instead of the old fixed 60 Hz tick.

## Runnable Probe

Build the MCP server and overlay daemon first:

```bash
env CARGO_BUILD_RUSTC_WRAPPER= cargo build --manifest-path rs/Cargo.toml \
  -p flashpaste-overlayd -p flashpaste-mcp --offline --bins
```

Run the headless MCP-to-overlay latency probe:

```bash
env CARGO_BUILD_RUSTC_WRAPPER= cargo test --manifest-path rs/Cargo.toml \
  -p flashpaste-overlayd --offline --test performance -- --ignored --nocapture
```

The probe sends 20 warmup calls and 500 measured MCP `highlight_region` calls through `flashpaste-mcp` into a headless `flashpaste-overlayd`, then prints:

```json
{
  "samples": 500,
  "p50_ms": 0.0,
  "p95_ms": 0.0,
  "p99_ms": 0.0,
  "max_ms": 0.0,
  "mean_ms": 0.0,
  "idle_cpu_percent_3s": 0.0
}
```

The values above are the output shape, not this machine's result. In the current sandbox the probe skips before measurement because Unix socket bind is denied.

## Full Pixel-Path Procedure

On a real desktop session:

1. Install Cairo development files so `pkg-config --cflags --libs cairo` succeeds.
2. Confirm a compositor path works:

   ```bash
   env CARGO_BUILD_RUSTC_WRAPPER= cargo run --manifest-path rs/Cargo.toml \
     -p flashpaste-overlayd --features wayland --bin flashpaste-overlayd -- --probe
   ```

3. Start the daemon with tracing:

   ```bash
   RUST_LOG=flashpaste_overlayd=debug rs/target/debug/flashpaste-overlayd
   ```

4. Start `flashpaste-mcp` with the same `XDG_RUNTIME_DIR` and call `highlight_region`.
5. Treat `commit_attach_damage_flush` close time as the last software boundary. For actual pixels-on-screen, pair the trace with compositor-level visual confirmation or a screen capture timestamp.

Passing criteria:

- MCP call-to-commit p99 under 50 ms.
- No single span dominates p99; if one does, optimize that span first.
- Idle CPU after `clear_annotations` and no active shapes rounds to 0% over a multi-second sample.
