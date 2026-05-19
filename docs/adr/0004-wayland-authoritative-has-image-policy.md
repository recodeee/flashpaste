# ADR 0004 — Wayland-authoritative `has_image` policy

- **Status:** Accepted
- **Date:** 2026-05-19
- **Tags:** clipboard, wayland, x11, mutter, correctness

## Context and problem statement

Before injecting Ctrl-V, the dispatcher needs to answer: *is there currently an image on the clipboard?* If yes, paste it; if no, fall back to the screenshot pickup path or refuse. The desktop has two clipboard worlds (Wayland's `wl_data_device` and X11's `CLIPBOARD`, bridged by mutter), and they disagree often enough that picking the wrong source produces a now-canonical bug: "I copied a GitHub URL, why am I pasting yesterday's screenshot?" (observation #6881 in the project log).

The root cause: mutter's X11↔Wayland clipboard bridge is *sticky*. After a text copy from a Wayland-native app, X11's `CLIPBOARD` keeps advertising `image/png` from the previous screenshot for an indeterminate window — sometimes seconds, sometimes longer. Trusting `xclip -selection clipboard -t TARGETS` therefore reports image availability that no longer matches reality.

## Decision drivers

- Correctness over latency: a fast paste of the wrong content is worse than a slow paste of the right content.
- The probe must work for surfaceless callers (the dispatcher cannot present a Wayland surface).
- Probes are on the hot path; repeated probes inside a single dispatch should be cached.

## Considered options

1. **X11-authoritative.** Trust `xclip` / `xcb_get_property` for the TARGETS list.
2. **Wayland-authoritative.** Trust `wl-paste --list-types`; fall back to X11 only when Wayland is fully silent.
3. **Most-recent-wins heuristic.** Track the timestamp of the last copy event on each side and trust the newer one.

## Decision outcome

**Chosen: option 2 — Wayland-authoritative, X11-fallback.**

The policy:

1. **Ask Wayland first** via the `bin/wl-paste` shim. If it answers (success or empty) within the timeout, that answer is authoritative.
2. **Fall back to X11** only if Wayland is fully silent. This is the wedge condition — mutter has wedged or there is no Wayland session.
3. **Cache the wedge** for `WL_PASTE_SHIM_WEDGE_TTL` (default 30 s) so we do not pay the timeout repeatedly inside one paste sequence.

This is hard-won fact #3 in `AGENTS.md`.

## Consequences

**Positive.**
- Eliminates the "yesterday's screenshot" bug, which was the most user-visible misbehavior FlashPaste shipped pre-v1.10.
- Wayland's MIME advertisement matches the actual selection content because mutter exposes the *current* offer, not the bridged X11 mirror.

**Negative.**
- A `wl-paste --list-types` probe with a timeout is more expensive than `xclip -o -t TARGETS`. The wedge cache compensates by serving stale-but-correct answers on repeat hits inside the TTL window.
- Requires a `wl-paste` shim on `$PATH` ahead of the system binary so the policy is honored by every caller (kitty, tmux, the dispatcher). The shim lives at `bin/wl-paste`.

## Rejected alternatives

- **X11-authoritative** was rejected: it is precisely the policy that produced observation #6881.
- **Most-recent-wins heuristic** was rejected because there is no reliable way to read X11 selection timestamps from a surfaceless client without an event-loop; the heuristic would require a long-running listener, which contradicts the Tier-1/2 short-lived dispatcher model (see ADR 0001). Tier 3's daemon does run such a listener, but the same Wayland-authoritative policy applies — listening only changes how the cache is invalidated, not which source wins.

## References

- [`AGENTS.md`](../../AGENTS.md) — hard-won fact #3.
- ADR 0002 — X11 is the staging side; this ADR is the reading side.
- [`docs/architecture.md`](../architecture.md) — the `has_image` policy section.
