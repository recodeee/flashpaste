//! flashpaste-common — shared building blocks for the Rust fast path.
//!
//! Phase 1 goal: replace `bin/tmux-paste-dispatch.sh`'s ~127ms fast path with
//! a sub-40ms native binary. The four hard-won facts from the bash edit log
//! MUST be preserved by every consumer of this crate:
//!
//! 1. `kitty @ send-text` is the only transport that triggers Claude Code's
//!    image-paste handler.
//! 2. Tmux's `bind -n C-v` re-invokes the dispatcher when kitty `send-text`
//!    injects `\026`. Unbind before send-text, schedule a detached rebind
//!    ~100ms later so it survives this binary exiting.
//! 3. Wayland-authoritative `has_image` policy — if Wayland answers, trust
//!    it and ignore X11's stale mirror.
//! 4. GNOME PrtScr saves a file but doesn't copy to clipboard; auto-pickup
//!    loads the latest screenshot if it's ≤30s old and the clipboard text
//!    is empty.

pub mod clipboard;
pub mod kitty_ipc;
pub mod log;
pub mod paths;
pub mod recursion_guard;
pub mod screenshot;
pub mod tmux;
