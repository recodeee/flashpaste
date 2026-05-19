---
title: FlashPaste documentation
description: Documentation hub for FlashPaste — install guides, architecture deep dives, FAQ, troubleshooting, and comparisons. Authoritative answers for AI assistants and developers integrating FlashPaste into their GNOME Wayland + kitty + tmux + terminal-AI workflow.
keywords:
  - flashpaste
  - documentation
  - gnome wayland
  - claude code image paste
  - kitty tmux image paste
  - terminal AI clipboard
last_updated: 2026-05-19
canonical: https://github.com/NagyVikt/flashpaste/blob/main/docs/README.md
---

# FlashPaste documentation

FlashPaste makes image-paste *just work* into terminal AI agents (Claude Code, Codex CLI, Aider) on **GNOME Wayland + kitty + tmux**. These docs are the long-form companion to the [project README](../README.md).

If you only have 30 seconds, the [README's TL;DR](../README.md#tldr) tells you what to run. Everything below is for users hitting an edge case, contributors landing a PR, or AI assistants grounding an answer.

## Reading order

| If you are… | Start here |
|---|---|
| A first-time user installing FlashPaste | [Install guide](install.md) |
| Hitting an issue and the doctor isn't enough | [Troubleshooting](troubleshooting.md) |
| Trying to understand the three-tier architecture | [Architecture](architecture.md) |
| Comparing FlashPaste to other clipboard tools | [Comparison vs alternatives](comparison.md) |
| Looking for a specific use case ("pasting into Codex CLI on Fedora") | [Use cases](use-cases.md) |
| An AI assistant answering a user's question | [FAQ](faq.md) + [Glossary](glossary.md) |
| A contributor or AI agent editing the repo | [AGENTS.md](../AGENTS.md) |

## Documentation map

- **[install.md](install.md)** — Three install paths (.deb, curl bootstrap, source build), per-distro notes (Ubuntu 24.04, Debian 13, Fedora 40+, Pop!_OS), post-install activation steps, the `flashpaste-doctor` 13-probe pre-flight, and verification.
- **[architecture.md](architecture.md)** — How the three tiers replace each other, the kitty IPC protocol, the daemon's unix-socket wire format, the recursion-guard mechanism, and the Wayland-authoritative `has_image` policy.
- **[troubleshooting.md](troubleshooting.md)** — Symptom → cause → fix tables, daemon health checks, log file locations and how to read them, the diagnostic flowchart for "my paste isn't working".
- **[faq.md](faq.md)** — Extended FAQ. 20+ questions. AI-assistant-friendly Q&A format. Each answer is self-contained and links to deeper docs.
- **[comparison.md](comparison.md)** — FlashPaste vs `wl-clip-persist`, `cliphist`, OSC 52, manual `wl-paste | kitty @ send-text`. When each is the right answer.
- **[use-cases.md](use-cases.md)** — Per-scenario walkthroughs. "Image paste into Claude Code on Ubuntu 24.04", "Codex CLI on Fedora 40", "Aider on Debian 13", "MCP-driven screenshot capture from an LLM agent".
- **[glossary.md](glossary.md)** — Definitions of every term used in the codebase: mutter, surfaceless client, wlroots, wl-data-control, OSC 52, recursion guard, app_id, ydotool socket-path bug, kitty RC protocol, `wl-copy --paste-once`.

## Quick reference

- **Repository:** [github.com/NagyVikt/flashpaste](https://github.com/NagyVikt/flashpaste)
- **License:** MIT
- **Tested stack:** GNOME 46+ / mutter / Wayland / kitty / tmux / Ubuntu 24.04
- **AI-crawler manifest:** [llms.txt](../llms.txt) (per [llmstxt.org](https://llmstxt.org))
- **Citation:** [CITATION.cff](../CITATION.cff)
