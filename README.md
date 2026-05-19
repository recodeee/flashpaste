# flashpaste

> Sub-120ms image-paste into terminal AI agents (Claude Code, Codex, etc.) on GNOME Wayland — even when mutter's clipboard is broken.

If you've ever tried to paste a screenshot into a TUI app running inside tmux inside kitty on GNOME 46 / mutter Wayland, you know what 15 frustrated `Ctrl+V` presses feel like. **flashpaste** is the fix.

## What it does

- **Image paste works on the *first* press.** No matter how wedged mutter's clipboard is.
- **PrtScr → right-click → Paste.** No extra clipboard helper, no manual file dance.
- **Multi-paste the same screenshot.** Hammer it as many times as you want, in any pane.
- **Falls back gracefully.** xclip (XWayland), file-based pre-stage, recursion guard, wedge cache.
- **End-to-end timing telemetry.** Every step is logged with `T+<ms> Δ<ms>` so regressions are visible at a glance.

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

```bash
git clone https://github.com/NagyVikt/flashpaste.git
cd flashpaste
./install.sh
```

The installer:
1. Symlinks scripts into `~/.local/bin/`.
2. Drops a systemd user service (`clipboard-janitor.service`) that reaps stuck `wl-copy` / `wl-paste` daemons.
3. Prints the tmux + kitty config snippets you need to copy into your dotfiles.

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

## Approaches that look promising but don't work on GNOME Wayland

A non-exhaustive list of dead-ends so future contributors don't waste a week:

- **`wl-clip-persist`** — wlroots-only. Mutter doesn't implement `wlr-data-control`, so it fails with `Failed to get clipboard manager`.
- **xclip → wl-copy bridge to prime the Wayland clipboard** — mutter rejects surfaceless `wl-copy` clients from claiming the selection. The daemon stays alive forever but `wl-paste` returns 0 bytes.
- **`@<path>` typed into Claude Code's prompt** — the TUI doesn't auto-attach typed file paths; the @-mention requires the interactive Tab-completion file picker.
- **Pure `ydotool` Ctrl+V from a kitty pane** — kitty's own `map ctrl+v` keybinding intercepts synthesized keystrokes and runs its paste action instead of letting the keystroke reach the inner TUI. (`kitty @ send-text` bypasses this, at the cost of needing the unbind-rebind dance with tmux.)
- **`tmux send-keys -t $pane C-v`** — the byte arrives in the pty but Claude Code's image-paste handler does NOT trigger from it. Use `kitty @ send-text \026` (verified working) instead.
- **The clipboard-poll service polling `wl-paste --type text`** — every poll cycle flashes the Ubuntu dock and re-writes the clipboard with cliphist's last cached text, breaking actual paste. **Keep it disabled.**

## License

MIT — see [LICENSE](LICENSE).

## Credits

Wrenched into shape on 2026-05-19 across a single multi-hour session of Wayland clipboard pain on a real GNOME 46 / kitty / tmux / Claude Code setup. The session log, including every dead-end and every fix, lives in the commit history.
