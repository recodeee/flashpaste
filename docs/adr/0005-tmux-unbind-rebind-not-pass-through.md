# ADR 0005 — Unbind + detached-rebind C-v around send-text, not pass-through

- **Status:** Accepted
- **Date:** 2026-05-19
- **Tags:** tmux, recursion, keybinding, hot-path

## Context and problem statement

The tmux binding that triggers FlashPaste is `bind -n C-v run-shell tmux-paste-dispatch.sh`. The dispatcher then injects `\026` (raw Ctrl-V) via `kitty @ send-text`. kitty writes the byte into the pty, which tmux is reading on the other end. Tmux sees a Ctrl-V, matches it against the root-table binding, and runs the dispatcher *again*. Without a guard this recurses infinitely until tmux runs out of children. The dispatcher therefore has to manipulate its own trigger binding around the send-text call.

## Decision drivers

- The keystroke that initiates a paste *must* still be bound (`bind -n C-v`) so the user can press Ctrl-V to paste; we cannot leave the binding off permanently.
- The dispatcher exits immediately after sending — it cannot block until kitty's IPC round-trips before rebinding.
- The recursion must be broken deterministically, not via timing luck.

## Considered options

1. **Prefix-only binding.** `bind C-v run-shell …` (no `-n`), so the user must press `prefix C-v`. Eliminates root-table recursion entirely.
2. **Pass-through binding.** Configure tmux so root C-v still reaches the dispatcher but the synthesized `\026` is interpreted as a literal. tmux has no first-class mechanism for this.
3. **Unbind + send + detached rebind.** `tmux unbind -n C-v && kitty @ send-text \026 && setsid -f sh -c 'sleep 0.1; tmux bind -n C-v …'`.

## Decision outcome

**Chosen: option 3 — unbind, send, detached-rebind.**

The dispatcher:

1. Acquires a recursion-guard lock at `$XDG_RUNTIME_DIR/tmux-paste-dispatch.lock` (no-op if held within 2 s).
2. Runs `tmux unbind -n C-v` so the synthesized byte cannot re-fire the binding.
3. Runs `kitty @ send-text \026`.
4. Spawns `setsid -f sh -c 'sleep 0.1; tmux bind -n C-v run-shell …'` — a detached process that survives the dispatcher exit and rebinds ~100 ms later.

This is hard-won fact #2 in `AGENTS.md`.

## Consequences

**Positive.**
- No infinite recursion under any timing.
- The user's next keystroke (~100 ms+ later in human reaction time) reliably sees the binding active again.
- The `setsid -f` detach means the dispatcher exits cleanly — the rebind isn't blocked on the dispatcher staying alive.

**Negative.**
- There is a ~100 ms window where root-table C-v is unbound. If the user mashes paste twice within that window, the second press is a literal Ctrl-V in whichever app has focus. The lock file at `$XDG_RUNTIME_DIR/tmux-paste-dispatch.lock` is the belt-and-braces guard: even if the second press *does* arrive while the binding is still active (e.g. rebind got there first), the lock no-ops the second invocation.
- The 100 ms sleep is a tunable; lowering it reduces the unbound window but risks racing the kitty IPC round-trip and re-arming before the byte is delivered.

## Rejected alternatives

- **Prefix-only binding** was rejected because the user explicitly wants root C-v to paste — putting it behind the tmux prefix defeats the muscle-memory goal of FlashPaste.
- **Pass-through binding** was rejected because tmux has no API to mark a specific incoming byte as "do not match against the key table." Every workaround in this direction (custom key tables, switch-client into a transient table) re-introduces the same recursion under a different name.

## References

- [`AGENTS.md`](../../AGENTS.md) — hard-won fact #2.
- ADR 0003 — the `kitty @ send-text` transport that makes recursion possible in the first place.
- [`docs/architecture.md`](../architecture.md) — the recursion-guard mechanism section.
