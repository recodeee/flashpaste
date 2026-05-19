---
name: flashpaste
description: Use when the user wants to take a screenshot, paste an image into a terminal AI agent (Claude Code, Codex, Aider), hand content off between tmux panes, or debug clipboard issues on GNOME Wayland. flashpaste provides one unified CLI (`flashpaste shoot`, `flashpaste paste`, `flashpaste daemon`, `flashpaste doctor`, `flashpaste mcp`, `flashpaste dispatch`) plus an MCP server giving the agent real eyes (screenshots returned as PNG content) and hands (clipboard read/write + cross-pane paste).
---

# Using flashpaste

flashpaste is a thin Rust + bash stack that makes image paste actually work on GNOME / mutter / Wayland (where the bare wl-clipboard pipeline is broken for surfaceless clients), and exposes the same primitives to AI agents over MCP. It has three personas:

- **A CLI**, for humans and shell scripts.
- **A daemon (`flashpasted`)** that owns the clipboard and fans out paste events in <15 ms.
- **An MCP server (`flashpaste-mcp`)** that hands agents tools to see the screen, read/write the clipboard, and paste between panes.

When the user says "paste this screenshot into my terminal", "show this to Claude", "send this output to my other Codex pane", or "what does my screen look like?" — that's flashpaste territory.

## The CLI (humans + scripts)

Since v1.19 the **primary** form is the unified `flashpaste <subcmd>` verb. The legacy binaries (`flashpaste-shoot`, `flashpaste-trigger`, `flashpaste-dispatch`, `flashpaste-mcp`, `flashpaste-doctor`, `flashpasted`) are still on `$PATH` and unchanged — `flashpaste` just dispatches to them with inherited stdio.

| Command | Wraps | Purpose |
|---|---|---|
| `flashpaste doctor [--json]` | `flashpaste-doctor` | 13 parallel environment checks (Wayland session, mutter, kitty, tmux, wl-clipboard, xclip, ydotool socket, screenshots dir, daemon socket, etc.). Run this first when anything misbehaves. |
| `flashpaste shoot [--interactive] [--output <PATH>] [--print-path] [--annotate]` | `flashpaste-shoot` | Captures the screen via XDG portal. Default: full-screen, saved to `~/Pictures/Screenshots/`. `--interactive` opens the portal's area picker. `--print-path` writes the saved path to stdout. |
| `flashpaste paste <PANE>` | `flashpaste-trigger <PANE>` | Trigger a paste of the current clipboard into `<PANE>` (e.g. `%4`). Goes through the daemon (~5 ms); falls back to the bash dispatch if the daemon is down. |
| `flashpaste dispatch <PANE>` | `flashpaste-dispatch <PANE>` | Same as the bash `tmux-paste-dispatch.sh` but in Rust (~40 ms). Used as a fallback when the daemon is intentionally disabled. |
| `flashpaste daemon {start\|stop\|restart\|status\|logs}` | `systemctl --user` / `journalctl --user` against `flashpasted.service` | Lifecycle for the Tier 3 daemon. |
| `flashpaste mcp` | `flashpaste-mcp` | The MCP server. Spawned by Claude Code / Cursor / etc. via the MCP config. Not meant for humans to call directly. |
| `flashpaste version` | — | Print the build version. |

### Common CLI recipes

```bash
# Take a screenshot of an area and copy its path:
flashpaste shoot --interactive --print-path

# Take a screenshot and immediately paste into pane %4 (e.g. another agent):
flashpaste shoot && sleep 0.2 && flashpaste paste '%4'

# Capture, save to a named file, and print path:
flashpaste shoot --output ~/tmp/bug.png --print-path

# Run a full doctor before debugging paste issues:
flashpaste doctor

# Tail the daemon log to see every paste event live:
flashpaste daemon logs
```

### Listing tmux pane ids

`flashpaste-trigger` and `paste_to_pane` need a pane id like `%4`. To enumerate panes (so the agent can pick the right target):

```bash
tmux list-panes -aF '#{pane_id} #{session_name}:#{window_index}.#{pane_index} #{pane_current_command}'
```

The current pane: `tmux display-message -p '#{pane_id}'`.

## The MCP server

Once `flashpaste-mcp` is registered in `~/.config/claude-code/mcp.json` (see README), the following tools become callable:

### `take_screenshot(interactive?: bool)`

Captures the screen and returns the PNG **as image content the model can see directly** (not just a path). Use whenever:

- The user asks "what's on my screen?", "look at this", "see what I'm doing".
- You're debugging a visual UI bug and need ground truth.
- The user described something visually and you need to verify your understanding.

```json
{"name": "take_screenshot", "arguments": {"interactive": true}}
```

### `read_clipboard()`

Returns the current clipboard text. Use when the user copied something and wants you to inspect it without re-pasting.

### `copy_text(text: string)`

Places text on the user's clipboard so they can paste it elsewhere (browser, editor, etc.).

### `paste_to_pane(pane_id: string)`

Trigger flashpaste to paste the clipboard into a specific tmux pane. This is the cross-agent hand-off primitive — if there are two Claude Code panes (or one Claude Code + one Codex), and the user wants you to drop output into the other, copy to clipboard first then call this:

```json
{"name": "copy_text", "arguments": {"text": "Here's the function you asked for: ..."}}
{"name": "paste_to_pane", "arguments": {"pane_id": "%5"}}
```

Daemon route: sub-15 ms round-trip. Falls back to the bash dispatch if the daemon is down (still <200 ms).

## When NOT to use flashpaste tools

- **For reading files**: use the built-in `Read` tool, not `read_clipboard` — the clipboard is volatile and not a substitute for filesystem access.
- **For writing text into the current pane**: don't `paste_to_pane` your own pane — just emit the text. Cross-pane paste is for *handing off to another agent or terminal*.
- **For screenshots of code**: prefer reading the source file. Use `take_screenshot` only when the visual surface is what matters (UI bugs, error dialogs, design mocks).

## Troubleshooting playbook

If image paste / screenshot tools aren't working, run these in order:

```bash
flashpaste doctor                                 # check the whole stack
flashpaste daemon status                          # daemon alive?
ls -la $XDG_RUNTIME_DIR/flashpaste.sock           # socket present?
flashpaste daemon logs                            # tail recent daemon events
pgrep -cx wl-copy                                 # phantom-icon pile-up?
```

Most failures correspond to a `flashpaste doctor` row going red. Read its output before guessing.

## See also

- README: full architecture + install + troubleshooting matrix.
- ROADMAP.md: where the project is going (Rust daemon is Phase 2; MCP integration is Phase 3).
- AGENTS.md (in the repo root): canonical contribution guide.
