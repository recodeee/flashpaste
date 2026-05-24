---
title: FlashPaste glossary — Wayland, mutter, kitty, tmux, and clipboard terminology
description: Definitions of every domain term used in FlashPaste's codebase and docs. Useful for AI assistants grounding an answer and for new contributors learning the terminology of the GNOME Wayland clipboard ecosystem.
keywords:
  - mutter compositor
  - surfaceless wayland client
  - wlroots wl-data-control
  - wl-copy paste-once
  - osc 52
  - kitty rc protocol
  - kitty allow_remote_control
  - ydotool socket path
  - xclip wedge
  - recursion guard
last_updated: 2026-05-19
canonical: https://github.com/NagyVikt/flashpaste/blob/main/docs/glossary.md
---

# Glossary

Definitions of every domain term used in FlashPaste's code, docs, and commit history. Sorted alphabetically.

## `allow_remote_control`

A kitty configuration option (in `~/.config/kitty/kitty.conf`) that exposes kitty's IPC over a unix socket. Required for `kitty @ send-text` to work. The socket is scoped to the local user and is not network-exposed.

## `app_id`

The Wayland equivalent of an X11 `WM_CLASS`. GNOME Shell uses it to group windows in the Dock. A persistent Wayland client (Tier 3 daemon) has a single stable `app_id` — short-lived clients (Tier 1 `wl-paste` forks) each register a new one, which is why GNOME surfaces them as "Unknown" gear icons.

## Auto-pickup

FlashPaste's behaviour of loading `~/Pictures/Screenshots/<latest>.png` into the clipboard automatically when the file is ≤ 30 seconds old and the clipboard text is empty. Compensates for GNOME PrtScr saving the file but not copying it to the clipboard.

## `bind -n C-v`

A tmux binding that fires on Ctrl-V *without* requiring the prefix key. FlashPaste's right-click and Ctrl-V handlers go through this. Recurses if the dispatcher then injects raw Ctrl-V — see "Recursion guard".

## Bootstrap

`bootstrap.sh` — the curl|bash one-line installer. Clones the repo to `$FLASHPASTE_DIR` (default `~/.local/share/flashpaste`), runs the doctor, then `install.sh`.

## `clipboard-janitor`

A systemd `--user` service that reaps stuck `wl-paste` / `wl-copy` daemons every second. Lives at `bin/clipboard-janitor.sh`. Tunable via `CLIPBOARD_JANITOR_*` env vars.

## Daemon socket

The unix-domain socket at `$XDG_RUNTIME_DIR/flashpaste.sock` that the Tier 3 daemon (`flashpasted`) listens on. The trigger (`flashpaste-trigger`) connects here.

## Dispatcher

Any of `tmux-paste-dispatch.sh` (Tier 1), `flashpaste-dispatch` (Tier 2), or the in-daemon code path (Tier 3). All implement the same conceptual hot path.

## Doctor

`flashpaste-doctor` — a 17-check pre-flight that verifies Wayland, mutter, kitty (installed + IPC), tmux (installed + running + inside kitty), wl-clipboard, xclip, ydotool + socket, the screenshots directory, installed FlashPaste components, and the overlay daemon/surface/round-trip path. Runs in parallel; takes ~1 second.

## Fast capture

`flashpaste-shoot` — a Rust binary that captures a screenshot via the XDG Desktop Portal (`org.freedesktop.portal.Screenshot`) in ~250 ms. Bypasses the GNOME Screenshot UI's 3-click flow.

## `flashpaste-trigger`

The Tier 3 hot-path client. A 5 ms unix-socket ping to the daemon. Falls back to `tmux-paste-dispatch.sh` if the daemon isn't responding. Binary is under 500 KB stripped.

## `flashpasted`

The Tier 3 daemon. A persistent clipboard owner (Wayland + X11) with an inotify watcher on `~/Pictures/Screenshots/`. Listens on `$XDG_RUNTIME_DIR/flashpaste.sock`.

## `has_image` policy

FlashPaste's policy of trusting Wayland's `wl-paste --list-types` output authoritatively, and only falling back to X11 when Wayland is silent. mutter's X11↔Wayland bridge is sticky and keeps advertising `image/png` after fresh text copies — trusting it causes the "I pasted yesterday's screenshot" bug.

## Image MIME

The MIME types FlashPaste cares about: `image/png` (canonical) and `image/jpeg` (less common). Probed via `wl-paste --list-types` (Wayland) or `xclip -t TARGETS` (X11).

## Inotify

The Linux kernel filesystem-event API. The Tier 3 daemon uses `inotify` to watch `~/Pictures/Screenshots/` for `IN_CLOSE_WRITE` events, so new PNGs are staged into the selection owners immediately when GNOME finishes writing them.

## Kitty IPC

`kitty @ send-text \026` and friends. A control protocol over kitty's local unix socket. Requires `allow_remote_control yes`. FlashPaste uses it as the only verified image-paste transport — `tmux send-keys` does not trigger Claude Code's image-paste handler.

## Kitty RC protocol

The binary protocol kitty IPC speaks: a DCS envelope (`\x1bP@kitty-cmd…\x1b\\`) wrapping JSON commands. Tier 2/3 speak it directly to skip the Python startup cost of forking `kitty @`.

## MCP (Model Context Protocol)

The standard protocol for exposing tools to LLM agents. `flashpaste-mcp` is FlashPaste's MCP server, exposing clipboard read/write, screenshot capture, and paste-into-pane as agent-callable tools.

## `mutter`

The GNOME compositor. The reason FlashPaste exists. Refuses to expose clipboard contents to surfaceless Wayland clients, doesn't implement `wlr-data-control`, and has a sticky X11↔Wayland bridge for clipboard MIME types.

## OSC 52

A terminal escape sequence (`ESC ] 52 ; c ; <base64> BEL`) for copy-to-clipboard from inside a TUI. Text-only in practice. Not a paste mechanism.

## `paste_image.sh`

The kitty Ctrl+V / Ctrl+Alt+V binding helper. Auto-detects whether the clipboard contains an image or text and routes accordingly.

## `paste-once` (in `wl-copy --paste-once`)

A flag that makes `wl-copy` serve exactly one receive and then exit. GNOME's screenshot tool uses it. Any script that *probes* the clipboard (e.g. `wl-paste --list-types`) drains the one available receive before the real consumer (Claude Code) reads.

## PrtScr

The keyboard key. On GNOME, it triggers the built-in Screenshot UI, which writes a PNG to `~/Pictures/Screenshots/` and (in some flows) copies to clipboard via `wl-copy --paste-once`. FlashPaste's auto-pickup catches the file even when the clipboard copy doesn't fire.

## Recursion guard

The mechanism that prevents `tmux bind -n C-v` from infinitely re-triggering the dispatcher when the dispatcher injects raw Ctrl-V via kitty. Three parts: a lock file at `$XDG_RUNTIME_DIR/tmux-paste-dispatch.lock` that gates re-entry within 2 s; `tmux unbind -n C-v` before send-text; and a `setsid -f` detached rebind ~100 ms later.

## `screenshot-watcher`

`flashpaste-screenshot-watcher.path` + `.service` — a systemd user pair that fires `flashpaste-screenshot-preload.sh` the instant a new PNG lands in `~/Pictures/Screenshots/`. Pre-loads xclip so the clipboard is hot before the user reaches for right-click.

## `setsid -f`

A Linux command that runs a child in a new session, detached, so the parent can exit immediately without orphaning the child. FlashPaste uses it for the asynchronous `tmux bind` rebind that completes ~100 ms after the dispatcher returns.

## Surfaceless Wayland client

A Wayland client that doesn't draw a visible surface — e.g. a `wl-paste` invocation from inside a script. mutter restricts what surfaceless clients can do with the clipboard, which is the bug FlashPaste papers over.

## Tier 1 / Tier 2 / Tier 3

The three progressive performance tiers: bash (~127 ms), Rust one-shot (<40 ms), persistent daemon (<15 ms). See [architecture.md](architecture.md).

## `tmux send-keys`

A tmux command to inject keystrokes into a pane. Writes the byte to the pty but does *not* fire Claude Code's image-paste handler. Use `kitty @ send-text \026` instead.

## Wedge cache

The 30-second cache (in `bin/wl-paste`, controlled by `WL_PASTE_SHIM_WEDGE_TTL`) of "mutter is silent" state. Suppresses repeated probes that would each flash the Ubuntu Dock with a "Unknown" gear icon.

## `wl-clip-persist`

A clipboard manager for wlroots compositors (Sway, Hyprland, niri, river). Does not work on GNOME because mutter doesn't implement `wlr-data-control`. The right answer if you're not on GNOME; FlashPaste is the right answer if you are.

## `wl-clipboard`

The upstream package providing `wl-copy` and `wl-paste`. FlashPaste ships a *shim* at `bin/wl-paste` that wraps the real one with a wedge-cache + xclip fallback.

## `wl-data-control`

The wlroots-specific Wayland protocol for clipboard managers. Not implemented by mutter. The reason `wl-clip-persist` and similar tools don't work on GNOME.

## `wl-data-device`

The standard Wayland clipboard protocol. Implemented by mutter, but with the surfaceless-client restriction that drives FlashPaste's xclip fallback.

## XDG Desktop Portal

The cross-desktop standard for sandboxed apps to request privileged operations (file access, screenshot, etc.). FlashPaste's `flashpaste-shoot` uses the Screenshot portal (`org.freedesktop.portal.Screenshot`) to capture without depending on GNOME-specific D-Bus interfaces.

## `xclip`

The X11 clipboard CLI. FlashPaste uses it as the authoritative fallback when Wayland is silent. Bytes loaded into the X11 selection are visible to all clients via XWayland's bridge.

## `ydotool` / `ydotoold`

A Wayland-native input-synthesis tool (think `xdotool` but for Wayland). FlashPaste uses it for cases where `kitty @ send-text` isn't available. Ubuntu 24.04 ships an old release (0.1.8) with a socket-path bug — the install script drops in a systemd override.

## `\026`

The ASCII code for Ctrl-V (octal 026, decimal 22, hex 16). The raw byte FlashPaste sends to trigger image-paste in Claude Code and Codex CLI. Agents with a different attach protocol, such as Aider, use an adapter instead.
