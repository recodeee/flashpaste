---
title: FlashPaste architecture — three performance tiers, kitty IPC, and the daemon protocol
description: In-depth architecture reference for FlashPaste. Covers Tier 1 (bash hot path), Tier 2 (Rust one-shot dispatcher), Tier 3 (persistent daemon + 1-byte trigger), the kitty RC protocol, the daemon's unix-socket wire format, the recursion-guard mechanism, and the Wayland-authoritative has_image policy.
keywords:
  - flashpaste architecture
  - kitty IPC protocol
  - tmux paste dispatcher
  - flashpaste daemon
  - wayland clipboard owner
  - has_image policy
  - recursion guard
  - sub 15 millisecond paste
last_updated: 2026-05-19
canonical: https://github.com/NagyVikt/flashpaste/blob/main/docs/architecture.md
---

# Architecture

FlashPaste implements the same conceptual hot path three times, in three different runtimes, at three different latencies. **Tier 1 (bash)** is always installed and always works. **Tier 2 (Rust one-shot)** replaces only the dispatcher binary. **Tier 3 (Rust daemon + trigger)** moves the slow work *before* you press paste. All three are wire-compatible at the tmux binding level.

## The shared hot path

Every tier executes the same four logical steps:

1. **Resolve the source.** Is there a fresh PNG in `~/Pictures/Screenshots/` (<30 s old)? Is the clipboard already advertising `image/png`? Is `~/.local/state/clip-pipeline.lastimg` recent enough to skip probes?
2. **Stage the bytes.** Load the PNG into the X11 selection via xclip (Tier 1, bash) or `x11rb` (Tier 2/3, Rust). Wayland is unreliable on mutter for surfaceless clients, so the X11 path is authoritative and the Wayland path is best-effort.
3. **Disable the recursion guard.** `tmux unbind -n C-v` so the synthesized `\026` byte doesn't re-trigger this very dispatcher.
4. **Inject the byte.** `kitty @ send-text \026` writes raw Ctrl-V into the inner pty, where the terminal AI's image-paste handler picks it up.

After step 4, a detached process re-binds `C-v` ~100 ms later so the next user keystroke is routed normally.

## Tier 1 — `tmux-paste-dispatch.sh`

The canonical bash implementation. Lives at `bin/tmux-paste-dispatch.sh`. Always installed. Symlinked to `~/.local/bin/tmux-paste-dispatch.sh`.

Wall-clock latency on a reference Ubuntu 24.04 / kitty / tmux box: **~127 ms**. The major checkpoints (visible in `~/.local/state/tmux-paste.log`):

| T+ | Δ | Step |
|---:|---:|---|
| 0 ms | — | `script-start argv='%2'` |
| 4 ms | 4 ms | `recursion-guard-passed` |
| 11 ms | 7 ms | `select-pane` |
| 37 ms | 26 ms | `early-preload before-xclip` |
| 91 ms | 54 ms | `early-preload after-sleep` |
| 99 ms | 8 ms | `fast-path before-unbind` |
| 104 ms | 5 ms | `fast-path after-unbind` |
| 125 ms | 21 ms | `fast-path after-send-text` |
| 127 ms | 2 ms | `fast-path exit` |

The 50 ms blind sleep after `xclip` is the biggest chunk and the reason Tier 2 exists.

## Tier 2 — `flashpaste-dispatch` (Rust one-shot)

A drop-in replacement for `tmux-paste-dispatch.sh`. Lives at `rs/flashpaste-dispatch/`. Symlinked to `~/.local/bin/flashpaste-dispatch`.

Wall-clock latency: **<40 ms**. Two specific wins:

1. **In-process X11 selection.** `x11rb` claims the selection inside a re-exec'd subcommand (`flashpaste-dispatch __hold-selection --mime image/png --path FILE --ready-fd N`) that signals readiness via a pipe-handshake — no `setsid xclip -i FILE &` plus blind 50 ms sleep.
2. **Direct kitty RC protocol.** The kitty IPC envelope (`\x1bP@kitty-cmd…\x1b\\`) is spoken directly over the unix socket. This eliminates the ~25 ms Python startup cost of forking `kitty @ send-text`.

Fallback: if X11 staging fails or no fresh screenshot is found, `flashpaste-dispatch` execs `tmux-paste-dispatch.sh` and the user sees Tier 1 latency.

## Tier 3 — `flashpasted` + `flashpaste-trigger`

The daemon owns the clipboard. The trigger is a 5 ms hot-path client.

Wall-clock latency: **<15 ms** end-to-end. The daemon pre-stages everything (Wayland selection claim, X11 selection ownership, kitty socket lookup, inotify-driven screenshot tracking) before the user ever presses paste.

### The daemon (`flashpasted`)

Built from `rs/flashpasted/`. Installed at `~/.local/bin/flashpasted`. Managed by `~/.config/systemd/user/flashpasted.service`.

Subsystems:

- **Wayland selection owner.** Persistent connection to mutter. Owns `image/png` on the data device whenever a fresh screenshot is staged.
- **X11 selection owner.** Long-lived connection to the X server (via XWayland). Owns `CLIPBOARD` and `PRIMARY` for `image/png`.
- **Inotify watcher.** Watches `~/Pictures/Screenshots/` for `IN_CLOSE_WRITE`. New PNGs are loaded into both selection owners immediately.
- **Unix socket listener.** Accepts trigger pings on `$XDG_RUNTIME_DIR/flashpaste.sock`.

Stable `app_id`: one persistent Wayland client instead of N short-lived `wl-paste`/`wl-copy` forks. This is why Tier 3 eliminates phantom "Unknown" gear icons in the Ubuntu Dock.

CLI flags:

```text
flashpasted [--socket PATH] [--screenshots-dir PATH]
            [--no-inotify] [--no-wayland] [--no-x11]
```

### The trigger (`flashpaste-trigger`)

Built from `rs/flashpaste-trigger/`. Stripped binary is under 500 KB. Zero runtime dependencies beyond `libc` and the daemon socket.

What the trigger does, end-to-end:

1. Resolve the socket path (`$XDG_RUNTIME_DIR/flashpaste.sock`, or `/run/user/<uid>` / `/tmp` fallbacks).
2. Open a unix-stream connection with a 5 ms `connect` timeout.
3. Write a 4-byte little-endian length prefix followed by a JSON message: `{"op":"paste","pane":"%2","ts":1716120000}`.
4. Read the daemon's response (10 ms write timeout, 150 ms read timeout): `{"ok":true,"latency_ms":11}` or `{"ok":false,"reason":"...","fallback":true}`.
5. If `fallback=true` or the daemon is unavailable, `exec` `tmux-paste-dispatch.sh` so Tier 1 takes over.

### Wire protocol

```text
┌──────────┬──────────────────────────────────┐
│ u32 LE   │ JSON body                        │
│ length   │ {"op":"paste","pane":"%2",       │
│          │  "ts":1716120000}                │
└──────────┴──────────────────────────────────┘
```

Operations:

- `paste` — execute a full paste dispatch (the common case)
- `stage` — load a PNG file into the selection owners without injecting Ctrl-V
- `health` — return daemon health and uptime

Responses are always `{"ok": bool, ...}` so a trigger that fails to parse can fall back.

## The recursion-guard mechanism

`tmux bind -n C-v` consumes Ctrl-V as a *trigger* — so if the dispatcher then sends `\026` (raw Ctrl-V) via kitty, tmux's binding fires *again* and infinitely recurses. FlashPaste breaks the cycle in three places:

1. **Lock file.** Each dispatcher invocation drops `$XDG_RUNTIME_DIR/tmux-paste-dispatch.lock`. A secondary invocation within 2 s no-ops.
2. **Unbind-before-send.** `tmux unbind -n C-v` runs before `kitty @ send-text \026`.
3. **Detached rebind.** `setsid -f sh -c 'sleep 0.1; tmux bind -n C-v ...'` rebinds asynchronously so the dispatcher can exit immediately and the next user keystroke is routed normally.

## The Wayland-authoritative `has_image` policy

mutter's X11↔Wayland clipboard bridge is *sticky*. After a text copy, X11 will keep advertising `image/png` from the previous screenshot for an indeterminate time. Trusting X11's MIME advertisement causes the canonical "I copied a GitHub URL, why am I pasting yesterday's screenshot?" bug (observation #6881 in the project log).

The policy:

1. **Ask Wayland first.** If `wl-paste --list-types` answers, use its answer authoritatively.
2. **Fall back to X11 only if Wayland is silent.** mutter going silent is the wedge condition; the wedge cache (TTL `WL_PASTE_SHIM_WEDGE_TTL`, default 30 s) suppresses repeated probes.

The shim at `bin/wl-paste` implements this policy and is what kitty / tmux / the dispatcher all call.

## Screenshot capture (`flashpaste-shoot`)

Built from `rs/flashpaste-shoot/`. Captures a screenshot via the XDG Desktop Portal (`org.freedesktop.portal.Screenshot`) and stages it directly into the daemon if running, or saves to `~/Pictures/Screenshots/` otherwise.

End-to-end Print → ready: ~250 ms, vs ~3 s for the GNOME Screenshot UI flow.

CLI:

```text
flashpaste-shoot [--interactive] [--no-daemon] [--output PATH]
                 [--print-path] [--timeout-ms N] [-v|--verbose]
```

## MCP server (`flashpaste-mcp`)

Built from `rs/flashpaste-mcp/`. Exposes clipboard + screenshot tools to LLM agents over the stdio MCP transport. An agent can take a screenshot, read the clipboard, or paste into a specific pane without leaving its tool-call loop.

Status: experimental. See [use-cases.md](use-cases.md#agent-driven-screenshots-via-mcp) for an example flow.

## Repo layout

```text
bin/                  Bash hot path (Tier 1) — canonical, always works
rs/                   Rust workspace
  flashpaste-common/  Shared library (paths, clipboard, kitty IPC, tmux, logging)
  flashpaste-dispatch/ Tier 2 binary
  flashpasted/        Tier 3 daemon
  flashpaste-trigger/ Tier 3 trigger
  flashpaste-shoot/   Portal-based screenshot capture
  flashpaste-mcp/     MCP server
systemd/              User unit files
share/applications/   NoDisplay .desktop files for surfaceless Wayland clients
examples/             tmux + kitty config snippets
packaging/            Debian .deb tooling
docs/                 This documentation tree
```

## Further reading

- [troubleshooting.md](troubleshooting.md) — diagnostic flowchart and log file map
- [AGENTS.md](../AGENTS.md) — the four hard-won facts every code change must preserve
- [ROADMAP.md](../ROADMAP.md) — what's planned next
