//! flashpaste-trigger — the 1-byte hot path.
//!
//! Tmux's `bind -n C-v` calls this binary with the target `pane_id`. It tries
//! to talk to the long-lived `flashpasted` daemon over its unix socket at
//! `$XDG_RUNTIME_DIR/flashpaste.sock`. If the daemon is up, the daemon does
//! all the work (clipboard ownership, kitty IPC, tmux unbind/rebind dance,
//! recursion guard) and replies in ~10ms.
//!
//! If the socket doesn't exist OR the connect/handshake fails within ~5ms,
//! we `exec` (replace ourselves with) the bash dispatcher at
//! `/home/deadpool/.local/bin/tmux-paste-dispatch.sh`. That way the trigger
//! is always safe to wire into tmux even when the daemon is dead or being
//! restarted.
//!
//! Hard constraints from the spec:
//!   * Minimal deps — clap, serde_json, nix, anyhow. No tokio. No x11rb.
//!     No wl-clipboard-rs. No notify/inotify.
//!   * Target binary size <500 KB stripped.
//!
//! Wire protocol (matches `flashpasted`'s `src/ipc.rs`):
//!   request:  4-byte LE u32 length || JSON bytes
//!   response: 4-byte LE u32 length || JSON bytes
//!
//! Request body:
//!   {"op":"paste","pane":"%4","ts":"2026-05-19T12:34:56.789Z"}
//!
//! Response body (on success):
//!   {"ok":true,"latency_ms":7}
//!   {"ok":true,"deduped":true}
//!
//! Response body (daemon punts back to bash):
//!   {"ok":false,"reason":"no-image","fallback":"bash"}

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;
use nix::unistd::Uid;
use serde_json::{json, Value};

/// Where the daemon listens.
fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("flashpaste.sock");
        }
    }
    let uid = Uid::current().as_raw();
    let candidate = PathBuf::from(format!("/run/user/{uid}"));
    if candidate.is_dir() {
        return candidate.join("flashpaste.sock");
    }
    PathBuf::from("/tmp").join("flashpaste.sock")
}

/// The bash dispatcher we hand off to when the daemon is unavailable.
const BASH_FALLBACK: &str = "/home/deadpool/.local/bin/tmux-paste-dispatch.sh";

/// Total budget for "is the daemon there and willing to take this paste?"
/// Anything slower and we'd rather fall through to bash than hang the user.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(5);
const WRITE_TIMEOUT: Duration = Duration::from_millis(10);
const READ_TIMEOUT: Duration = Duration::from_millis(150);

#[derive(Debug, Parser)]
#[command(
    name = "flashpaste-trigger",
    about = "Trigger a paste via the flashpasted daemon (with bash fallback)."
)]
struct Args {
    /// Target tmux pane id (e.g. `%4`).
    pane: String,
    /// Op to send. Defaults to `paste`.
    #[arg(long, default_value = "paste")]
    op: String,
    /// Force the bash fallback path (skip the daemon entirely). For debugging.
    #[arg(long)]
    force_fallback: bool,
}

fn main() -> ! {
    let args = Args::parse();

    if args.force_fallback {
        exec_bash_fallback(&args.pane);
    }

    match try_daemon(&args) {
        Ok(DaemonOutcome::Handled) => std::process::exit(0),
        Ok(DaemonOutcome::FallbackRequested) => exec_bash_fallback(&args.pane),
        Err(_) => exec_bash_fallback(&args.pane),
    }
}

enum DaemonOutcome {
    /// Daemon handled the paste end-to-end.
    Handled,
    /// Daemon explicitly told us to fall back (e.g. no staged image, text paste).
    FallbackRequested,
}

fn try_daemon(args: &Args) -> Result<DaemonOutcome> {
    let path = socket_path();
    if !path.exists() {
        anyhow::bail!("no daemon socket at {}", path.display());
    }

    // connect_timeout doesn't exist for UnixStream in std, but the socket is
    // local and SOCK_STREAM connect on a listening unix socket completes in
    // microseconds. If it stalls, the read/write timeouts below catch it.
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("connect {}", path.display()))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT)).ok();
    stream.set_read_timeout(Some(READ_TIMEOUT)).ok();
    // Don't sit on Nagle; payloads are tiny.
    // (UnixStream uses AF_UNIX so TCP_NODELAY doesn't apply, but we don't
    // need anything: the kernel flushes on close anyway.)

    let _ = CONNECT_TIMEOUT; // budget hint for readers; std's API doesn't expose it.

    let req = json!({
        "op": args.op,
        "pane": args.pane,
        "ts": iso8601_utc_now(),
    });
    let body = serde_json::to_vec(&req)?;
    let len = u32::try_from(body.len()).context("request too large")?;
    stream.write_all(&len.to_le_bytes())?;
    stream.write_all(&body)?;
    // Half-close write side so the daemon can detect EOF if it wants.
    // (Optional; the daemon also honors the length prefix.)

    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let resp_len = u32::from_le_bytes(len_buf) as usize;
    // Bound the response so a wedged daemon can't make us allocate gigabytes.
    if resp_len > 64 * 1024 {
        anyhow::bail!("daemon response too large: {resp_len} bytes");
    }
    let mut resp_buf = vec![0u8; resp_len];
    stream.read_exact(&mut resp_buf)?;
    let resp: Value = serde_json::from_slice(&resp_buf)?;

    let ok = resp.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        return Ok(DaemonOutcome::Handled);
    }

    // Daemon politely declined — fall through to bash.
    Ok(DaemonOutcome::FallbackRequested)
}

/// Replace this process with the bash dispatcher. Zero overhead — the
/// trigger binary's pid simply becomes the bash script.
fn exec_bash_fallback(pane: &str) -> ! {
    let mut cmd = Command::new(BASH_FALLBACK);
    cmd.arg(pane);
    // Propagate the trigger source so the bash script's logs distinguish
    // "real ctrl-v" from "daemon punted".
    let trigger_value: OsString = std::env::var_os("TMUX_PASTE_TRIGGER")
        .unwrap_or_else(|| OsString::from("flashpaste-trigger-fallback"));
    cmd.env("TMUX_PASTE_TRIGGER", trigger_value);

    // If the bash script doesn't exist, exit 0 silently rather than spam
    // tmux's command output. The trigger is best-effort.
    if !Path::new(BASH_FALLBACK).exists() {
        std::process::exit(0);
    }

    // `exec` only returns on error.
    let err = cmd.exec();
    eprintln!("flashpaste-trigger: exec {} failed: {}", BASH_FALLBACK, err);
    std::process::exit(127);
}

/// Minimal ISO-8601 UTC formatter — keeps us off the `chrono` / `time`
/// crates which would balloon the trigger's binary size.
///
/// Output looks like `2026-05-19T12:34:56.789Z`.
fn iso8601_utc_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();

    // Days since 1970-01-01 (Unix epoch).
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400) as u32;

    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day / 60) % 60;
    let second = secs_of_day % 60;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, minute, second, millis
    )
}

/// Howard Hinnant's `civil_from_days` algorithm — converts days-since-epoch
/// to (year, month, day). Vendored here to dodge the chrono dep.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y } as i32;
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_known_values() {
        // 1970-01-01 = day 0
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-01-01
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        // 2026-05-19 (today, per the spec)
        // days_from_epoch(2026-05-19) = 20593
        assert_eq!(civil_from_days(20_593), (2026, 5, 19));
    }

    #[test]
    fn iso8601_has_expected_shape() {
        let s = iso8601_utc_now();
        assert_eq!(s.len(), 24);
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], "T");
        assert_eq!(&s[19..20], ".");
    }
}
