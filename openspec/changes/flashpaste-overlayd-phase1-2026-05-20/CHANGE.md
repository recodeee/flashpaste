---
base_root_hash: missing-spec-root
slug: flashpaste-overlayd-phase1-2026-05-20
---

# CHANGE · flashpaste-overlayd-phase1-2026-05-20

## §P  proposal
# flashpaste-overlayd Phase 1 — references, protocol spec, crate scaffold, Rust message types

## Problem

Ship a tiny Rust daemon (flashpaste-overlayd) that paints agent-driven annotations on a Wayland screen, with new MCP tools on flashpaste-mcp that drive it. Phase 1 lays the foundation: study reference implementations (wayscriber, gromit-mpx, gtk4-layer-shell), pin down the JSON-over-unix-socket wire protocol, scaffold the new Rust crate inside the existing flashpaste workspace, and implement the protocol types in Rust with round-trip serde tests. Phases 2-7 (rendering, IPC, MCP wiring, fallbacks, tests, release) follow in later plans. Source of truth: docs/flashpaste-overlayd-plan.md in the flashpaste repo.

## Acceptance criteria

- docs/overlay-references.md exists, summarizes wayscriber/gromit-mpx/gtk4-layer-shell across the four required dimensions, is under 400 lines, lints clean.
- docs/overlay-protocol.md fully specifies the five-message wire protocol with three example messages, three example responses, and the canonical socket path under $XDG_RUNTIME_DIR.
- rs/flashpaste-overlayd/ exists as a binary crate, registered in the rs/ workspace, with all required dependencies pinned and 'cargo check -p flashpaste-overlayd' succeeding clean.
- rs/flashpaste-overlayd/src/protocol.rs implements serde Serialize/Deserialize for every message in the spec with default-applying helpers, a Color newtype parsing #rrggbb/#rrggbbaa, and passing #[cfg(test)] round-trip tests; 'cargo test -p flashpaste-overlayd' is green.
- All four sub-task branches merge to main and the plan archives without an outstanding BLOCKED line in any tasks.md.

## Sub-tasks

### Sub-task 0: Prompt 1 — Read reference repos and write docs/overlay-references.md

Clone devmobasa/wayscriber, bk138/gromit-mpx, and wmww/gtk4-layer-shell into a references/ directory at the flashpaste repo root (add references/ to .gitignore). Do not modify these clones. Read each README.md and the top-level src/ directory listing. Produce docs/overlay-references.md summarizing for each repo: (a) crates/libraries used for Wayland layer-shell, (b) how GNOME (no layer-shell) is handled, (c) how rendering is done (Cairo, OpenGL, etc.), (d) one concrete pattern to adopt and one to avoid. Keep under 400 lines. Do not copy code. Acceptance: docs/overlay-references.md exists, lints clean with markdownlint, mentions all three repos. See docs/flashpaste-overlayd-plan.md Prompt 1 for the full prompt.

File scope: docs/overlay-references.md, .gitignore

### Sub-task 1: Prompt 2 — Write JSON-over-unix-socket wire protocol spec

Create docs/overlay-protocol.md. Define a JSON-over-Unix-socket protocol with exactly five message types: draw_rect, draw_circle, draw_arrow, draw_label, clear. Each message has fields: id (uuid v4 string), ttl_ms (int, default 3000, max 30000), color (hex string, default #ffae00), stroke_width (float pixels, default 2.0). Shape-specific fields: rect/circle take x,y,w,h; arrow takes x1,y1,x2,y2; label takes x,y,text (max 200 chars). clear takes optional id to clear one shape, or no field to clear all. Specify response format: {"ok":true,"id":"..."} or {"ok":false,"error":"..."}. Socket path: $XDG_RUNTIME_DIR/flashpaste-overlay.sock. One JSON object per line, newline-delimited. Include three example messages and three example responses. Do not add fields beyond what is listed. See docs/flashpaste-overlayd-plan.md Prompt 2.

File scope: docs/overlay-protocol.md

### Sub-task 2: Prompt 3 — Scaffold the flashpaste-overlayd Rust crate

Inside the existing flashpaste Rust workspace (rs/), create a new binary crate called flashpaste-overlayd. Add it to the workspace Cargo.toml. The crate's Cargo.toml should declare these dependencies (pin to latest minor compatible): smithay-client-toolkit=0.19, wayland-protocols=0.32, wayland-protocols-wlr=0.3, cairo-rs=0.20, pangocairo=0.20, serde=1 with derive feature, serde_json=1, tokio=1 with full feature, clap=4 with derive feature, anyhow=1, tracing=0.1, tracing-subscriber=0.3, uuid=1 with v4 feature. Add a minimal src/main.rs that prints 'flashpaste-overlayd 0.1.0' and exits. Run 'cargo check -p flashpaste-overlayd' and confirm it builds clean. Report any unresolved dependency versions and pick the next-latest compatible version. See docs/flashpaste-overlayd-plan.md Prompt 3.

File scope: rs/Cargo.toml, rs/flashpaste-overlayd/Cargo.toml, rs/flashpaste-overlayd/src/main.rs

### Sub-task 3: Prompt 4 — Define the wire-protocol message types in Rust (depends on: 1, 2)

Create rs/flashpaste-overlayd/src/protocol.rs. Implement #[derive(Serialize, Deserialize)] enums and structs that exactly match the JSON spec in docs/overlay-protocol.md (from Prompt 2). Use #[serde(tag="type", rename_all="snake_case")] for the message enum. Use uuid::Uuid for ids. Default values via #[serde(default)] and helper fns. Add a Color newtype that parses #rrggbb and #rrggbbaa hex strings into an (r,g,b,a) tuple of f64s in 0.0-1.0, with impl Default returning #ffae00. Add #[cfg(test)] unit tests in the same file that round-trip each message type through serde_json and verify defaults apply. Update src/main.rs to declare 'mod protocol;'. Run 'cargo test -p flashpaste-overlayd' and confirm all tests pass. See docs/flashpaste-overlayd-plan.md Prompt 4.

File scope: rs/flashpaste-overlayd/src/protocol.rs


## §S  delta
op|target|row
-|-|-

## §T  tasks
id|status|task|cites
-|-|-|-

## §B  bugs
id|status|task|cites
-|-|-|-
