//! Thin wrappers over the `tmux` CLI for the three IPC calls we still
//! need: `select-pane`, `unbind -n C-v`, and the detached rebind.
//!
//! We deliberately accept the ~5ms-per-tmux-fork cost — replacing tmux's
//! control protocol is out of scope for Phase 1. Each call is a one-shot
//! command via `std::process::Command`.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use nix::libc;

/// Exact rebind command the bash dispatcher uses. Kept as a constant so
/// the rebind always points at the canonical install location of the
/// bash script — that script is the slow-path fallback during Phase 1.
///
/// IMPORTANT: this MUST stay pointed at `tmux-paste-dispatch.sh` (NOT
/// the Rust binary) until Phase 1 is the default. The Rust binary is
/// opt-in via the README instructions; users who flip the bind to the
/// Rust binary will manually update this rebind text too.
pub const REBIND_CMD: &str =
    "tmux bind -n C-v run-shell -b \"TMUX_PASTE_TRIGGER=ctrl-v /home/deadpool/.local/bin/tmux-paste-dispatch.sh '#{pane_id}'\"";

/// `tmux select-pane -t <pane>`. Errors are silently swallowed — the
/// bash script ends with `|| true` for the same reason: pane selection
/// is a UX nicety, not load-bearing for the paste itself.
pub fn select_pane(pane: &str) {
    let _ = Command::new("tmux")
        .arg("select-pane")
        .arg("-t")
        .arg(pane)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// `tmux unbind -n C-v`. MUST run BEFORE `kitty @ send-text \x16` or
/// tmux's `bind -n C-v` will catch our synthesized Ctrl-V byte and
/// recursively re-invoke the dispatcher (see hard-won fact #2 in the
/// bash edit log).
pub fn unbind_ctrl_v() -> io::Result<()> {
    let status = Command::new("tmux")
        .arg("unbind")
        .arg("-n")
        .arg("C-v")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("tmux unbind exited with {status}"),
        ))
    }
}

/// Schedule a `tmux bind -n C-v ...` to re-arm 100ms from now, fully
/// detached from this process (setsid + new session) so it survives
/// the dispatcher exiting.
///
/// Mirrors the bash:
///
/// ```text
///   setsid -f sh -c '
///     sleep 0.1
///     tmux bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v /home/deadpool/.local/bin/tmux-paste-dispatch.sh '"'"'#{pane_id}'"'"'"
///   ' </dev/null >/dev/null 2>&1
/// ```
///
/// The 100ms delay is empirically what tmux needs to process the
/// in-flight Ctrl-V byte before the rebind re-arms (v1.4 of the bash
/// script down from v1.3's 1s).
pub fn schedule_rebind() -> io::Result<()> {
    let cmd = format!("sleep 0.1; {REBIND_CMD}");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // SAFETY: pre_exec runs in the forked child, before execve. setsid
    // is async-signal-safe. We use this rather than spawning `setsid -f`
    // so we don't need the `setsid` binary in PATH.
    unsafe {
        command.pre_exec(|| {
            let _ = libc::setsid();
            Ok(())
        });
    }
    let _child = command.spawn()?;
    Ok(())
}
