---
title: FlashPaste use cases — pasting images into Claude Code, Codex CLI, Aider on Linux
description: Concrete walkthroughs for the most common FlashPaste workflows. Pasting screenshots into Claude Code on Ubuntu 24.04, Codex CLI on Fedora 40, Aider on Debian 13, agent-driven captures via MCP, and area screenshots with flashpaste-shoot.
keywords:
  - paste screenshot into claude code ubuntu
  - codex cli image paste fedora
  - aider image paste debian
  - flashpaste mcp screenshot
  - terminal ai screenshot workflow
  - claude code linux workflow
  - flashpaste-shoot area capture
last_updated: 2026-05-19
canonical: https://github.com/NagyVikt/flashpaste/blob/main/docs/use-cases.md
---

# Use cases

Concrete walkthroughs for the most common FlashPaste workflows. Each section is self-contained — pick the one that matches your stack and your task.

## Pasting a screenshot into Claude Code on Ubuntu 24.04

The reference workflow. Tested daily on a real Ubuntu 24.04 / kitty / tmux / Claude Code setup.

1. Open kitty. Inside it, start tmux. Inside tmux, run `claude` to attach to Claude Code.
2. Position the GNOME window you want to screenshot.
3. Press **PrtScr**. GNOME's screenshot UI appears. Click "Screenshot" or hit Enter to capture full screen — the PNG lands in `~/Pictures/Screenshots/`.
4. Right-click anywhere in the tmux pane → **Paste**.
5. The image attaches to your Claude Code prompt in ~120 ms (Tier 1) or ~15 ms (Tier 3).

Variations:

- Use **Ctrl+Shift+PrtScr** (kitty binding from the snippet) to capture via the XDG portal directly into the daemon — faster path, no GNOME UI involvement.
- Use **Ctrl+Alt+PrtScr** for the area-picker variant.
- Use **Ctrl+Alt+V** to force the image-paste path when you suspect clipboard text is shadowing it.

## Pasting screenshots into Codex CLI on Fedora 40

Identical to the Ubuntu flow. Fedora 40 ships a recent enough `ydotool` that the socket-path drop-in is a no-op, but installing it via `install.sh` is harmless.

```bash
# Bootstrap install (no apt on Fedora)
curl -fsSL https://raw.githubusercontent.com/NagyVikt/flashpaste/main/bootstrap.sh | bash

# Required deps via dnf
sudo dnf install wl-clipboard xclip xsel ydotool tmux kitty

# Inside kitty, start tmux, then:
codex
# (Codex CLI starts. PrtScr → right-click → Paste works identically.)
```

## Pasting screenshots into Aider on Debian 13

Debian 13 (Trixie) is binary-compatible with the Ubuntu `.deb`:

```bash
curl -fsSL -o /tmp/flashpaste.deb \
  https://github.com/NagyVikt/flashpaste/releases/latest/download/flashpaste_all.deb
sudo apt install /tmp/flashpaste.deb
```

Aider accepts image paste via the same Ctrl-V sentinel Claude Code uses. The standard FlashPaste right-click → Paste flow attaches the image directly.

## Fast captures with `flashpaste-shoot`

GNOME's screenshot UI is good but slow (3–4 clicks, ~3 seconds). `flashpaste-shoot` is a Rust binary that captures via the XDG Desktop Portal in ~250 ms and stages directly into the daemon (or `~/Pictures/Screenshots/` if the daemon isn't running).

```bash
flashpaste-shoot                 # full screen, immediate
flashpaste-shoot --interactive   # area picker
flashpaste-shoot --print-path    # write PNG path to stdout
flashpaste-shoot --no-daemon     # skip daemon staging, just write to disk
```

Wire to a kitty keybinding (already in `examples/kitty.conf.snippet`):

```conf
map ctrl+shift+print launch --type=background -- flashpaste-shoot
map ctrl+alt+print   launch --type=background -- flashpaste-shoot --interactive
```

## Agent-driven screenshots via MCP

`flashpaste-mcp` exposes clipboard + screenshot tools to LLM agents over the stdio MCP transport. Useful for "take a screenshot of the current screen and analyze it" loops where the agent is driving instead of the user.

Register the MCP server with your agent (Claude Code, Codex, etc.):

```json
{
  "mcpServers": {
    "flashpaste": {
      "command": "flashpaste-mcp"
    }
  }
}
```

Tools the agent can call:

- `screenshot` — capture full-screen via the XDG portal, returns base64 PNG
- `screenshot_area` — interactive area capture
- `clipboard_read` — read current clipboard (text or image)
- `clipboard_write` — write text to clipboard
- `paste_into_pane` — inject paste into a specific tmux pane

Status: experimental. The contract is stable enough to depend on but the tool schema may evolve.

## Multi-paste the same screenshot

The dispatcher's recursion guard auto-clears after 2 seconds, so you can paste the same screenshot multiple times into different panes:

1. PrtScr once.
2. Right-click pane 1 → Paste. Image attaches.
3. Move to pane 2. Right-click → Paste. Image attaches again from the same source.
4. Repeat as many times as you need until the screenshot ages past the 30 s auto-pickup window.

## Drive FlashPaste from a script

```bash
# Stage a PNG into the clipboard from anywhere
~/.local/bin/clipboard-set.sh < /path/to/image.png

# Read current clipboard text
~/.local/bin/get-clipboard-text.sh

# Take a screenshot, get the path
PNG=$(flashpaste-shoot --print-path --no-daemon)
echo "Saved to: $PNG"

# Force a paste into a specific tmux pane
flashpaste-trigger '%3'   # or fall back: tmux-paste-dispatch.sh '%3'
```

## Combining FlashPaste with claude-mem or other Claude Code plugins

FlashPaste lives at the *clipboard* layer; Claude Code plugins live at the *agent* layer. They compose cleanly — no conflicts. A typical flow:

1. PrtScr (capture)
2. Right-click → Paste (FlashPaste delivers the image to Claude Code)
3. Claude Code analyzes the image (agent layer)
4. `claude-mem` saves the observation (memory layer)

Each layer is independent.
