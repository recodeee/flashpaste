# ADR 0001 — Three progressive tiers instead of a single daemon

- **Status:** Accepted
- **Date:** 2026-05-19
- **Deciders:** maintainers
- **Tags:** architecture, latency, fallback

## Context and problem statement

FlashPaste's job is to inject an image clipboard paste into a terminal AI in under one human reaction time (~150 ms). The implementation must run on Ubuntu 24.04 GNOME/Wayland with XWayland and tmux + kitty, where every component (mutter's clipboard bridge, kitty's IPC, tmux's key binding layer) has its own quirks. A single implementation strategy forces a tradeoff: a bash script is robust and trivial to debug but caps at ~127 ms; a long-running daemon is fast but adds a service-management surface and a failure mode where "FlashPaste is not running" silently degrades the user. We needed a way to ship both.

## Decision drivers

- The 90% case must work with zero daemons, zero Rust toolchain, zero systemd.
- Power users should be able to opt into sub-15 ms paste without rewriting their setup.
- An upgrade must never make a working install worse: every fast path falls back to the canonical bash path.

## Considered options

1. **Daemon-only.** Ship `flashpasted` as the only implementation.
2. **One-shot-only.** Ship the Rust dispatcher as the only fast path.
3. **Three progressive tiers.** Bash dispatcher (Tier 1), Rust one-shot (Tier 2), daemon + trigger (Tier 3), with each higher tier falling back to the next.

## Decision outcome

**Chosen: option 3 — three progressive tiers.**

Tier 1 (`bin/tmux-paste-dispatch.sh`) is the canonical implementation. It is always installed and is what every higher tier falls back to. Tier 2 (`flashpaste-dispatch`) replaces only the dispatcher binary at the tmux binding level; it execs Tier 1 on failure. Tier 3 (`flashpasted` + `flashpaste-trigger`) moves staging work before the keypress; the trigger execs Tier 1 if the socket is absent or the daemon answers `fallback=true`.

The tiers are wire-compatible at the tmux binding: a user can flip between them by changing one `bind -n C-v` line.

## Consequences

**Positive.**
- Tier 1 acts as a permanent safety net — every higher tier is a strict latency optimization with the same observable behavior.
- A failed `flashpasted` deployment degrades gracefully to Tier 1; the user notices a latency change, not a broken paste.
- New contributors can read the bash script first to understand the protocol, then look at the Rust to understand the optimization.

**Negative.**
- Triple maintenance burden: behavior fixes have to land in three places. Mitigated by the regression-test discipline in `AGENTS.md` (every fix that affects a hard-won fact needs a manual test plan in the commit body).
- Three latency tiers in the docs require explaining the tradeoffs (`docs/architecture.md` does this).

## Rejected alternatives

- **Daemon-only** was rejected because clipboard-owner daemons are a known source of GNOME Dock phantom icons, systemd unit drift, and "why is my paste silently broken" reports. The bash dispatcher being mandatory turns these from outages into latency regressions.
- **One-shot-only** was rejected because ~40 ms is the floor for a fresh process that has to claim the X11 selection; sub-15 ms is only reachable with pre-staging, which requires a long-lived owner.

## References

- [`docs/architecture.md`](../architecture.md) — full latency budget per tier.
- [`AGENTS.md`](../../AGENTS.md) — release discipline that keeps the three implementations in lockstep.
