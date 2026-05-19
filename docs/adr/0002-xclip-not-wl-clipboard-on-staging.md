# ADR 0002 — Stage screenshots into X11 via xclip, not Wayland via wl-clipboard

- **Status:** Accepted
- **Date:** 2026-05-19
- **Tags:** clipboard, wayland, x11, staging

## Context and problem statement

To paste a screenshot, FlashPaste must own a clipboard selection that advertises `image/png` at the moment the receiving app reads it. On a GNOME/Wayland desktop with XWayland present, there are two clipboard worlds: Wayland's `wl_data_device` (owned by mutter) and X11's `CLIPBOARD` selection (bridged into the same namespace by mutter). The dispatcher has to pick one to *stage* into. The wrong choice produces silent failures: either nothing pastes, or the byte arrives without an image MIME advertised, or the selection is dropped after the first receive.

## Decision drivers

- The staged selection must survive long enough for the receiving terminal AI to read it (sometimes hundreds of ms after `\026` is injected).
- A surfaceless dispatcher cannot present a Wayland surface, so it has limited rights to claim selections under mutter.
- The dispatcher exits after staging; the selection owner must either persist independently or transfer ownership atomically.

## Considered options

1. **X11-via-xclip.** `setsid xclip -i -selection clipboard -t image/png FILE &` — xclip stays alive as a detached process holding the X11 selection. mutter's bridge re-advertises it on the Wayland side.
2. **Wayland-via-wl-copy.** `wl-copy --type image/png < FILE` (optionally `--paste-once`).
3. **Bridge process.** Run a custom Wayland client (or `flashpasted` in Tier 3) that owns the data-device persistently.

## Decision outcome

**Chosen: option 1 for Tiers 1/2 — X11 via xclip. Option 3 for Tier 3 only.**

In Tier 1 and Tier 2, the dispatcher is a short-lived process. It stages into X11 via xclip (Tier 1) or `x11rb` (Tier 2). mutter's X11→Wayland bridge then surfaces the same bytes on the Wayland side, so receivers on either world see `image/png`. The Wayland path is *best-effort*: if a Wayland-native receiver picks up the bytes first, fine; if not, the X11 owner is still there.

In Tier 3, the daemon is long-lived and surfaceless-but-persistent, so it can hold *both* selections directly; option 3 only becomes viable once a daemon exists.

## Consequences

**Positive.**

- The X11 path works on every GNOME/Wayland box because XWayland is universal; no edge cases for users without it.
- xclip's detached-owner model decouples staging from dispatcher lifetime — exit-after-stage is safe.
- mutter's bridge does the cross-world replication for free.

**Negative.**

- Requires `xclip` (Tier 1) and a live XWayland server (all tiers). Documented in install requirements.
- The X11→Wayland bridge is *sticky* (see ADR 0004); we have to compensate with a Wayland-authoritative `has_image` policy when reading.

## Rejected alternatives

- **Wayland-only via `wl-copy`** was rejected because mutter refuses surfaceless clients claiming the data device under typical session configurations, and `wl-copy --paste-once` only serves a single receive — fatal when the receiver retries or probes the MIME list first.
- **Custom bridge process for Tiers 1/2** was rejected because it adds a long-running surface to the one-shot path and reintroduces the daemon-only failure modes that ADR 0001 explicitly rejected.

## References

- ADR 0001 — three progressive tiers.
- ADR 0004 — Wayland-authoritative `has_image` policy (the read-side counterpart).
- [`docs/architecture.md`](../architecture.md) — shared hot path step 2.
