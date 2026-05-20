---
title: FlashPaste troubleshooting — image paste isn't working
description: Diagnostic guide for FlashPaste on GNOME Wayland. Symptom-to-fix tables, daemon health checks, log file locations, and the canonical decision flowchart for "image paste isn't working in Claude Code / Codex CLI / Aider".
keywords:
  - image paste not working claude code
  - wl-paste returns 0 bytes
  - flashpaste daemon socket missing
  - kitty image paste broken
  - tmux paste recursion
  - ubuntu dock unknown icons
  - ydotool socket path bug
last_updated: 2026-05-19
canonical: https://github.com/NagyVikt/flashpaste/blob/main/docs/troubleshooting.md
---

# Troubleshooting

If something's wrong with FlashPaste, walk the diagnostic flow from top to bottom. Most issues resolve in the first three steps.

## Step 1 — Run the doctor

```bash
flashpaste-doctor       # or: bash ~/.local/share/flashpaste/bin/flashpaste-doctor.sh
```

17 core parallel checks for Wayland, mutter, kitty (installed + IPC socket reachable), tmux (installed + running + inside kitty), wl-clipboard, xclip, ydotool + socket, the screenshots directory, installed flashpaste components, and the overlay daemon/surface/round-trip path. Everything should be green. Anything red is your first lead.

## Step 2 — Check the log files

| Log | What it contains |
|---|---|
| `~/.local/state/clipboard-pipeline.log` | Cross-script event timeline. Every clipboard-touching script writes here. |
| `~/.local/state/tmux-paste.log` | Per-invocation timing checkpoints (T+/Δ format). Latest entries at the bottom. |
| `~/.local/state/flashpaste-trace.jsonl` | Structured JSONL trace sink. Only written when `FLASHPASTE_TRACE=1`. |
| `journalctl --user -u flashpasted` | Tier 3 daemon log (if installed). |
| `journalctl --user -u clipboard-janitor` | Janitor service log. |

Tail-friendly:

```bash
tail -F ~/.local/state/clipboard-pipeline.log    # cross-script
tail -F ~/.local/state/tmux-paste.log            # per-invocation
journalctl --user -u flashpasted -f              # daemon
```

For percentile analysis (only with `FLASHPASTE_TRACE=1`):

```bash
flashpaste-trace.sh                              # p50/p90/p99 per checkpoint
flashpaste-trace.sh --tail                       # live
flashpaste-trace.sh --since 2026-05-19T12:00Z
```

## Step 3 — Symptom-to-fix table

| Symptom | Likely cause | Fix |
|---|---|---|
| Paste returns text when you wanted an image | Clipboard text shadowed the image MIME after copy | Re-screenshot, or use `Ctrl+Alt+V` to force the image path |
| `flashpaste-doctor` flags ydotool socket | Ubuntu 24.04 socket-path bug | Re-run `install.sh`, or drop in the systemd override manually (see [install.md](install.md#the-ydotoold-socket-path-drop-in-mandatory-on-ubuntu-2404)) |
| Dock fills with "Unknown" gear icons during heavy paste | `share/applications/wl-clipboard.desktop` not installed | `make install` again, or upgrade to Tier 3 (eliminates the issue at the root) |
| Tier 3 daemon won't start | Stale socket | `rm "$XDG_RUNTIME_DIR/flashpaste.sock"` then `systemctl --user restart flashpasted` |
| Right-click "Paste" menu does nothing | tmux `mouse off`, or snippet not loaded | `tmux source-file ~/.tmux.conf`; confirm `set -g mouse on` is present |
| `wl-paste -t image/png` returns 0 bytes | mutter went silent for surfaceless clients (wedge) | FlashPaste's shim already handles this — confirm `~/.local/bin/wl-paste` shim is on PATH ahead of `/usr/bin/wl-paste` |
| Ctrl+V pastes the *previous* screenshot | mutter's X11↔Wayland bridge is sticky | Re-copy or wait `WL_PASTE_SHIM_WEDGE_TTL` seconds (default 30) — the wedge cache will refresh |
| Kitty IPC socket not found | `allow_remote_control yes` not set in kitty.conf | Add `allow_remote_control yes` to `~/.config/kitty/kitty.conf` and restart kitty |
| `flashpaste-trigger` always falls back to bash | Daemon socket missing | `ls $XDG_RUNTIME_DIR/flashpaste.sock` — if absent, `systemctl --user start flashpasted` |
| Paste works in one pane but not another | Stale recursion-guard lock | `rm $XDG_RUNTIME_DIR/tmux-paste-dispatch.lock`; the lock auto-clears after 2 s on next paste |
| GNOME Screenshot saves but FlashPaste doesn't pick it up | `flashpaste-screenshot-watcher.path` not enabled | `systemctl --user enable --now flashpaste-screenshot-watcher.path` |
| Latency is ~3 s instead of ~120 ms | Auto-pickup file is stale (>30 s old) or clipboard has text | Re-screenshot to trigger fresh auto-pickup |

## Step 4 — Diagnostic flowchart

```text
"Image paste isn't working"
        │
        ▼
flashpaste-doctor passes? ─── no ──► fix the red probe first
        │ yes
        ▼
Is `~/.local/bin` ahead of `/usr/bin` on PATH? ─── no ──► fix PATH
        │ yes
        ▼
Press PrtScr — does a PNG appear in ~/Pictures/Screenshots/?
        │
        ├── no ──► GNOME screenshot binding broken (not FlashPaste)
        │
        ▼ yes
Within 5 s of PrtScr, does
    journalctl --user -u flashpaste-screenshot-watcher
show "preload OK"?
        │
        ├── no ──► .path unit not enabled; see install.md
        │
        ▼ yes
Right-click in tmux pane → Paste:
    tail -1 ~/.local/state/tmux-paste.log
shows "fast-path exit"?
        │
        ├── no ──► dispatcher not on PATH; check ln -sf in install.sh
        │
        ▼ yes
The byte landed in kitty but Claude Code shows nothing?
        │
        └─► kitty's `map ctrl+v` is intercepting before tmux. Check kitty.conf
            uses `flashpaste-trigger` (snippet) instead of native paste.
```

## Daemon health (Tier 3 only)

```bash
systemctl --user status flashpasted              # Active (running)
ss -lUn | grep flashpaste.sock                   # Socket present
journalctl --user -u flashpasted -n 50           # Last 50 log lines
```

A healthy daemon reports each subsystem on startup:

```text
flashpasted starting (socket=/run/user/1000/flashpaste.sock)
  ✓ Wayland selection owner ready
  ✓ X11 selection owner ready
  ✓ inotify watching ~/Pictures/Screenshots/
  ✓ unix socket listening
```

If a subsystem fails the daemon keeps running with the other subsystems active — check the log for `subsystem disabled`.

## When to ask for help

If you've walked all four steps and the issue persists, open an issue at [github.com/NagyVikt/flashpaste/issues](https://github.com/NagyVikt/flashpaste/issues) with:

1. The output of `flashpaste-doctor`
2. The last 50 lines of `~/.local/state/clipboard-pipeline.log`
3. The last 50 lines of `~/.local/state/tmux-paste.log`
4. Your distro version (`lsb_release -a` or `cat /etc/os-release`)
5. Your kitty version (`kitty --version`) and tmux version (`tmux -V`)
