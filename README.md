# flashpaste

> Sub-120ms (bash) → sub-40ms (Rust one-shot) → sub-15ms (daemon) image-paste into terminal AI agents (Claude Code, Codex, etc.) on GNOME Wayland — even when mutter's clipboard is wedged.

Don't fight the stack. **Install once, paste forever.** `PrtScr` → right-click → **Paste** → the screenshot is attached to your TUI session before you blink. No more retry-spamming Ctrl+V hoping the clipboard daemon will cooperate.

| Tier | Path | Target latency | Status |
|---|---|---:|---|
| 1 | `bin/tmux-paste-dispatch.sh` (bash) | **~127 ms** | stable, default since v1.0 |
| 2 | `flashpaste-dispatch` (Rust one-shot, direct kitty IPC + in-process X11 selection) | **<40 ms** | opt-in, v1.15 |
| 3 | `flashpasted` daemon + `flashpaste-trigger` (1-byte unix-socket trigger) | **<15 ms** | opt-in, v1.15 |

Tier 1 is always-on. Tiers 2 and 3 are progressive enhancements — both fall back to Tier 1 cleanly when not wired in.

flashpaste is the missing glue that makes the standard Linux terminal stack just work for image paste:

| Layer | Upstream | What flashpaste plugs into it |
|---|---|---|
| Compositor | [GNOME / mutter](https://gitlab.gnome.org/GNOME/mutter) | Works around mutter's surfaceless-client clipboard refusal |
| Terminal | [kitty](https://github.com/kovidgoyal/kitty) | Uses `kitty @ send-text` to bypass keybinding interception |
| Multiplexer | [tmux](https://github.com/tmux/tmux) | Plugs into `bind -n C-v` + right-click menu without recursing on itself |
| Clipboard | [wl-clipboard](https://github.com/bugaevc/wl-clipboard) | Shims `wl-paste` with xclip fallback + wedge cache |
| Input synth | [ydotool](https://github.com/ReimuNotMoe/ydotool) | Auto-patches the Ubuntu 24.04 socket-path bug in 0.1.8 |
| Screenshots | GNOME Screenshot (built-in) | A systemd `.path` unit pre-loads each new PNG into xclip the instant the file lands |

If you already run **kitty + tmux on GNOME Wayland** (the standard Claude Code / Codex setup), you have everything flashpaste needs. If not, the installer will tell you exactly what to `apt install`.

## What it does

- **Image paste works on the *first* press.** No matter how wedged mutter's clipboard is.
- **PrtScr → right-click → Paste.** No extra clipboard helper, no manual file dance.
- **Multi-paste the same screenshot.** Hammer it as many times as you want, in any pane.
- **Falls back gracefully.** xclip (XWayland), file-based pre-stage, recursion guard, wedge cache.
- **No phantom dock icons.** Aggressive janitor + NoDisplay `.desktop` for every short-lived Wayland client.
- **Three tiers, one knob.** Bash by default; flip a tmux binding to opt into the Rust dispatch or the daemon path.
- **End-to-end timing telemetry.** Every checkpoint is logged with `T+<ms> Δ<ms>`; `FLASHPASTE_TRACE=1` emits one JSONL row per checkpoint for percentile analysis (`flashpaste-trace.sh`).

## Why this exists

On a Linux box running GNOME 46 / mutter / Wayland with tmux inside kitty (or GNOME Terminal), the "normal" clipboard pipeline for image paste is brittle for several compounding reasons:

| Layer | What goes wrong |
|---|---|
| **mutter** | Refuses to expose clipboard contents to surfaceless Wayland clients. `wl-paste -t image/png` returns 0 bytes from a background tmux pane even when the data is *right there*. |
| **wl-copy --paste-once** | GNOME screenshot tools publish via `wl-copy --paste-once`, which serves exactly *one* receive then exits. Any script that probes the clipboard (e.g. `wl-paste --list-types`) drains it before the real consumer (Claude Code) reads. |
| **tmux** | A `bind -n C-v` binding that re-dispatches paste handlers *consumes* the keystroke instead of forwarding it — synthesized Ctrl-V never reaches the inner TUI. |
| **kitty** | A `map ctrl+v` keybinding intercepts physical Ctrl+V before it reaches the inner TUI; `kitty @ send-text` lets you bypass it but the byte then hits tmux's `bind -n C-v` and recurses. |
| **ydotool 0.1.8** | Ubuntu 24.04 ships an old release with the wrong syntax (`ctrl+v`, NOT `29:1 47:1`) and a socket-path bug (`ignores --socket-path`, always uses `/tmp/.ydotool_socket`). |
| **Ubuntu Dock** | Surfaces every short-lived Wayland client as a transient "Unknown" gear icon. Every `wl-paste` call flashes the dock. |

Each layer alone is mostly harmless. Stacked together they create the "needs 5–15 paste presses, then dock fills with phantom gear icons, then mutter wedges" experience that anyone trying to use Claude Code on GNOME Wayland is familiar with.

flashpaste papers over all of it.

## The 120-millisecond fast path

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

Total dispatch latency: **~120ms**, down from ~3 seconds and 4 paste-presses without flashpaste.

## Quick start

### Option A — Debian / Ubuntu .deb (recommended)

Each tagged release attaches a `.deb` to GitHub Releases. Install via apt:

```bash
# Once a release exists:
curl -fsSL -o /tmp/flashpaste.deb \
  https://github.com/NagyVikt/flashpaste/releases/latest/download/flashpaste_all.deb
sudo apt install /tmp/flashpaste.deb

# Then per-user activation:
systemctl --user daemon-reload
systemctl --user enable --now clipboard-janitor.service
systemctl --user enable --now flashpaste-screenshot-watcher.path
cat /usr/share/flashpaste/examples/tmux.conf.snippet  >> ~/.tmux.conf
cat /usr/share/flashpaste/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf
ln -sf /usr/share/flashpaste/paste_image.sh ~/paste_image.sh
flashpaste-doctor
```

Or build the .deb yourself from a checkout:

```bash
git clone https://github.com/NagyVikt/flashpaste.git
cd flashpaste
make deb                                    # → dist/flashpaste_*_all.deb
sudo apt install ./dist/flashpaste_*_all.deb
```

### Option B — One-line dotfile install (no apt)

```bash
curl -fsSL https://raw.githubusercontent.com/NagyVikt/flashpaste/main/bootstrap.sh | bash
```

The bootstrap runs the [**doctor**](bin/flashpaste-doctor.sh) first — 13 parallel environment checks (Wayland session, mutter, kitty installed, kitty IPC socket, tmux installed and running, tmux inside kitty, wl-clipboard, xclip, ydotool + socket, screenshots dir, …) so you see green/red status before anything is touched. Run it standalone anytime:

```bash
bash ~/.local/share/flashpaste/bin/flashpaste-doctor.sh
```

**Or the careful version:**

```bash
git clone https://github.com/NagyVikt/flashpaste.git ~/.local/share/flashpaste
cd ~/.local/share/flashpaste
./bin/flashpaste-doctor.sh   # pre-flight check
./install.sh
```

The installer:
1. Symlinks every script into `~/.local/bin/`.
2. Drops two systemd user services:
   - `clipboard-janitor.service` — reaps stuck `wl-copy`/`wl-paste` daemons.
   - `flashpaste-screenshot-watcher.path` + `.service` — fires `flashpaste-screenshot-preload.sh` the instant a new PNG lands in `~/Pictures/Screenshots/`, so xclip is "hot" before you reach for right-click.
3. Patches `ydotoold.service` with the Ubuntu-24.04 socket-path drop-in.
4. Prints the tmux + kitty config snippets you need to copy into your dotfiles.

After install, append the snippets:

```bash
cat ~/.local/share/flashpaste/examples/tmux.conf.snippet  >> ~/.tmux.conf
cat ~/.local/share/flashpaste/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf
tmux source-file ~/.tmux.conf
# restart kitty
```

### Performance knobs

| Env var | Effect |
|---|---|
| `FLASHPASTE_QUIET=1` | Suppresses every `log`/`clog`/`t` call. Saves ~5–15ms per dispatch. Recommended once the system is stable and you don't need timing telemetry. |
| `FLASHPASTE_DIR` | Override the install location (default `~/.local/share/flashpaste`). |
| `TMUX_PASTE_LOG` | Override the per-invocation log path. |
| `CLIP_PIPELINE_LOG` | Override the structured event-log path. |

### Required dependencies

```bash
sudo apt install wl-clipboard xclip xsel ydotool ydotoold tmux kitty
```

Plus the **ydotoold socket-path workaround** (mandatory on Ubuntu 24.04):

```ini
# ~/.config/systemd/user/ydotoold.service
[Service]
ExecStartPost=ln -sf /tmp/.ydotool_socket %t/.ydotool_socket
ExecStopPost=rm -f %t/.ydotool_socket
```

### Required tmux config

Drop these into `~/.tmux.conf` (full example in [`examples/tmux.conf.snippet`](examples/tmux.conf.snippet)):

```tmux
set -g set-clipboard on
set -g @clip  '/home/$USER/.local/bin/clipboard-set.sh'
set -g @paste '/home/$USER/.local/bin/get-clipboard-text.sh'

# C-v in any pane → dispatch (image paste + text paste + auto-pickup)
bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v /home/$USER/.local/bin/tmux-paste-dispatch.sh '#{pane_id}'"

# Right-click menu Paste item
bind -n MouseDown3Pane display-menu -O -x M -y M \
  "Paste" p "run-shell -b \"/home/$USER/.local/bin/tmux-paste-dispatch.sh '#{pane_id}'\""
```

### Required kitty config

Drop into `~/.config/kitty/kitty.conf`:

```conf
map ctrl+v       launch --type=background --copy-env -- ~/paste_image.sh
map ctrl+alt+v   launch --type=background --copy-env -- ~/paste_image.sh image
```

## Files

| File | Role |
|---|---|
| `bin/tmux-paste-dispatch.sh` | The dispatcher. Right-click Paste / Ctrl-V → routes to image / text branch. |
| `bin/wl-paste` | Drop-in shim for `wl-paste` — falls back to xclip when mutter is silent. Wedge cache. |
| `bin/clipboard-set.sh` | tmux `@clip` target. Pipes selection bytes to `wl-copy` with env-resilience. |
| `bin/get-clipboard-text.sh` | tmux `@paste` target. Multi-source text reader (wl-paste → xclip → xsel → cliphist). |
| `bin/clipboard-janitor.sh` | systemd user service. Reaps wedged `wl-paste` / `wl-copy` daemons. |
| `bin/clip-pipeline-log.sh` | Shared `clog` logger used by every script for unified timeline view. |
| `bin/screenshot-to-clipboard` | One-shot screenshot helper using gnome-shell dbus. |
| `bin/paste_image.sh` | Kitty Ctrl+V / Ctrl+Alt+V keybinding helper. |

## Logging

Two streams of structured logs, both live and tail-friendly:

```bash
tail -F ~/.local/state/clipboard-pipeline.log    # cross-script event timeline
tail -F ~/.local/state/tmux-paste.log            # per-invocation timing (T+/Δ)
```

Every dispatch invocation emits checkpoints like:

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

### Tracing

Set `FLASHPASTE_TRACE=1` in the env (e.g. via `~/.tmux.conf`'s `set-environment`) to also emit one JSON line per checkpoint to `~/.local/state/flashpaste-trace.jsonl`. Aggregate with:

```bash
flashpaste-trace.sh                    # p50/p90/p99 per step, last 100 pastes
flashpaste-trace.sh --tail             # live
```

`FLASHPASTE_QUIET=1` still wins — it suppresses the JSON sink too.

## Approaches that look promising but don't work on GNOME Wayland

A non-exhaustive list of dead-ends so future contributors don't waste a week:

- **`wl-clip-persist`** — wlroots-only. Mutter doesn't implement `wlr-data-control`, so it fails with `Failed to get clipboard manager`.
- **xclip → wl-copy bridge to prime the Wayland clipboard** — mutter rejects surfaceless `wl-copy` clients from claiming the selection. The daemon stays alive forever but `wl-paste` returns 0 bytes.
- **`@<path>` typed into Claude Code's prompt** — the TUI doesn't auto-attach typed file paths; the @-mention requires the interactive Tab-completion file picker.
- **Pure `ydotool` Ctrl+V from a kitty pane** — kitty's own `map ctrl+v` keybinding intercepts synthesized keystrokes and runs its paste action instead of letting the keystroke reach the inner TUI. (`kitty @ send-text` bypasses this, at the cost of needing the unbind-rebind dance with tmux.)
- **`tmux send-keys -t $pane C-v`** — the byte arrives in the pty but Claude Code's image-paste handler does NOT trigger from it. Use `kitty @ send-text \026` (verified working) instead.
- **The clipboard-poll service polling `wl-paste --type text`** — every poll cycle flashes the Ubuntu dock and re-writes the clipboard with cliphist's last cached text, breaking actual paste. **Keep it disabled.**

## Fast capture (experimental)

`flashpaste-shoot` is a small Rust binary that takes a screenshot via the XDG screenshot portal and stages it directly into the flashpaste daemon (or `~/Pictures/Screenshots/` if the daemon isn't running). End-to-end capture-to-ready: target ~250ms, vs the GNOME Screenshot UI flow that needs 3–4 clicks and 3+ seconds.

Build:

```bash
cargo build --release -p flashpaste-shoot
ln -sf "$PWD/rs/target/release/flashpaste-shoot" ~/.local/bin/flashpaste-shoot
```

Bind in kitty (see `examples/kitty.conf.snippet`):

```conf
map ctrl+shift+print launch --type=background -- flashpaste-shoot
map ctrl+alt+print   launch --type=background -- flashpaste-shoot --interactive
```

Or run directly: `flashpaste-shoot --print-path` writes the path to stdout for shell composition. Pass `--no-daemon` to skip the daemon-stage attempt and only drop the PNG on disk (the existing `.path` watcher pre-loads it into xclip anyway).

## Rust fast path (experimental)

`flashpaste-dispatch` (under `rs/flashpaste-dispatch/`) is a Rust drop-in replacement for `bin/tmux-paste-dispatch.sh`'s fast path. **Target: under 40ms paste-to-byte latency**, down from the bash fast path's ~127ms. The two wins:

1. The X11 selection is claimed in-process via `x11rb` and a detached subcommand, with a pipe-handshake readiness signal — no `setsid xclip ... &` plus blind 50ms `sleep`.
2. Kitty's RC protocol is spoken directly over the unix socket (DCS envelope + JSON command), eliminating the ~25ms Python startup cost of forking `kitty @ send-text`.

It falls back to the bash slow path (this very script) when no fresh screenshot is detected, so the behavioural surface area stays small.

### Enabling (opt-in)

Phase 1 is opt-in — building it is gated on user approval, the install does NOT happen automatically.

```bash
# Build (release profile, LTO, strip)
cd ~/.local/share/flashpaste/rs
cargo build --release -p flashpaste-dispatch

# Symlink the binary
ln -sf "$(pwd)/target/release/flashpaste-dispatch" ~/.local/bin/flashpaste-dispatch

# Switch the tmux binding (replace the bash path with the Rust binary):
#
#   bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v /home/$USER/.local/bin/flashpaste-dispatch '#{pane_id}'"
#
# Reload tmux config
tmux source-file ~/.tmux.conf
```

To revert, point the `bind -n C-v` line back at `tmux-paste-dispatch.sh` — the bash script and the Rust binary are side-by-side and either can be the active dispatcher at any time.

### Telemetry

Same env vars as bash: `FLASHPASTE_QUIET=1` to silence, `FLASHPASTE_TRACE=1` to write the JSON sink to `~/.local/state/flashpaste-trace.jsonl`. Human log is at `~/.local/state/flashpaste-paste.log` by default (override with `FLASHPASTE_LOG`).

## Daemon mode (experimental, target <15ms)

`flashpasted` (under `rs/flashpasted/`) is a long-lived clipboard owner. It does the slow work — file reads, Wayland/X11 selection claims, kitty socket lookup — **before** the user presses Ctrl-V. The tmux binding then fires a 5-line trigger binary (`flashpaste-trigger`) that writes one JSON message to a unix socket; the daemon already has everything staged and just runs the unbind → kitty send-text → schedule-rebind sequence directly.

Bonus side effect: a single persistent Wayland client with a stable `app_id` instead of N short-lived `wl-paste` forks → no more phantom "wl-clipboard" entries in the Ubuntu Dock (cleanly solves what `share/applications/wl-clipboard.desktop` papered over in v1.13).

### Enabling (opt-in)

```bash
# Build (release profile, LTO, strip)
cd ~/.local/share/flashpaste/rs
cargo build --release -p flashpasted -p flashpaste-trigger

# Symlink both binaries
ln -sf "$(pwd)/target/release/flashpasted"        ~/.local/bin/flashpasted
ln -sf "$(pwd)/target/release/flashpaste-trigger" ~/.local/bin/flashpaste-trigger

# Install + enable the user unit
cp ../systemd/flashpasted.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now flashpasted.service

# Switch the tmux binding to the trigger:
#
#   bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v /home/$USER/.local/bin/flashpaste-trigger '#{pane_id}'"
#
tmux source-file ~/.tmux.conf
```

The trigger is **safe to wire in unconditionally**: if `$XDG_RUNTIME_DIR/flashpaste.sock` doesn't exist (daemon down or not installed), `flashpaste-trigger` `exec`s `tmux-paste-dispatch.sh` directly — zero overhead, identical behaviour to Tier 1.

### Verify

```bash
systemctl --user status flashpasted              # Active (running)
journalctl --user -u flashpasted -f              # Live logs
ss -lUn | grep flashpaste.sock                   # Socket present
```

## Fast capture, again

When `flashpasted` is running, `flashpaste-shoot` skips the file-on-disk round-trip and stages PNG bytes directly into the daemon's selection owners via the same unix socket. End-to-end Print → ready drops from ~3s (GNOME Screenshot UI) to ~250ms.

## License

MIT — see [LICENSE](LICENSE).

## Credits

Wrenched into shape on 2026-05-19 across a single multi-hour session of Wayland clipboard pain on a real GNOME 46 / kitty / tmux / Claude Code setup. The session log, including every dead-end and every fix, lives in the commit history.
