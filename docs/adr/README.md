---
title: FlashPaste Architecture Decision Records
description: Index of MADR-format Architecture Decision Records documenting the why behind FlashPaste's design choices.
last_updated: 2026-05-19
---

# Architecture Decision Records

An **Architecture Decision Record (ADR)** captures a single architecturally significant decision: the context that forced it, the option chosen, the alternatives rejected, and the consequences accepted. The goal is that a future contributor (human or AI) can read the record and understand *why* the code looks the way it does without having to re-derive the reasoning from logs and issues.

FlashPaste uses the lightweight **MADR 3.0.0** template: [adr.github.io/madr](https://adr.github.io/madr/). Each record has: status, context and problem statement, decision drivers, considered options, decision outcome, consequences.

The four hard-won facts in [`AGENTS.md`](../../AGENTS.md) are essentially mini-ADRs; the records here expand them with the full alternatives matrix. The architectural overview lives in [`docs/architecture.md`](../architecture.md); comparisons with other tools live in [`docs/comparison.md`](../comparison.md). ADRs cover the *why*; those documents cover the *what*.

## Index

| # | Title | Status | Date |
|---:|---|---|---|
| [0001](0001-three-progressive-tiers.md) | Three progressive tiers instead of a single daemon | Accepted | 2026-05-19 |
| [0002](0002-xclip-not-wl-clipboard-on-staging.md) | Stage screenshots into X11 via xclip, not Wayland via wl-clipboard | Accepted | 2026-05-19 |
| [0003](0003-kitty-send-text-not-tmux-send-keys.md) | Use `kitty @ send-text` for image-paste, not `tmux send-keys` | Accepted | 2026-05-19 |
| [0004](0004-wayland-authoritative-has-image-policy.md) | Wayland-authoritative `has_image` policy | Accepted | 2026-05-19 |
| [0005](0005-tmux-unbind-rebind-not-pass-through.md) | Unbind + detached-rebind C-v around send-text, not pass-through | Accepted | 2026-05-19 |

## Writing a new ADR

1. Copy the structure from any existing record.
2. Number it sequentially. Never renumber.
3. Status starts as `Proposed`; once merged on `main`, flip to `Accepted`. Use `Superseded by ADR-####` when a later record replaces it; never delete a record.
4. Keep it under ~80 lines. ADRs are *decisions*, not tutorials.
5. Add a row to the table above.
