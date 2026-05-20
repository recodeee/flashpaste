//! Clipboard staging.
//!
//! Two public entry points:
//!
//! * [`clipboard_has_text`] — best-effort check, with the PNG-magic gotcha
//!   from the bash early-preload block: a `text/plain` target whose first
//!   bytes are `\x89PNG` is NOT text. See bash comment around line ~283.
//!
//! * [`stage_image`] — load `path` (with explicit `mime`) into the X11
//!   `CLIPBOARD` selection and return as quickly as possible. The
//!   ownership lives on in a detached child that re-execs ourselves
//!   as a hidden subcommand `__hold-selection`. The child is the long-
//!   lived X11 selection owner; this binary exits immediately after.
//!
//! ## NOTE: fork strategy
//!
//! We use the **hidden-subcommand + pipe-handshake** strategy. Rationale:
//!
//! 1. x11rb's `Connection` cannot be safely inherited across `fork()` in
//!    the general case (xcb's mutex/state is process-wide and the child
//!    would race on the same fd). Re-executing means the child gets a
//!    fresh connection.
//!
//! 2. To eliminate the bash script's 50ms sleep, the parent needs to know
//!    the child is actually owning the selection before returning. We
//!    pass a pipe via `CommandExt::pre_exec` (fd inheritance) — the
//!    parent reads one byte from the read-end, the child writes that
//!    byte AFTER `SetSelectionOwner` succeeds. Round-trip is microseconds,
//!    not milliseconds. If anything goes wrong the parent gets EOF on the
//!    pipe and falls through with an Err — the caller can choose to fall
//!    back to spawning `xclip` exactly like the bash script does.
//!
//! 3. Re-exec also means the child process group is independent — no
//!    SIGHUP issues when the dispatch binary's parent shell closes the
//!    pty.

use std::env;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use nix::libc;

/// Best-effort "is there text on the clipboard right now?" probe.
///
/// Implementation detail: arboard's `get_text()` may succeed with bytes
/// even when the actual owner only advertises an image; if those bytes
/// turn out to be a PNG header, we treat that as "no text" (mirroring
/// the bash `case "$_early_xt" in $'\x89PNG'*) ;; esac` gotcha around
/// line ~283).
///
/// Returns `false` for any error (no clipboard, no display, etc.) —
/// the caller's contract is "do not clobber existing text"; if we can't
/// tell, we conservatively assume there is none. The bash script does
/// the same: a `timeout 0.15 xclip` that errors-out is treated as empty.
pub fn clipboard_has_text() -> bool {
    let Ok(mut cb) = arboard::Clipboard::new() else {
        return false;
    };
    match cb.get_text() {
        Ok(s) => {
            let bytes = s.as_bytes();
            // PNG magic: 0x89 P N G (\x89\x50\x4e\x47).
            if bytes.starts_with(b"\x89PNG") {
                return false;
            }
            !s.is_empty()
        }
        Err(_) => false,
    }
}

/// Stage `path` as the active X11 `CLIPBOARD` selection with the given
/// MIME (`"image/png"`, `"image/jpeg"`, …). Returns as soon as the
/// detached holder child has confirmed ownership via the pipe handshake.
///
/// On success the X11 selection is owned by a detached grandchild
/// (the `__hold-selection` re-exec). That child holds the selection for
/// ~10 seconds — long enough for the kitty send-text round-trip and the
/// inner TUI's `wl-paste -t image/png` (via the wl-paste shim that falls
/// back to xclip / XGetSelectionOwner).
pub fn stage_image(path: &Path, mime: &str) -> Result<()> {
    // Find our own executable so we can re-exec the hidden subcommand.
    let exe = env::current_exe().context("locating current_exe for re-exec")?;

    // Pipe for the readiness handshake. The READ end stays in the parent;
    // the WRITE end is moved into the child via fd inheritance.
    let (read_fd, write_fd) = pipe_cloexec().context("creating handshake pipe")?;
    let write_raw = write_fd.into_raw_fd();

    // Spawn the holder. setsid detaches into a new session so it survives
    // this process exiting (identical reasoning to the tmux rebind dance).
    let mut cmd = Command::new(exe);
    cmd.arg("__hold-selection")
        .arg("--mime")
        .arg(mime)
        .arg("--path")
        .arg(path)
        .arg("--ready-fd")
        .arg(write_raw.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // SAFETY: pre_exec runs in the forked child between fork and execve.
    // We do ONLY async-signal-safe operations: setsid() and fcntl() to
    // clear O_CLOEXEC on the write fd so it survives the execve. Both
    // are listed in signal-safety(7).
    unsafe {
        cmd.pre_exec(move || {
            // setsid: detach into a new session/process-group so SIGHUP
            // from the dispatcher's closing pty doesn't kill the holder.
            // Ignore failure: we may already be a session leader.
            let _ = libc::setsid();
            // Clear FD_CLOEXEC on write_raw so it survives execve.
            let flags = libc::fcntl(write_raw, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(write_raw, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            }
            Ok(())
        });
    }

    let _child = cmd.spawn().context("spawning __hold-selection child")?;

    // Close our parent-side copy of the write end so we see EOF if the
    // child dies before signalling. SAFETY: write_raw was created by
    // pipe2 and we still own this numeric fd in the parent.
    unsafe {
        libc::close(write_raw);
    }

    // Wait for the child to signal readiness (one byte). Capped to a
    // generous timeout — if the holder really can't claim the selection
    // we don't want to hang forever.
    wait_ready(read_fd, Duration::from_millis(500))
        .context("waiting for selection-owner readiness handshake")?;
    Ok(())
}

/// Create an `O_CLOEXEC` pipe. Returns `(read_end, write_end)`.
fn pipe_cloexec() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: pipe2 writes two valid fds on success.
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe2 just gave us two owned fds; transferring ownership.
    unsafe { Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
}

/// Read exactly one byte from `fd`, blocking up to `timeout`. Used as the
/// readiness handshake: child writes a byte AFTER `SetSelectionOwner`,
/// parent reads it and returns.
fn wait_ready(fd: OwnedFd, timeout: Duration) -> io::Result<()> {
    // poll(2) for the timeout, then read one byte.
    let raw = fd.as_raw_fd();
    let mut pfd = libc::pollfd {
        fd: raw,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    // SAFETY: pfd is a single valid pollfd by value.
    let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    if rc == 0 {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "selection-owner handshake timed out",
        ));
    }
    let mut buf = [0u8; 1];
    // Transfer ownership of the fd to File so it gets closed exactly once.
    let mut f = std::fs::File::from(fd);
    f.read_exact(&mut buf)?;
    Ok(())
}
