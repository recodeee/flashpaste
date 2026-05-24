# flashpaste — Roadmap

A phased plan from where we are (v1.12, 1775 lines of bash) to where we want to be (battle-tested, sub-50ms, distro-packaged).

## Where we are

- **Working:** kitty + tmux + Claude Code/Codex image paste on GNOME 46 Wayland in ~120ms via the fast path.
- **Bash everywhere:** all 9 scripts (1775 lines) are bash. No native binaries yet.
- **One-line install + 13-check doctor + screenshot preload daemon** all functional.
- **Known limitations / code review findings:**
  - `tmux-paste-dispatch.sh` is **634 lines** — too long, many overlapping code paths (early-preload, fast-path, prestage, image-branch text-fallbacks).
  - **9 hardcoded `/home/deadpool/...` paths** in scripts. Installer symlinks work today but the source files won't run from anywhere else without edits.
  - **Clipboard probe duplication** between `paste_image.sh` and `tmux-paste-dispatch.sh` (same wl-paste/xclip TARGETS probes done in both).
  - **No shellcheck CI**, no test suite.
  - **systemd `.path` watcher** has 100–300ms wakeup lag — the biggest remaining latency before user input.
  - **wedge cache TTL is time-based (30s)**, not event-based — when mutter recovers we wait up to 30s before trying real `wl-paste` again.
  - **No uninstall script.**
  - **No config file** — every knob is an env var, scattered across files.

## Phase 1 — Quick wins (days, no Rust yet)

Target: tighten the existing bash, reduce surprises, make non-Ubuntu installs friction-free.

- [ ] **Replace hardcoded `/home/deadpool/...` with `$HOME` and a sourced `flashpaste.env`.** Each script reads `${FLASHPASTE_BIN:-$HOME/.local/bin}` instead of hardcoding. Lets users put the install anywhere.
- [ ] **Extract a `lib/clip-probe.sh`** that does the wl-paste/xclip MIME detection once, used by both `paste_image.sh` and `tmux-paste-dispatch.sh`. Cuts ~200 lines of duplication and a class of "where did `_has_image` come from" confusion.
- [ ] **Uninstall script** (`uninstall.sh`) — disables systemd units, removes symlinks, restores any `.flashpaste-bak` backups.
- [ ] **Config file at `~/.config/flashpaste/config.toml`** — central place for the knobs that today are env vars (`FLASHPASTE_QUIET`, paths, timeouts).
- [ ] **`flashpaste update`** subcommand that does `git -C ~/.local/share/flashpaste pull && bash install.sh`. Matches the existing one-line install ergonomics.
- [ ] **Add shellcheck + bats-core to a GitHub Actions CI** so regressions don't ship.
- [ ] **Doctor: add machine-readable `--json` output** so other tools can consume the diagnosis.
- [ ] **Wedge-cache eviction on `SIGUSR1`** — if user runs `flashpaste reset` (or after mutter recovers), drop the cache immediately instead of waiting 30s.
- [x] **Handle copy-image-from-browser** explicitly (Firefox / Chrome put image bytes on the clipboard without writing a file). Today the auto-pickup only triggers on screenshot *files*; browser images work via the regular probe path but the FAST PATH is bypassed. *Shipped via `bin/flashpaste-capture-clip`: the image branch of `bin/paste_image.sh` reads the bytes from the kitty-subprocess context, writes them to `~/Pictures/Screenshots/flashpaste-clip-latest.png`, and the daemon's inotify watcher stages them.*

## Phase 2 — Performance (Rust where it pays)

Target: cut the remaining latency to sub-50ms end-to-end. Rust only where it materially helps.

- [ ] **`flashpaste-watcherd`** — Rust binary using `inotify` directly. Replaces the systemd `.path` unit (which has 100–300ms wakeup). Native inotify fires within <5ms of `write()`. Single ~150-line file, no external crates beyond `inotify`. **Highest impact change.**
- [ ] **`flashpaste-dispatch`** (Rust) — single binary replacing `tmux-paste-dispatch.sh`. Saves ~30–50ms of bash fork/exec overhead. Uses `wl-clipboard-rs` crate so the wl-paste/wl-copy calls happen in-process without forking subprocesses.
- [ ] **Persistent daemon model** — `flashpasted` stays resident, listens on a Unix socket. Each paste click sends a 1-byte trigger; daemon does the work in <10ms. Total paste latency would drop to **~20ms** (limited by tmux/kitty IPC, not by us).
- [ ] **Drop `setsid xclip -i FILE` in favor of in-process selection ownership** via `wl-clipboard-rs`. Removes one fork + one Wayland connection setup per paste.
- [ ] **Optional: kitty native protocol direct send** instead of `kitty @ send-text` IPC. Saves the kitty roundtrip (~20ms).

Performance budget after Phase 2:

| Step | Today (bash) | After Phase 2 (Rust daemon) |
|---|---|---|
| Watcher latency (PrtScr → xclip loaded) | 100–300ms | <5ms |
| Dispatch fork + probe | ~80ms | <5ms (in-daemon) |
| xclip selection claim | ~50ms (setsid + sleep) | <2ms (in-process) |
| tmux unbind + send-text + rebind | ~50ms | ~50ms (kitty/tmux bound) |
| **Total perceived latency** | **~150ms + user time** | **~60ms + user time** |

## Phase 3 — Features

Target: solve adjacent paper cuts users hit once basic paste is solid.

- [ ] **Clipboard history with image support** — `cliphist` doesn't store image blobs. Build a small SQLite-backed history that keeps the last N images and their thumbnails. Recall via `flashpaste pick`.
- [ ] **Auto-resize/compress large screenshots** — Claude's image attachments have size limits. If PNG >5MB, transparently re-encode to a configurable max-dim (e.g. 2400px) before sending to xclip. Saves bandwidth, avoids attachment rejections.
- [ ] **Image preview in tmux status** — when a fresh screenshot is preloaded, briefly show a tiny indicator in tmux's status-right (`📎 1.2MB ready`). Catches the user's eye so they know the paste is hot.
- [ ] **OCR mode** — `flashpaste ocr` extracts text from the latest screenshot via `tesseract` and pastes both image AND text. Useful when Claude needs to discuss UI labels exactly.
- [ ] **Drag-and-drop file → auto-attach** — listen for kitty's file-drop OSC sequence and run the auto-attach flow on the dropped file path.
- [x] **Support for Aider, Codex CLI, other TUI agents** — the daemon now has an agent detector. Claude Code / Codex stay on the raw Ctrl-V image-paste path; Aider uses its documented `/add <image-path>` chat command against the staged file. Future agents can add another delivery branch without disturbing the Claude/Codex transport.
- [ ] **Multi-monitor screenshot dirs** — some setups save to per-monitor dirs; allow `screenshots_dirs = [..., ...]` in the config.

## Phase 4 — Distribution & community

Target: make `flashpaste` installable by people who've never seen the README.

- [ ] **AUR package** (`flashpaste-git`) — natural fit, Arch users are an early adopter pool.
- [ ] **Homebrew tap on Linux** — `brew install nagyvikt/flashpaste/flashpaste`.
- [ ] **.deb + .rpm via fpm** — Ubuntu/Fedora-friendly downloads on the GitHub Releases page.
- [ ] **Documentation site** (`flashpaste.dev` via mkdocs-material) — searchable docs, troubleshooting matrix per compositor.
- [ ] **3-minute walkthrough video** — PrtScr → paste in Claude Code, with the dock-flash going away once the wedge cache primes. Embed in README.
- [ ] **A blog post** about the rabbit hole — mutter's surfaceless-client clipboard refusal, `wl-copy --paste-once` drainage, tmux `bind -n C-v` recursion, ydotool 0.1.8 socket-path bug. Each one is a 30-min puzzle solo, ~6 hours collectively.

## Cross-cutting

- [ ] **Telemetry-free.** flashpaste should never phone home. Stays opt-in even for crash reports.
- [ ] **Backwards compatibility.** Bash scripts stay during Phase 2 — Rust binaries are opt-in via config. Users who don't want a daemon get exactly today's behavior.
- [ ] **Test matrix.** Phase 1 ships shellcheck + bats CI; Phase 2 adds an integration test that spawns a real kitty + tmux + dummy TUI and asserts `wl-paste -t image/png` returns N bytes after a synthesized PrtScr.

## Decision: Phase 2 daemon — opt-in or default?

**Recommendation: opt-in via config.** The bash fast-path is already 120ms — plenty for almost all users. The daemon's wins matter for power users doing dozens of pastes per minute (active coding session with screenshots). Keep bash as the default install; users who want sub-50ms flip `daemon = true` in `~/.config/flashpaste/config.toml` and `systemctl --user enable flashpasted`.

## What we are NOT going to do

(Things considered and rejected — documenting so they don't get re-proposed.)

- ❌ **Replace `wl-clipboard` system-wide** — too invasive; flashpaste is supposed to fix paste, not replace your clipboard stack.
- ❌ **Patch mutter** — out of scope; we work around it. If GNOME ever fixes surfaceless-client clipboard, flashpaste degrades gracefully (the shim's real-wl-paste path will start succeeding).
- ❌ **Snap package** — `snap install` for a dotfile-injecting tool is more friction than `curl | bash`.
- ❌ **GUI tray app** — out of scope. CLI + systemd is sufficient.
- ❌ **Built-in paste history shortcut bound to `Ctrl+Shift+H`** — let users wire their own keybind; we provide `flashpaste pick`.

## Versioning

Following the dispatch script's `v1.x` numbering:

- **v1.x** — bash-only era. Bug fixes, code reorganization, Phase 1 deliverables.
- **v2.0** — first Rust component (`flashpaste-watcherd`) ships. Still bash dispatch by default.
- **v3.0** — daemon mode (`flashpasted`) becomes opt-in default for new installs.
- **v4.0** — Phase 3 features land. Considered "feature-complete".
