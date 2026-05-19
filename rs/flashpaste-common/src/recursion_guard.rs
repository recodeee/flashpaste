//! Recursion guard — mtime-based 2-second lock.
//!
//! The bash script's logic, reproduced verbatim:
//!
//! ```text
//!   if [ -e "$RECURSION_LOCK" ]; then
//!     lock_age=$(($(date +%s) - $(stat -c %Y "$RECURSION_LOCK")))
//!     if [ "$lock_age" -lt 2 ]; then exit 0; fi
//!   fi
//!   : >"$RECURSION_LOCK"
//!   ( sleep 3; rm -f "$RECURSION_LOCK" ) &
//! ```
//!
//! Why mtime instead of pidfile or flock: the recursive `tmux bind -n C-v`
//! invocation that fires from kitty's `send-text \026` runs concurrently
//! with our parent — if we removed the lock on exit, that recursion would
//! see no lock and re-paste. The lock must outlive THIS process, hence
//! "let it age out via mtime."
//!
//! We replicate the same shape: `acquire()` returns `Ok(Some(Guard))` if
//! we got the lock, `Ok(None)` if it's already held (recursion tripped).
//! Drop is intentionally a no-op — see comment on [`Guard`].

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use nix::libc;

use crate::paths::recursion_lock_path;

/// Age (in seconds) below which an existing lock is considered "live."
/// Matches the bash `if [ "$lock_age" -lt 2 ]`.
pub const LOCK_WINDOW: Duration = Duration::from_secs(2);

/// Background cleanup delay. Bash uses `( sleep 3; rm -f $LOCK )`. We do
/// the same — fork a detached `sh -c 'sleep 3; rm -f ...'` because we
/// need it to outlive this process.
pub const LOCK_CLEANUP_DELAY_SECS: u64 = 3;

/// RAII handle returned by [`acquire`]. Intentionally a no-op on Drop —
/// see module-level docs. Holding the value is purely informational so
/// callers can name the lifetime "the lock is mine."
pub struct Guard {
    /// Path of the lock file. Kept so callers can introspect / log.
    pub path: PathBuf,
}

/// Try to acquire the recursion-guard lock.
///
/// Returns:
/// * `Ok(Some(Guard))` — we won the race; the lock now exists with a
///   fresh mtime, and a detached cleanup child will `rm -f` it in
///   ~3 seconds.
/// * `Ok(None)` — an existing lock with mtime <2s ago is present.
///   Caller should exit cleanly (this is the "recursion tripped" case
///   from kitty's `send-text \026` re-firing tmux's `bind -n C-v`).
/// * `Err(_)` — IO error reading or writing the lock file. The bash
///   script ignores most IO errors here (`2>/dev/null || echo 0`); we
///   propagate so the caller can log and continue.
pub fn acquire() -> io::Result<Option<Guard>> {
    let path = recursion_lock_path();
    acquire_at(&path)
}

/// Lower-level entry point for tests — accepts an explicit lock path.
pub fn acquire_at(path: &Path) -> io::Result<Option<Guard>> {
    if let Ok(meta) = fs::metadata(path) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(age) = SystemTime::now().duration_since(mtime) {
                if age < LOCK_WINDOW {
                    return Ok(None);
                }
            }
        }
    }
    // Touch the file with a fresh mtime. `: >"$LOCK"` in bash truncates
    // or creates. OpenOptions with write+create+truncate matches.
    let _ = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    spawn_cleanup(path);
    Ok(Some(Guard {
        path: path.to_path_buf(),
    }))
}

/// Spawn the detached `sleep 3; rm -f $LOCK` child. Errors here are
/// non-fatal — the lock will be stale-ish but the next invocation more
/// than 2s later will refresh its mtime anyway.
fn spawn_cleanup(path: &Path) {
    // We use `sh -c` for shell-style `sleep ...; rm -f ...` and call
    // setsid() in pre_exec so the child gets its own session — that way
    // SIGHUP from the dispatcher's closing pty can't reach it (same trick
    // bash uses with `setsid -f`).
    let cmd = format!(
        "sleep {LOCK_CLEANUP_DELAY_SECS}; rm -f {}",
        shell_quote(path)
    );
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(&cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: pre_exec runs between fork and execve; setsid is
    // async-signal-safe (signal-safety(7)).
    unsafe {
        command.pre_exec(|| {
            let _ = libc::setsid();
            Ok(())
        });
    }
    let _ = command.spawn();
}

/// Minimal POSIX shell quoter — safe single-quote escape. Lock paths come
/// from `$XDG_RUNTIME_DIR` so they shouldn't contain spaces, but it's free.
fn shell_quote(path: &Path) -> String {
    let s = path.to_string_lossy();
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Touch a file to "now" without opening it for writes. Used in unit tests.
#[doc(hidden)]
pub fn touch_now(path: &Path) -> io::Result<()> {
    File::create(path)?;
    Ok(())
}
