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
