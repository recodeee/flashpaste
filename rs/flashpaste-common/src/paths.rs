//! Filesystem paths the dispatch binary needs to locate at runtime.
//!
//! All helpers are pure stdlib — no env-crate, no dirs-crate. The bash
//! script uses literal paths, so we match that exactly to avoid surprises.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use nix::unistd::Uid;

/// Resolve `$XDG_RUNTIME_DIR`, falling back to `/run/user/<uid>` (which is
/// what GNOME on Ubuntu actually populates), and finally `/tmp` if even
/// that's missing. Mirrors the bash `${XDG_RUNTIME_DIR:-/run/user/$(id -u)}`
/// idiom.
pub fn xdg_runtime_dir() -> PathBuf {
    if let Ok(dir) = env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    let uid = Uid::current().as_raw();
    let candidate = PathBuf::from(format!("/run/user/{uid}"));
    if candidate.is_dir() {
        return candidate;
    }
    PathBuf::from("/tmp")
}

/// Path to the recursion-guard lock file. Matches the bash script's
/// `RECURSION_LOCK="${XDG_RUNTIME_DIR:-/tmp}/tmux-paste-dispatch.lock"`.
pub fn recursion_lock_path() -> PathBuf {
    xdg_runtime_dir().join("tmux-paste-dispatch.lock")
}

/// `~/Pictures/Screenshots`. Mirrors the bash `_early_ss_dir`.
pub fn screenshots_dir() -> Option<PathBuf> {
    let home = env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Pictures").join("Screenshots"))
}

/// Scan `/run/user/<uid>/` for the live kitty IPC socket (`kitty-main-*`).
///
/// The bash script globs `for sock_path in /run/user/$(id -u)/kitty-main-*`
/// and takes the first socket it finds. We do the same: read_dir + filter
/// by prefix + check it's actually a socket via `file_type().is_socket()`.
pub fn kitty_socket() -> Option<PathBuf> {
    let dir = xdg_runtime_dir();
    let entries = fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.starts_with("kitty-main-") {
            continue;
        }
        // file_type().is_socket() avoids one extra stat compared to metadata().
        let Ok(ft) = entry.file_type() else { continue };
        use std::os::unix::fs::FileTypeExt;
        if ft.is_socket() {
            return Some(entry.path());
        }
    }
    None
}

/// Default per-invocation log path. Matches the bash script's
/// `~/.local/state/tmux-paste.log` — but the Rust binary writes its own
/// stream to `flashpaste-paste.log` so the two implementations can be
/// compared head-to-head during the Phase 1 cutover.
pub fn default_log_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home)
        .join(".local")
        .join("state")
        .join("flashpaste-paste.log")
}

/// Path for the JSON trace sink. Matches the bash
/// `FLASHPASTE_TRACE_LOG=~/.local/state/flashpaste-trace.jsonl` so the
/// analyzer can group bash and Rust invocations side-by-side.
pub fn default_trace_log_path() -> PathBuf {
    let home = env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    Path::new(&home)
        .join(".local")
        .join("state")
        .join("flashpaste-trace.jsonl")
}
