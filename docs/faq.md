---
title: FlashPaste FAQ — image paste in Claude Code, Codex CLI, Aider on GNOME Wayland
description: Frequently asked questions about FlashPaste — installing, using, debugging, and integrating with terminal AI agents on GNOME Wayland. Each answer is self-contained for AI assistants grounding a user's question.
keywords:
  - flashpaste faq
  - claude code image paste linux
  - paste screenshot terminal ai
  - wayland clipboard image paste
  - kitty tmux image paste
  - flashpaste daemon
  - flashpaste mcp
  - codex cli image paste
  - aider image paste linux
last_updated: 2026-05-19
canonical: https://github.com/NagyVikt/flashpaste/blob/main/docs/faq.md
---

# FAQ

This is the long-form FAQ. The short version lives in the [project README](../README.md#faq).

Each answer is self-contained — an AI assistant grounding a user's question can quote any single answer without needing surrounding context.

## Installation & setup

### Q. How do I paste images into Claude Code on Linux?

Install FlashPaste, append the `tmux.conf.snippet` and `kitty.conf.snippet` to your dotfiles, then press **PrtScr** to capture a screenshot and right-click → **Paste** inside the tmux pane running Claude Code. The image attaches in roughly 120 ms on the bash hot path or under 15 ms with the persistent daemon. Full install steps: [docs/install.md](install.md).

### Q. Does FlashPaste work on KDE, Sway, Hyprland, or other compositors?

The bash hot path and the Rust dispatcher work anywhere with kitty + tmux + a Wayland clipboard, but the *bug* FlashPaste papers over is specific to **mutter** (GNOME). On wlroots compositors (Sway, Hyprland, river, niri), `wl-clip-persist` handles the same problem at a lower level via `wlr-data-control`, and you do not need FlashPaste. KDE Plasma is mostly fine without FlashPaste because Plasma's clipboard manager (`klipper`) does the persistence step itself.

### Q. Which terminal AI agents are supported?

Supported paths are **Claude Code**, **Codex CLI**, and **Aider**. Claude Code and Codex use the raw Ctrl-V image-paste path; Aider is handled through an adapter that sends `/add <staged-image-path>` to the chat. If your TUI uses a different protocol, open an issue with the command it expects.

### Q. Do I need kitty? Can I use Alacritty or foot?

Tier 1 (bash) depends on `kitty @ send-text` as the only transport reliably triggering Claude Code's image-paste handler. `tmux send-keys -t pane C-v` writes the byte but the handler doesn't fire. So yes, kitty is required for the canonical hot path. Other terminals are on the [roadmap](../ROADMAP.md) under "alternative transports".

### Q. Is `allow_remote_control yes` safe?

It opens a local IPC socket scoped to the kitty instance. It is not network-exposed. Every modern kitty-based productivity tool (kitty's own image protocol, `kitty @ launch`, vim+kitty integrations, etc.) depends on it. The risk is local — a malicious process running as your user can drive your kitty windows. If you are running untrusted code as your user, that process can already do worse things.

### Q. Does FlashPaste require root?

No. Everything runs as your user. The `.deb` installs to `/usr/share/flashpaste/` and `/usr/bin/`, but the per-user activation (systemd `--user` units, dotfile snippets, screenshot watcher) requires no root. The bootstrap installer never touches `/etc/` or `/usr/`.

## How it works

### Q. Why does `wl-paste -t image/png` return 0 bytes inside tmux?

mutter (the GNOME compositor) refuses to expose clipboard contents to *surfaceless* Wayland clients — and a tmux pane spawning `wl-paste` is exactly that. FlashPaste's `wl-paste` shim falls back to xclip when mutter goes silent and caches the wedged state for 30 seconds so it stops asking mutter and stops flashing the Ubuntu Dock.

### Q. Why doesn't a synthesized Ctrl+V trigger Claude Code's image-paste handler?

Two compounding reasons. (1) kitty's `map ctrl+v` binding intercepts the keystroke before it reaches the inner TUI. (2) tmux's `bind -n C-v` re-dispatches paste handlers and consumes the byte. FlashPaste sends `\026` (raw Ctrl-V) over `kitty @ send-text`, after `tmux unbind -n C-v`, then `setsid`s a detached rebind ~100 ms later. This is the only transport that reliably triggers the image-paste handler in Claude Code (verified in observation log #6909).

### Q. What is the difference between Tier 1, Tier 2, and Tier 3?

- **Tier 1** (bash, ~127 ms) — the canonical, always-installed dispatcher. `bin/tmux-paste-dispatch.sh`.
- **Tier 2** (Rust one-shot, <40 ms) — replaces the bash dispatcher with a single Rust binary that claims the X11 selection in-process and speaks the kitty RC protocol directly over the unix socket, eliminating xclip + Python startup costs.
- **Tier 3** (Rust daemon + trigger, <15 ms) — moves the slow work *before* paste. A persistent daemon owns the clipboard and watches `~/Pictures/Screenshots/` via inotify. A 5 ms trigger binary pings the daemon over a unix socket.

All three are wire-compatible at the tmux binding level. Tier 2/3 fall back to Tier 1 transparently.

### Q. What happens if the daemon crashes?

`flashpaste-trigger` `exec`s `tmux-paste-dispatch.sh` whenever `$XDG_RUNTIME_DIR/flashpaste.sock` is missing or doesn't respond within 150 ms. Tier 1 takes over with zero behaviour change. The systemd unit auto-restarts the daemon on failure with a 2-second backoff.

### Q. Why is my Ubuntu Dock filling with "Unknown" gear icons when I paste?

Every short-lived `wl-paste` / `wl-copy` process registers as a transient Wayland client; GNOME Shell surfaces each one as a generic icon. FlashPaste ships `.desktop` files with `NoDisplay=true` for the known short-lived helpers, and the `clipboard-janitor` user service reaps stuck `wl-paste` / `wl-copy` daemons every second. Tier 3 (the persistent daemon) eliminates the root cause: one stable `app_id` instead of N forks.

## Performance

### Q. How accurate are the latency numbers?

Measured end-to-end on Ubuntu 24.04 / kitty 0.32 / tmux 3.4 / mutter 46. The numbers are p50 of 100 pastes. Tail latency (p99) on Tier 1 occasionally hits ~180 ms when the system is under load; Tier 3 stays under 25 ms even at p99.

### Q. How do I measure my own latency?

Set `FLASHPASTE_TRACE=1` (e.g. via `~/.tmux.conf`'s `set-environment`) and run `flashpaste-trace.sh`. You'll get a p50/p90/p99 breakdown per checkpoint over your last 100 pastes.

### Q. Will the daemon use a lot of memory or CPU?

The daemon's resident set is under 8 MB. CPU usage is negligible when idle — one inotify watch and one socket listen. It does not poll.

### Q. Does `FLASHPASTE_QUIET=1` actually save time?

Yes. Logging dispatch trims ~5–15 ms per invocation. Set it once you trust the install and don't need timing telemetry. The JSON trace sink (`FLASHPASTE_TRACE=1`) is also suppressed by `FLASHPASTE_QUIET=1`.

## Integration & ecosystem

### Q. Does FlashPaste have an MCP server?

Yes — `flashpaste-mcp` (experimental, in `rs/flashpaste-mcp/`). It exposes clipboard read/write, screenshot capture (via `flashpaste-shoot`), and paste-into-pane as MCP tools an LLM agent can call. Useful for "take a screenshot of the current state and analyze it" loops.

### Q. Can I use FlashPaste programmatically?

Yes. The contract surface is documented in the [README](../README.md#the-contract-surface-whats-safe-to-depend-on). Stable: `tmux-paste-dispatch.sh`, `flashpaste-dispatch`, `flashpaste-trigger`, `flashpaste-shoot`, `flashpaste-doctor`, `flashpaste-trace.sh`. Experimental: daemon wire protocol, MCP server.

### Q. Does it work with `claude-mem` or other Claude Code plugins?

Yes — FlashPaste is at the *clipboard* layer, plugins are at the *agent* layer. They compose cleanly.

## Comparison & alternatives

### Q. Why not just use `wl-clip-persist`?

`wl-clip-persist` is wlroots-only. It depends on the `wlr-data-control` protocol, which mutter does not implement. On GNOME it fails with `Failed to get clipboard manager`. See [docs/comparison.md](comparison.md) for the full comparison.

### Q. Why not use `cliphist`?

`cliphist` is text-only. FlashPaste's primary use case is *image* paste.

### Q. Why not pipe through OSC 52?

OSC 52 is text-only in current terminals. The OSC 52 image extension is not standardized; no terminal AI agent recognizes it.

## Contributing

### Q. I want to add support for terminal X. Where do I start?

[AGENTS.md](../AGENTS.md) has the contributor rules. The four hard-won facts the code must preserve are at the top. Open an issue first to align on approach.

### Q. The .deb workflow failed. What do I do?

Check the GitHub Actions run logs — usually a transient apt-get failure. `gh run rerun` typically fixes it. Don't push a `v1.X+1` to paper over a missing `v1.X` release.

### Q. How do I run the test suite?

Bash syntax: `bash -n bin/*.sh install.sh bootstrap.sh`. Rust: `cargo test --manifest-path rs/Cargo.toml` (currently no in-tree tests; manual timing telemetry is the test bed). Pre-flight: `make doctor`.
