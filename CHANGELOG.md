# Changelog

All notable changes to FlashPaste are listed here. The format follows [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Release-tag policy: every `vX.Y` commit on `main` must be tagged and have a matching GitHub release before the turn ends — see [`AGENTS.md`](AGENTS.md) for the full rule.

## [Unreleased]

### Added

- Unified `flashpaste` CLI wrapping the six binaries (`dispatch`, `trigger`, `shoot`, `doctor`, `trace`, `mcp`)
- `flashpaste-shoot --ocr` and `--ocr-only` flags for portal screenshot capture
- `flashpaste-shoot --annotate` for screenshot annotation
- `flashpasted` auto-compresses staged screenshots on the inotify path
- `flashpaste-common` image auto-compress module
- Doctor + docs surface OCR / annotate / auto-compress to users
- `docs/` tree (architecture, install, troubleshooting, FAQ, comparison, use-cases, glossary)
- `llms.txt` (AI-crawler manifest per llmstxt.org)
- `CITATION.cff`
- Distribution packaging: AUR PKGBUILDs (stable + git), Homebrew tap formula, Nix flake
- Project-health files: `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, this `CHANGELOG.md`, issue + PR templates, `FUNDING.yml`
- CI: shellcheck + markdownlint + link-check workflow
- Architecture Decision Records under `docs/adr/`
- Reproducible benchmark suite (`bin/flashpaste-bench.sh`, `make bench`)
- `assets/og-image.svg` for social sharing
- `assets/hero-flow-light.svg` light-mode variant
- README badges, animated SVG hero, animated tier-comparison chart, Mermaid sequence diagram, AI-assistant TL;DR block, extended FAQ, alternatives comparison

## [1.25] - 2026-05-19

### Added

- `rs/flashpasted/src/ipc.rs` text-vs-image intent decision in `handle_paste`. The single `latest_selection` slot still holds at most one variant (mirrors how real clipboards work), but the dispatcher now consults live X11 TARGETS too: if the daemon has a fresh staged image AND the X11 CLIPBOARD has been taken over by an external app advertising text-only targets (browser, terminal selection, IDE copy, …), the user's text is scraped, staged, and dispatched as text instead of forcing the image through. The staged image stays in memory so a subsequent paste with no text-overlay still serves it.
- `rs/flashpasted/src/paste.rs` `dispatch_text_paste`: tmux `load-buffer` + `paste-buffer -t <pane>` text path. No clipboard claim, no kitty IPC, no unbind/rebind dance — pure tmux pty injection so the text lands in any pane regardless of which terminal hosts the tmux client. Replaces the "punt to bash" path for the text case.
- `rs/flashpasted/src/tmux.rs` `send_ctrl_v_to_pane(pane)`: `tmux send-keys -t pane -l \x16` injects the literal Ctrl-V byte directly into the named pane's pty, bypassing kitty's "active window only" filter. Fixes the user-reported "I could paste image only into one Claude Code chat — the rest doesn't get my img."

### Fixed

- `bin/wl-paste` shim now refuses to lie. `xclip -selection clipboard -t image/png -o` on a text-only clipboard *silently returns the text bytes* instead of failing (xclip falls back to whatever's in the selection when the requested MIME isn't advertised). The shim was forwarding that text-as-image lie up to Claude Code, which would report "no image found" while pasting raw text into the chat. New behaviour: when a MIME-typed target is requested, the shim probes TARGETS first; if the requested mime isn't on offer, exit 1 with no stdout — matching what a healthy `wl-paste -t image/png` does on a missing MIME.
- `rs/flashpasted/src/ipc.rs` removed the `clipboard_holds_user_text` punt-to-bash short-circuit at the top of `handle_paste`. It was firing on every tmux highlight (which auto-copies via `@clip` to xclip), forcing Claude pastes to deliver highlighted log junk instead of the user's screenshot. The new intent decision (above) handles the same case more precisely — it only honours external text when X11 is actually owned by another app, not when xclip is briefly text-typed by our own pipe.

### Changed

- `bin/clipboard-set.sh`, `bin/flashpaste-logs.sh`, `bin/flashpaste-screenshot-preload.sh`, `rs/flashpasted/src/{inotify_watch,wayland}.rs`: in-flight tweaks bundled with the release. Notable: clipboard-set.sh gates `wl-copy` behind `FLASHPASTE_USE_WL_COPY=1` and reaps stale `wl-broken` flags; flashpaste-logs.sh adds `--clip` / `--kitty` poller streams with the wl-paste call gated behind `--clip-wayland` to keep the dock quiet on Mutter.

## [1.24] - 2026-05-19

### Removed

- `wait_for_pane_idle` in `rs/flashpasted/src/tmux.rs` (and the `claude_is_busy` / `line_has_token_counter` helpers + the `FLASHPASTE_PANE_IDLE_TIMEOUT_MS` env knob in `paste.rs`). The v1.23 idea was to detect Claude generating via the live `↓ N tokens` indicator and hold the dispatch until idle; empirically the detector matched any scrollback line containing `<digit> tokens` (chat history, release notes, "Saved 200 tokens", etc.), so it timed out on every press into a Claude pane and added the full timeout (5 s default after the v1.23 30 s → 5 s tweak) as pure latency. Confirmed in journalctl: `ms_idle_wait=5097`, `ms_idle_wait=5145` back-to-back on `pane=%41`. Dispatches now fire immediately; if the TUI drops the byte the user retries, which is far cheaper than 5 s of guaranteed hang.

### Kept

- The `paste_in_flight` + `pending_paste` dedup in `state.rs` / `ipc.rs` stays. With the wait gone its window shrinks from "up to 30 s" to "~10–20 ms" (just the dispatch itself) but it's still useful for absorbing a rapid double-click on the right-click → Paste menu so Claude sees one `\026` instead of two.

### Changed

- The in-flight dedup is now **pane-aware**. `state.rs` adds a `pending_pane: Mutex<Option<String>>` that records the most recent absorbed pane id; the replay dispatch reads it and targets the saved pane instead of always replaying to the pane the initial dispatch was running on. Watcher had caught the cross-pane bug as "absorbed-press pane=%38 → replay pane=%41 (wrong pane)."
- `ipc.rs` demotes `Broken pipe (os error 32)` on the IPC accept path from `WARN` to `DEBUG`. That's the trigger's 150 ms read timeout closing the socket before we finish writing the queued-paste reply — expected behaviour, not a bug, but it was polluting the WARN stream.

## [1.23] - 2026-05-19

### Added

- `bin/flashpaste-logs` — unified live viewer across the three streams the pipeline writes to (daemon journal, trigger log, clipboard-pipeline log). Colorized, prefixed, grep / since / -n / no-follow flags. `install.sh` symlinks it without the `.sh` suffix to match the muscle memory of `flashpaste-trigger` / `flashpaste-doctor`.

### Changed

- `rs/flashpasted/main.rs` bounds the tokio runtime drop with `shutdown_timeout(500ms)`. Without it, the blocking selection-owner threads kept the runtime alive forever, leaving systemd in `deactivating (stop-sigterm)` until the 90 s `TimeoutStopSec` SIGKILL — during which the socket file existed but the listener was already torn down, so `flashpaste-trigger` got ECONNREFUSED and the user saw paste as "broken after every restart."
- `rs/flashpasted` paste pipeline now cancels copy-mode before sending `\026` (a wheel-scrolled pane silently swallowed the byte) and waits up to 30 s for the Claude Code TUI to finish generating (detected by the live `↓ N tokens` indicator) so pastes during generation no longer drop on the floor.
- `rs/flashpasted/ipc.rs` adds an in-flight dispatch guard. While one dispatch is waiting for Claude to become idle, additional paste presses are deduped instead of queueing — previously 4 queued presses fired four `\026` bytes back-to-back the instant Claude unblocked.
- `rs/flashpasted` kitty dispatch matches `state:active` instead of `state:focused` — survives the focus steal from screenshot tools, right-click menu rendering, and other transient focus changes.
- `rs/flashpasted` latches `WAYLAND_WEDGED` once the compositor proves it speaks no `ext-data-control` / `wlr-data-control` (Mutter on GNOME 46): subsequent re-asserts skip the doomed `copy_multi` task entirely instead of spawn-blocking on every paste.
- `rs/flashpasted` staged-image TTL bumped 2 min → 30 min so the AFK-then-paste case (screenshot, switch away, come back) doesn't silently demote Tier 3 → bash.
- `rs/flashpasted` Ctrl-V rebind now matches the documented `flashpaste-trigger || tmux-paste-dispatch.sh` fallback. Previously the daemon rebound to bash-only after the first paste, silently demoting the rest of the tmux session to Tier 1.
- `bin/clipboard-set.sh` gates the `wl-copy` path behind `FLASHPASTE_USE_WL_COPY=1`. On Mutter the wl-copy fork costs ~4–5 extra execs per paste AND surfaces phantom "Unknown" dock icons, all without acting as a durable selection owner (no data-control protocol available). xclip remains the durable owner.

### Fixed

- shellcheck SC2163 in `bin/clipboard-set.sh:40` and `bin/get-clipboard-text.sh:50` (`export "$kv"` → `export "${kv?}"`).
- shellcheck SC2209 in `bin/flashpaste-trace.sh:162` (`AWK_BIN=awk` → `AWK_BIN='awk'`).
- markdownlint: bulk MD022 / MD031 / MD032 blank-line fixes via `markdownlint-cli2 --fix` across `AGENTS.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `docs/adr/*`, `docs/*`. MD040 fence-language tags added to 12 plain-text fences. `docs/glossary.md` entries promoted h3 → h2 (flat list, no intermediate h2). README "TL;DR for AI assistants" blockquote heading converted to bold (was h3 skipping h2). `.markdownlint.json` sets `MD025.front_matter_title=""` so YAML frontmatter `title:` no longer clashes with the body h1, and disables `MD060` (table-pipe spacing) which the CI action's bundled markdownlint version does not enforce.

### Reverted

- `examples/tmux.conf.snippet`: v1.22 dropped the `-O` flag from the right-click menu binding on the theory that `-O` froze the TUI pane until Escape. That repro was on tmux <3.4 and does not hold on 3.6a; removing `-O` instead caused the menu to auto-dismiss the instant the user moved the mouse toward an entry. v1.23 restores `-O` so the menu stays open until item-click, click-outside, or Escape.

## [1.22] - 2026-05-19

### Fixed

- `examples/tmux.conf.snippet`: dropped the `-O` flag from the right-click menu binding so the menu auto-dismisses on click-outside / mouse-release-outside. With `-O` the menu held the pane in modal-grab until Escape, which read as the pane being "frozen" after a right-click — most visible in TUIs that grab keystrokes (Claude Code chat input). _Reverted in v1.23 — the freeze repro was on tmux <3.4 and the removal made the menu unusable on 3.6a._

## [1.19] - 2026-05-19

### Fixed

- Killed the "wl-clipboard" dock flicker on copy (replaces the v1.13 `NoDisplay` workaround with the root-cause fix in the daemon path)

## [1.18] - 2026-05-19

### Added

- `flashpaste-mcp` server exposing clipboard / screenshot / paste-into-pane tools to LLM agents over MCP stdio
- `flashpaste` agent skill for Claude Code
- `AGENTS.md` release-policy enforcement
- `AGENTS-release-check.sh` audit script

## [1.17] - 2026-05-19

### Changed

- Tier 3 path enabled by default in the example snippets — Ctrl+V now uses `flashpaste-trigger` with automatic fallback to `tmux-paste-dispatch.sh` when the daemon socket is absent

## [1.16] - 2026-05-19

### Added

- Rust daemon (`flashpasted`) — long-lived clipboard owner with inotify-driven screenshot pre-stage
- Sub-15 ms Tier 3 paste path via `flashpaste-trigger` (1-byte unix-socket ping to the daemon)
- Trigger falls back to the bash dispatcher when the daemon is absent

## [1.15] - 2026-05-19

### Added

- Debian packaging — `make deb` produces `dist/flashpaste_*_all.deb`
- GitHub Actions release workflow (`.github/workflows/release.yml`) auto-builds the `.deb` and publishes a GitHub release on every `v*` tag push

## [1.14] - earlier

### Fixed

- Aggressively kill phantom dock icons (refines v1.13)

> The v1.10–v1.14 tags predate the `.github/workflows/release.yml` workflow. Per [AGENTS.md](AGENTS.md), we do not retroactively tag them by default — their build environment may not reproduce.

## [1.13] - earlier

### Added

- `NoDisplay=true` `.desktop` files for `wl-paste` / `wl-copy` to suppress Ubuntu Dock phantom-icon flashes

## [1.12] - earlier

### Added

- Kitty `Ctrl+V` auto-routes between text and image paste

## [1.11] - earlier

### Added

- Parallel `flashpaste-doctor` probes (13 checks)
- Upstream credits surfaced in README

## [1.10] - earlier

### Added

- One-line `bootstrap.sh` installer
- Optional structured logging
- Screenshot watcher (`flashpaste-screenshot-watcher.path` + `.service`)

## [1.0] - initial

Initial commit: sub-120 ms bash hot path for image-paste into GNOME Wayland TUIs.

[Unreleased]: https://github.com/NagyVikt/flashpaste/compare/v1.19...HEAD
[1.19]: https://github.com/NagyVikt/flashpaste/releases/tag/v1.19
[1.18]: https://github.com/NagyVikt/flashpaste/releases/tag/v1.18
[1.17]: https://github.com/NagyVikt/flashpaste/releases/tag/v1.17
[1.16]: https://github.com/NagyVikt/flashpaste/releases/tag/v1.16
[1.15]: https://github.com/NagyVikt/flashpaste/releases/tag/v1.15
