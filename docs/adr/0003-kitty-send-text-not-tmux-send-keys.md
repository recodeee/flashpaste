# ADR 0003 — Use `kitty @ send-text` for image-paste, not `tmux send-keys`

- **Status:** Accepted (verified empirically)
- **Date:** 2026-05-19
- **Tags:** kitty, tmux, transport, image-paste

## Context and problem statement

Once a screenshot is staged into the clipboard, FlashPaste has to deliver a Ctrl-V (`\026`) to the receiving terminal AI (Claude Code) so its image-paste handler runs. The TUI is running inside a tmux pane inside a kitty window. There are two obvious ways to inject the byte: `tmux send-keys -t %ID C-v`, which writes into the pty from the tmux side, or `kitty @ send-text \026`, which writes into the pty from the terminal-emulator side via kitty's remote-control protocol. The two look equivalent on a stream-of-bytes level, but they are *not* equivalent observably.

## Decision drivers

- The receiving TUI must trigger its image-paste handler, not just receive a literal Ctrl-V.
- Round-trip latency matters; whichever transport is also faster is preferred.
- The transport must be reachable from a short-lived dispatcher with no shared state.

## Considered options

1. **`tmux send-keys -t %ID C-v`.** Use tmux's existing key injection facility.
2. **`kitty @ send-text \026`.** Use kitty's remote-control protocol over the kitty unix socket.
3. **Synthesize an X11/Wayland key event** via xdotool / ydotool.

## Decision outcome

**Chosen: option 2 — `kitty @ send-text`.**

This is hard-won fact #1 in `AGENTS.md`. Routing Ctrl-V through tmux's `send-keys` writes the byte to the pty but Claude Code's image-paste handler does not fire — the byte arrives but is treated as a literal control char, not a paste-intent. Routing through kitty's IPC, by contrast, *does* fire the handler. The mechanism is not fully understood from the outside, but the empirical result is stable across kitty versions: the kitty transport works, the tmux transport does not.

In Tier 2 we go further and speak the kitty RC protocol (`\x1bP@kitty-cmd…\x1b\\`) directly over the socket, eliminating the ~25 ms Python startup of forking `kitty @`. The transport choice (kitty IPC vs tmux send-keys) is what matters; how we get there (subprocess vs direct socket) is a latency detail.

## Consequences

**Positive.**

- Image-paste works at all. This was the unblocking finding.
- The kitty socket transport is faster than `tmux send-keys` even setting aside correctness.

**Negative.**

- Hard runtime dependency on kitty with `allow_remote_control yes` (or a listen socket configured). Documented in `docs/install.md`.
- A user on a different terminal (Alacritty, foot, Ghostty) cannot use FlashPaste's image path. Mitigated only by them switching terminals; text paste is unaffected.

## Rejected alternatives

- **`tmux send-keys`** was rejected because it silently fails to trigger the image-paste handler in Claude Code. This is the canonical bug that the FlashPaste project exists to work around.
- **Synthetic key events via xdotool/ydotool** were rejected because they require focus assumptions, are racy under tiling window managers, and introduce a permissions surface (uinput on Wayland) without buying anything kitty IPC doesn't already provide.

## References

- [`AGENTS.md`](../../AGENTS.md) — hard-won fact #1.
- ADR 0005 — recursion guard around the `\026` injection.
- [`docs/architecture.md`](../architecture.md) — shared hot path step 4.
