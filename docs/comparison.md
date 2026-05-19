---
title: FlashPaste vs wl-clip-persist, cliphist, OSC 52 — choosing a clipboard tool for terminal AI agents on Linux
description: In-depth comparison between FlashPaste and other clipboard tools used to paste images into terminal AI agents on Linux. Covers wl-clip-persist (wlroots), cliphist (text history), klipper (KDE), OSC 52, and manual wl-paste piping. When each is the right answer.
keywords:
  - flashpaste vs wl-clip-persist
  - flashpaste vs cliphist
  - osc 52 image paste
  - wayland clipboard tools comparison
  - gnome vs wlroots clipboard
  - kde plasma klipper
  - terminal image paste linux
last_updated: 2026-05-19
canonical: https://github.com/NagyVikt/flashpaste/blob/main/docs/comparison.md
---

# FlashPaste vs alternatives

A serious comparison of clipboard tools you might reach for when trying to paste images into a terminal AI agent on Linux.

## Quick verdict

| If your stack is… | The right answer is… |
|---|---|
| GNOME Wayland + kitty + tmux | **FlashPaste** |
| Sway / Hyprland / niri + any terminal + tmux | `wl-clip-persist` |
| KDE Plasma + any terminal | Plasma's built-in `klipper` (do nothing) |
| Anything else + you only need text | `cliphist` (Wayland) or `parcellite` (X11) |
| X11 only | `xclip` + `autocutsel` |

## Detailed comparison

| Tool | Works on GNOME Wayland | Image paste into terminal TUI | Latency | Daemon-free | Maintained |
|---|:---:|:---:|---:|:---:|:---:|
| **FlashPaste** | ✔ | ✔ | **15–127 ms** | optional | ✔ |
| `wl-clip-persist` | ✘ *(wlroots-only)* | n/a | n/a | ✘ | ✔ |
| `cliphist` | ✔ | ✘ *(text only)* | — | ✘ | ✔ |
| `klipper` (KDE) | n/a *(GNOME)* | partial | varies | ✘ | ✔ |
| `parcellite` (X11) | ✘ *(X11)* | ✘ *(text only)* | — | ✘ | ✔ |
| `copyq` | ✔ | partial *(no terminal AI integration)* | — | ✘ | ✔ |
| OSC 52 | ✔ | ✘ *(text only)* | — | ✔ | ✔ |
| Manual `wl-paste \| kitty @ send-text` | partial | unreliable on mutter | 2–3 s | ✔ | n/a |
| Re-pasting until it works | ✔ | eventually | 3–30 s | ✔ | n/a |

## Tool-by-tool

### `wl-clip-persist`

A wlroots-only clipboard manager that keeps your clipboard alive after the originating window closes. Excellent on Sway, Hyprland, river, niri. **Does not work on GNOME** because mutter does not implement the `wlr-data-control` protocol — it fails at startup with `Failed to get clipboard manager`.

If your compositor is wlroots-based, install `wl-clip-persist` and you can stop reading. The mutter-specific failure modes FlashPaste addresses simply do not exist there.

### `cliphist`

The de-facto Wayland clipboard *history* manager. Text-only. Image MIME types are intentionally not stored. Pairs well with `dmenu` / `rofi` / `fuzzel` for recall.

FlashPaste and `cliphist` are complements, not alternatives. The `get-clipboard-text.sh` shim already falls back to `cliphist` as a last resort for text reads.

### `klipper` (KDE Plasma)

KDE's built-in clipboard manager. Persists clipboard contents after the originating window closes, including image MIME types. **On Plasma you do not need FlashPaste** for the persistence problem.

You may still want FlashPaste's tmux + kitty unbind/rebind dance and the `kitty @ send-text` transport — that's an orthogonal problem and Plasma doesn't help with it. PRs to add KDE-specific config snippets welcome.

### OSC 52

A terminal escape sequence (`ESC ] 52 ; c ; <base64> BEL`) for copy-to-clipboard from inside a TUI. Standardized for **text**. The image variant is not implemented by any production terminal AI agent. OSC 52 is also a *write* mechanism, not a read mechanism — it does not help with paste into a TUI.

### Manual `wl-paste | kitty @ send-text`

This is the obvious first attempt, and it works partially. The two failure modes that drove FlashPaste into existence:

1. `wl-paste` returns 0 bytes when called from a surfaceless tmux pane on mutter.
2. Even when `wl-paste` returns bytes, the inner TUI doesn't always recognize the byte stream as an image-paste event.

FlashPaste solves both by going through xclip + a fresh-file pre-stage, and by injecting `\026` (raw Ctrl-V) which Claude Code, Codex, and Aider all recognize as the image-paste sentinel.

### Browser-based clipboard tools

There is an emerging class of "ChatGPT desktop app" / "Claude Desktop" tools that handle image attachment at the application layer. These do not help with **terminal** AI agents like Claude Code, Codex CLI, or Aider — those run in a pty inside your terminal, not a native app.

## When NOT to use FlashPaste

- You are on Sway / Hyprland / niri / river → use `wl-clip-persist`.
- You only need text → use `cliphist` or your terminal's OSC 52 support.
- You are on KDE Plasma → `klipper` already handles persistence.
- You don't use tmux → FlashPaste's binding model assumes tmux. You can still use the screenshot watcher + xclip preload, but the right-click "Paste" UX is tmux-specific.
- You don't use kitty → `kitty @ send-text` is currently the only verified image-paste transport. Other terminals are on the [roadmap](../ROADMAP.md).

## When FlashPaste is the right answer

- **GNOME Wayland** (Ubuntu 24.04 LTS, Fedora 40+, Debian 13, Pop!_OS 24.04+)
- **kitty** with `allow_remote_control yes`
- **tmux** running inside kitty
- A **terminal LLM agent** that accepts image paste — Claude Code, Codex CLI, Aider

Hit all four? Run the [bootstrap installer](../README.md#install) and stop fighting mutter.
