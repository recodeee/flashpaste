<div align="center">

<img src="assets/logo.svg" alt="FlashPaste logo" width="120">

# FlashPaste

**The missing glue that makes image-paste *just work* in your terminal AI agent on GNOME Wayland.**

`PrtScr` → right-click → **Paste**. Done. Sub-120 ms on the bash hot path. Sub-15 ms on the daemon.

[Install](#install) · [Configure](#configure) · [How it works](#how-it-works) · [Performance](#performance) · [Troubleshooting](#troubleshooting) · [For agents & contributors](#for-agents--contributors)

</div>

---

## TL;DR

```bash
curl -fsSL https://raw.githubusercontent.com/NagyVikt/flashpaste/main/bootstrap.sh | bash
```

Then append the tmux + kitty snippets, reload, and press **PrtScr → right-click → Paste** inside any kitty + tmux pane running Claude Code, Codex, or any other terminal AI.

| Before flashpaste | After flashpaste |
|---|---|
| 5–15 paste presses, dock fills with "Unknown" gear icons, mutter wedges | One right-click, image attached in ~120 ms |
| `wl-paste -t image/png` returns 0 bytes from a background pane | Wayland-authoritative `has_image` policy with xclip fallback |
| Synthesized Ctrl+V gets eaten by kitty's keybinding | `kitty @ send-text \026` bypasses keybinding interception |

---

## Is this for you?

FlashPaste assumes the standard "Claude Code on Linux" stack. If you tick all three, you're in:

- **GNOME Wayland** (mutter compositor — Ubuntu 24.04, Fedora 40+, Debian 13, etc.)
- **kitty** as your terminal emulator, with `allow_remote_control yes`
- **tmux** running inside kitty

That's also the stack the upstream tools assume:

| Layer | Upstream | What flashpaste plugs into it |
|---|---|---|
| Compositor | [GNOME / mutter](https://gitlab.gnome.org/GNOME/mutter) | Works around mutter's surfaceless-client clipboard refusal |
| Terminal | [kitty](https://github.com/kovidgoyal/kitty) | Uses `kitty @ send-text` to bypass keybinding interception |
| Multiplexer | [tmux](https://github.com/tmux/tmux) | Plugs into `bind -n C-v` + right-click menu without recursing on itself |
| Clipboard | [wl-clipboard](https://github.com/bugaevc/wl-clipboard) | Shims `wl-paste` with xclip fallback + wedge cache |
| Input synth | [ydotool](https://github.com/ReimuNotMoe/ydotool) | Auto-patches the Ubuntu 24.04 socket-path bug in 0.1.8 |
| Screenshots | GNOME Screenshot (built-in) | A systemd `.path` unit pre-loads each new PNG into xclip the instant the file lands |

Don't have one of these? The installer's pre-flight tells you exactly what to `apt install`.

---

## Performance tiers

FlashPaste ships three implementations of the same hot path. Tier 1 is always on; Tiers 2 and 3 are progressive enhancements that fall back transparently to Tier 1.

| Tier | Path | Target latency | Status |
|---|---|---:|---|
| **1** | `bin/tmux-paste-dispatch.sh` (bash) | **~127 ms** | stable, default since v1.0 |
| **2** | `flashpaste-dispatch` (Rust one-shot — direct kitty IPC + in-process X11 selection) | **<40 ms** | opt-in, v1.15+ |
| **3** | `flashpasted` daemon + `flashpaste-trigger` (1-byte unix-socket trigger) | **<15 ms** | opt-in, v1.15+ |

The snippets in `examples/` already wire Tier 3 with automatic fallback to Tier 1, so you can install the daemon later without re-editing your dotfiles.

---

## Install

### Option A — Debian / Ubuntu `.deb` *(recommended)*

```bash
curl -fsSL -o /tmp/flashpaste.deb \
  https://github.com/NagyVikt/flashpaste/releases/latest/download/flashpaste_all.deb
sudo apt install /tmp/flashpaste.deb
```

Then activate per-user (one time):

```bash
systemctl --user daemon-reload
systemctl --user enable --now clipboard-janitor.service
systemctl --user enable --now flashpaste-screenshot-watcher.path
cat /usr/share/flashpaste/examples/tmux.conf.snippet  >> ~/.tmux.conf
cat /usr/share/flashpaste/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf
ln -sf /usr/share/flashpaste/paste_image.sh ~/paste_image.sh
flashpaste-doctor
```

Building the `.deb` yourself from a checkout:

```bash
git clone https://github.com/NagyVikt/flashpaste.git
cd flashpaste
make deb                                  # → dist/flashpaste_*_all.deb
sudo apt install ./dist/flashpaste_*_all.deb
```

### Option B — One-line dotfile install *(no apt, no root)*

```bash
curl -fsSL https://raw.githubusercontent.com/NagyVikt/flashpaste/main/bootstrap.sh | bash
```

The bootstrap clones to `$FLASHPASTE_DIR` (default `~/.local/share/flashpaste`), runs `flashpaste-doctor` for a 13-probe pre-flight, then `install.sh` to symlink scripts into `~/.local/bin/` and drop systemd user units. No root, no `apt`. Cautious variant:

```bash
git clone https://github.com/NagyVikt/flashpaste.git ~/.local/share/flashpaste
cd ~/.local/share/flashpaste
./bin/flashpaste-doctor.sh    # pre-flight only — touches nothing
./install.sh                  # symlinks + systemd units
```

### Option C — Build the Rust tiers from source

The bash hot path is always on; building the Rust workspace unlocks Tier 2 and Tier 3.

```bash
cd ~/.local/share/flashpaste/rs
cargo build --release
install -m 0755 target/release/flashpaste-{dispatch,trigger,shoot} \
                target/release/flashpasted \
                ~/.local/bin/

# Enable the daemon (Tier 3)
cp ../systemd/flashpasted.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now flashpasted.service
```

The tmux + kitty snippets in `examples/` already invoke `flashpaste-trigger` with a fallback to the bash dispatcher — once the daemon is up, Tier 3 takes over automatically.

> **Note:** `cargo build` hits crates.io. Run it interactively when you trust the workspace, not as part of an unattended install.

### Required system packages

```bash
sudo apt install wl-clipboard xclip xsel ydotool ydotoold tmux kitty
```

Plus the ydotoold socket-path drop-in (mandatory on Ubuntu 24.04, baked into the `.deb`):

```ini
# ~/.config/systemd/user/ydotoold.service.d/socket.conf
[Service]
ExecStartPost=ln -sf /tmp/.ydotool_socket %t/.ydotool_socket
ExecStopPost=rm -f %t/.ydotool_socket
```

---

## Configure

FlashPaste needs three small additions to your dotfiles. Full annotated versions live in [`examples/`](examples/) — paste them or symlink them, your choice.

### tmux — `~/.tmux.conf`

```tmux
set -g set-clipboard on
set -g mouse on
set -g @clip  '/home/$USER/.local/bin/clipboard-set.sh'
set -g @paste '/home/$USER/.local/bin/get-clipboard-text.sh'

# C-v → Tier 3 trigger (with automatic fallback to Tier 1)
bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v flashpaste-trigger '#{pane_id}' 2>/dev/null || TMUX_PASTE_TRIGGER=ctrl-v /home/$USER/.local/bin/tmux-paste-dispatch.sh '#{pane_id}'"

# Right-click → "Paste" menu item
bind -n MouseDown3Pane display-menu -O -x M -y M \
  "Paste" p "run-shell -b \"flashpaste-trigger '#{pane_id}' 2>/dev/null || /home/$USER/.local/bin/tmux-paste-dispatch.sh '#{pane_id}'\""
```

Reload with `tmux source-file ~/.tmux.conf`.

### kitty — `~/.config/kitty/kitty.conf`

```conf
map ctrl+v       launch --type=background --copy-env -- sh -c 'flashpaste-trigger "$(tmux display-message -p "#{pane_id}" 2>/dev/null)" 2>/dev/null || ~/paste_image.sh'
map ctrl+alt+v   launch --type=background --copy-env -- ~/paste_image.sh image
map ctrl+shift+print launch --type=background -- flashpaste-shoot
map ctrl+alt+print   launch --type=background -- flashpaste-shoot --interactive
```

Then restart kitty (it doesn't hot-reload keybindings).

> Append the verbatim snippets with `cat /usr/share/flashpaste/examples/tmux.conf.snippet >> ~/.tmux.conf` (or from `~/.local/share/flashpaste/examples/` for the dotfile install).

---

## Verify

```bash
flashpaste-doctor       # 13 parallel environment probes — all should be green
```

Smoke test:

1. Open kitty, attach to a tmux session, run your favourite terminal AI inside it.
2. Press **PrtScr** (GNOME drops a PNG into `~/Pictures/Screenshots/`).
3. Right-click anywhere in the pane → **Paste**.
4. The image attaches to your TUI session in ~120 ms (Tier 1) or ~15 ms (Tier 3).

If something's off, check `~/.local/state/clipboard-pipeline.log` and `~/.local/state/tmux-paste.log` — see [Troubleshooting](#troubleshooting).

---

## How it works

### The 120-millisecond fast path

```
PrtScr  ──►  file in ~/Pictures/Screenshots/
                                                                                       ┌── Claude Code
              ┌─ right-click → Paste in tmux pane                                      │   reads from xclip
              │                                                                        ▼   via the wl-paste shim
              ▼                                          T+ 50ms                  T+115ms
   tmux-paste-dispatch.sh  ──►  setsid xclip -i FILE  ──►  tmux unbind -n C-v  ──►  kitty @ send-text \026
              │                                                  │                       │
              │                                                  └── 100ms detached      └── setsid rebinds
              │                                                      sleep then rebind       tmux C-v
              │
              └── recursion-guard + early-preload + skip-probes when image is fresh
```

Total dispatch latency on Tier 1: **~127 ms**, down from ~3 seconds and 4–15 paste presses without flashpaste.

### What each tier replaces

- **Tier 1 (bash)** is the canonical implementation. Always installed, always working. Used as the fallback whenever Tiers 2/3 bail.
- **Tier 2 (`flashpaste-dispatch`)** replaces the bash dispatcher with a single Rust binary. The X11 selection is claimed in-process via `x11rb` with a pipe-handshake readiness signal — no `setsid xclip ... &` plus blind 50 ms sleep. Kitty's RC protocol is spoken directly over the unix socket, eliminating the ~25 ms Python startup cost of forking `kitty @ send-text`.
- **Tier 3 (`flashpasted` + `flashpaste-trigger`)** moves the slow work — file reads, Wayland/X11 selection claims, kitty socket lookup — *before* you press Ctrl-V. The tmux binding fires a 5-line trigger binary that writes one JSON message to a unix socket; the daemon already has everything staged and just runs unbind → kitty send-text → schedule-rebind. Bonus side effect: one persistent Wayland client with a stable `app_id` instead of N short-lived `wl-paste` forks, which means **no more phantom "wl-clipboard" entries in the Ubuntu Dock**.

### Why it exists

On a Linux box running GNOME 46 / mutter / Wayland with tmux inside kitty, the "normal" clipboard pipeline for image paste is brittle in compounding ways:

| Layer | What goes wrong |
|---|---|
| **mutter** | Refuses to expose clipboard contents to surfaceless Wayland clients. `wl-paste -t image/png` returns 0 bytes from a background tmux pane even when the data is right there. |
| **wl-copy --paste-once** | GNOME screenshot tools publish via `wl-copy --paste-once`, which serves exactly *one* receive then exits. Any script that probes the clipboard (e.g. `wl-paste --list-types`) drains it before the real consumer reads. |
| **tmux** | A `bind -n C-v` binding that re-dispatches paste handlers *consumes* the keystroke instead of forwarding it — synthesized Ctrl-V never reaches the inner TUI. |
| **kitty** | A `map ctrl+v` keybinding intercepts physical Ctrl+V before it reaches the inner TUI; `kitty @ send-text` bypasses it but the byte then hits tmux's `bind -n C-v` and recurses. |
| **ydotool 0.1.8** | Ubuntu 24.04 ships an old release with the wrong syntax (`ctrl+v`, not `29:1 47:1`) and a socket-path bug (ignores `--socket-path`, always uses `/tmp/.ydotool_socket`). |
| **Ubuntu Dock** | Surfaces every short-lived Wayland client as a transient "Unknown" gear icon. Every `wl-paste` call flashes the dock. |

Each layer alone is mostly harmless. Stacked together they create the "needs 5–15 paste presses, then dock fills with phantom gear icons, then mutter wedges" experience. FlashPaste papers over all of it.

### Screenshot capture (Phase 3)

`flashpaste-shoot` is a small Rust binary that takes a screenshot via the XDG Desktop Portal and stages it directly into the daemon (or `~/Pictures/Screenshots/` if the daemon isn't running). End-to-end **Print → ready: ~250 ms**, vs the GNOME Screenshot UI flow that needs 3–4 clicks and 3+ seconds.

```bash
flashpaste-shoot                 # full screen, stage to daemon
flashpaste-shoot --interactive   # area picker
flashpaste-shoot --print-path    # write PNG path to stdout for shell composition
flashpaste-shoot --no-daemon     # skip daemon stage, just drop on disk
```

### Repo layout

```
bin/                  Bash hot path — canonical, always works
rs/                   Rust workspace: flashpaste-{common,dispatch,trigger,shoot,mcp} + flashpasted
share/applications/   NoDisplay .desktop files for surfaceless Wayland clients
systemd/              User units: clipboard-janitor, screenshot-watcher, flashpasted
examples/             Config snippets for tmux + kitty
packaging/            .deb tooling
```

---

## Performance

### Environment variables

| Env var | Effect | Default |
|---|---|---|
| `FLASHPASTE_QUIET` | Suppress every `log`/`clog`/`t` call. Saves ~5–15 ms per dispatch. Recommended once the system is stable. | `0` |
| `FLASHPASTE_TRACE` | Emit one JSON line per checkpoint to `$FLASHPASTE_TRACE_LOG`. | `0` |
| `FLASHPASTE_TRACE_LOG` | Destination JSONL trace sink. | `~/.local/state/flashpaste-trace.jsonl` |
| `FLASHPASTE_DIR` | Bootstrap clone destination. | `~/.local/share/flashpaste` |
| `TMUX_PASTE_LOG` | Per-invocation text log path. | `~/.local/state/tmux-paste.log` |
| `CLIP_PIPELINE_LOG` | Shared structured event-log path. | `~/.local/state/clipboard-pipeline.log` |
| `CLIPBOARD_JANITOR_INTERVAL` | Janitor sweep interval (seconds). | `1` |
| `CLIPBOARD_JANITOR_REAP_PASTE_AFTER` | Reap `wl-paste` after this many seconds. | `8` |
| `CLIPBOARD_JANITOR_REAP_COPY_AFTER` | Reap `wl-copy` after this many seconds. | `3` |
| `CLIPBOARD_GET_TIMEOUT` | `wl-paste` text-read timeout. | `1.0` |
| `CLIPBOARD_GET_X11_TIMEOUT` | xclip / xsel text-read timeout. | `0.5` |
| `WL_PASTE_SHIM_WEDGE_TTL` | Seconds to cache "mutter is wedged" state (suppresses dock flashes). | `30` |
| `RUST_LOG` | Tracing level for Rust binaries. | `info,flashpasted=debug` |

`FLASHPASTE_QUIET=1` wins over `FLASHPASTE_TRACE` — it suppresses the JSON sink too.

### Tracing + percentile analysis

```bash
flashpaste-trace.sh                     # p50/p90/p99 per checkpoint, last 100 pastes
flashpaste-trace.sh --tail              # live
flashpaste-trace.sh --last 500          # widen the window
flashpaste-trace.sh --since 2026-05-19T12:00:00Z
flashpaste-trace.sh --raw               # cat the JSONL log
```

A typical Tier 1 dispatch:

```
[12:01:33.487] T+   0ms (Δ  0ms) :: script-start argv='%2'
[12:01:33.491] T+   4ms (Δ  4ms) :: recursion-guard-passed
[12:01:33.498] T+  11ms (Δ  7ms) :: select-pane
[12:01:33.524] T+  37ms (Δ 26ms) :: early-preload before-xclip
[12:01:33.578] T+  91ms (Δ 54ms) :: early-preload after-sleep
[12:01:33.586] T+  99ms (Δ  8ms) :: fast-path before-unbind
[12:01:33.591] T+ 104ms (Δ  5ms) :: fast-path after-unbind
[12:01:33.612] T+ 125ms (Δ 21ms) :: fast-path after-send-text
[12:01:33.614] T+ 127ms (Δ  2ms) :: fast-path exit
```

If something regresses, the `Δ` column tells you immediately which step.

---

## Troubleshooting

### Log locations (tail-friendly)

```bash
tail -F ~/.local/state/clipboard-pipeline.log    # cross-script event timeline
tail -F ~/.local/state/tmux-paste.log            # per-invocation timing (T+/Δ)
journalctl --user -u flashpasted -f              # Tier 3 daemon (if installed)
```

### Daemon health (Tier 3)

```bash
systemctl --user status flashpasted              # Active (running)
ss -lUn | grep flashpaste.sock                   # Socket present
```

If the socket isn't there, the `flashpaste-trigger` binary `exec`s the bash dispatcher — Tier 1 keeps working unchanged.

### Common issues

| Symptom | Likely cause | Fix |
|---|---|---|
| Paste returns text when you wanted an image | Clipboard text overshadowed the image MIME | Re-screenshot, or use Ctrl+Alt+V to force the image path |
| `flashpaste-doctor` flags ydotool socket | Ubuntu 24.04 socket-path bug | Re-run `install.sh` (drops in the systemd override) |
| Dock fills with "Unknown" icons during heavy paste | `share/applications/wl-clipboard.desktop` not installed | `make install` again, or upgrade to Tier 3 (daemon eliminates the issue at the root) |
| Tier 3 daemon won't start | Stale socket at `$XDG_RUNTIME_DIR/flashpaste.sock` | `rm "$XDG_RUNTIME_DIR/flashpaste.sock"` then `systemctl --user restart flashpasted` |
| Right-click menu does nothing | tmux `mouse off`, or snippet not loaded | `tmux source-file ~/.tmux.conf`; confirm `set -g mouse on` is present |

---

## MCP — agents get eyes and hands on your desktop

flashpaste ships **`flashpaste-mcp`**, a Model Context Protocol server that any MCP-aware agent (Claude Code, Cursor, …) can register over stdio. Four tools:

| Tool | What it does |
|---|---|
| `take_screenshot(interactive?)` | Captures the screen via XDG portal, returns **real PNG bytes the model can see** so it can debug visual UI / "look at my screen". |
| `read_clipboard()` | Returns the current clipboard as text (uses flashpaste's wl-paste shim → xclip fallback). |
| `copy_text(text)` | Puts text on your clipboard so you can paste it elsewhere. |
| `paste_to_pane(pane_id)` | Triggers flashpaste to paste your clipboard into a specific tmux pane — including a **different agent's pane** for cross-agent hand-off in <15 ms. |

### Register with Claude Code

```bash
mkdir -p ~/.config/claude-code
cat <<'JSON' > ~/.config/claude-code/mcp.json
{ "mcpServers": { "flashpaste": { "command": "flashpaste-mcp", "args": [] } } }
JSON
# or:  claude mcp add flashpaste flashpaste-mcp
```

The server is hand-rolled JSON-RPC 2.0 over stdio (~300 LOC, no SDK churn). Logs to stderr; protocol on stdin/stdout.

### Agent skill (paired prompt + usage notes)

A copy-paste Claude Code [skill](https://docs.anthropic.com/en/docs/claude-code/skills) lives at [`skills/flashpaste/SKILL.md`](skills/flashpaste/SKILL.md). Install with:

```bash
mkdir -p ~/.claude/skills
cp -r skills/flashpaste ~/.claude/skills/
```

Then Claude Code will load it whenever the user mentions screenshots, clipboard, paste, or cross-pane hand-off.

---

## For agents & contributors

> **Authoritative agent guidance lives in [AGENTS.md](AGENTS.md).** Read it before opening a PR — it covers the non-negotiable release workflow (every `vX.Y` commit must be tagged + released same turn), the file layout, the parallel-agent rules of engagement, and the four hard-won facts the code must preserve.

### Four hard-won facts the code must preserve

1. **`kitty @ send-text` is the only transport** that triggers Claude Code's image-paste handler. `tmux send-keys -t pane C-v` writes the byte but the handler doesn't fire.
2. **`tmux bind -n C-v` recurses** when `\026` arrives via kitty send-text. Unbind before send-text, rebind ~100 ms later via `setsid -f` (detached so it survives parent exit).
3. **Wayland-authoritative `has_image` policy.** Trust Wayland if it answers; only fall back to X11 when Wayland is fully silent. mutter's X11↔Wayland bridge is sticky and produces stale `image/png` advertisements after fresh text copies.
4. **GNOME PrtScr saves but doesn't copy.** Auto-pickup loads `~/Pictures/Screenshots/<latest>.png` into the clipboard if ≤ 30 s old and clipboard text is empty.

### The contract surface (what's safe to depend on)

| Boundary | Path / name | Stable? |
|---|---|---|
| Tier 1 dispatcher | `~/.local/bin/tmux-paste-dispatch.sh <pane-id>` | yes |
| Tier 2 dispatcher | `~/.local/bin/flashpaste-dispatch <pane-id>` | yes |
| Tier 3 trigger | `~/.local/bin/flashpaste-trigger <pane-id>` | yes |
| Daemon socket | `$XDG_RUNTIME_DIR/flashpaste.sock` | yes |
| Wire protocol | 4-byte LE u32 length + JSON `{op,pane,ts}` → `{ok,latency_ms}` / `{ok,reason,fallback}` | experimental |
| Screenshot capture | `flashpaste-shoot [--interactive\|--no-daemon\|--print-path\|--output PATH]` | yes |
| MCP server | `flashpaste-mcp` (stdio) — exposes clipboard/screenshot tools to LLM agents | experimental |
| Pre-flight | `flashpaste-doctor` | yes |
| Trace aggregator | `flashpaste-trace.sh [--last N\|--since ISO\|--tail\|--raw]` | yes |

### Dev commands

```bash
# Bash syntax check (run before every commit)
bash -n bin/*.sh install.sh bootstrap.sh

# Rust workspace (gated on user approval — hits crates.io)
cargo build   --release --manifest-path rs/Cargo.toml
cargo fmt              --manifest-path rs/Cargo.toml --check
cargo clippy --release --manifest-path rs/Cargo.toml -- -D warnings

# .deb build (auto-includes Rust binaries if rs/target/release exists)
make deb

# Pre-flight
make doctor

# Tag-check (AGENTS.md release rule)
bash AGENTS-release-check.sh
```

### Parallel-agent workflow

When dispatching multiple agents at the same repo:

- Each agent owns **disjoint file paths** — no cross-edits.
- Pre-create shared scaffolding (e.g. `rs/Cargo.toml` workspace root) before dispatching so agents don't race on it.
- Agents needing types from sibling crates should **duplicate small helpers inline** rather than depend across in-flight crates. Refactor to a shared crate after all agents land.
- After agents return, run the sanity sweep: `bash -n` on every script, `cargo metadata --offline --no-deps` on the workspace.

Full rules and the version-bump policy: [AGENTS.md](AGENTS.md).

---

## Approaches that look promising but don't work on GNOME Wayland

A non-exhaustive list of dead-ends so future contributors don't waste a week:

- **`wl-clip-persist`** — wlroots-only. Mutter doesn't implement `wlr-data-control`, so it fails with `Failed to get clipboard manager`.
- **xclip → wl-copy bridge to prime the Wayland clipboard** — mutter rejects surfaceless `wl-copy` clients from claiming the selection. The daemon stays alive forever but `wl-paste` returns 0 bytes.
- **`@<path>` typed into Claude Code's prompt** — the TUI doesn't auto-attach typed file paths; the @-mention requires the interactive Tab-completion file picker.
- **Pure `ydotool` Ctrl+V from a kitty pane** — kitty's own `map ctrl+v` keybinding intercepts synthesized keystrokes and runs its paste action instead of letting the keystroke reach the inner TUI. (`kitty @ send-text` bypasses this, at the cost of needing the unbind-rebind dance with tmux.)
- **`tmux send-keys -t $pane C-v`** — the byte arrives in the pty but Claude Code's image-paste handler does NOT trigger from it. Use `kitty @ send-text \026` (verified working) instead.
- **Clipboard-poll service polling `wl-paste --type text`** — every poll cycle flashes the Ubuntu dock and re-writes the clipboard with cliphist's last cached text, breaking actual paste. **Keep it disabled.**

---

## License & credits

MIT — see [LICENSE](LICENSE).

Wrenched into shape across a single multi-hour session of Wayland clipboard pain on a real GNOME 46 / kitty / tmux / Claude Code setup. The session log, including every dead-end and every fix, lives in the commit history.
