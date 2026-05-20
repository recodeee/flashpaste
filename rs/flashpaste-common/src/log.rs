//! Logging + millisecond-resolution tracing.
//!
//! Two sinks:
//!
//! * **Per-invocation log** at `~/.local/state/flashpaste-paste.log`
//!   (override via `FLASHPASTE_LOG`). One line per checkpoint formatted
//!   identically to the bash script:
//!
//!   ```text
//!     [HH:MM:SS.mmm] T+ 123ms (Δ 12ms) :: <step>
//!   ```
//!
//! * **JSON trace sink** at `~/.local/state/flashpaste-trace.jsonl`
//!   (override via `FLASHPASTE_TRACE_LOG`). Only written when
//!   `FLASHPASTE_TRACE=1` is set. One JSON object per line:
//!
//!   ```json
//!     {"trace":"<id>","t_ms":N,"delta_ms":N,"step":"...","ts":"...Z"}
//!   ```
//!
//! `FLASHPASTE_QUIET=1` short-circuits everything (no file IO, no clock
//! read). Same semantics as the bash script.

use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use once_cell::sync::OnceCell;

use crate::paths::{default_log_path, default_trace_log_path};

/// Global timing state. Initialized on first call to [`init`]; thereafter
/// every [`emit`] computes T+/Δ relative to `start`.
struct State {
    start: Instant,
    prev: Mutex<Instant>,
    trace_id: String,
    quiet: bool,
    log_file: Option<Mutex<File>>,
    trace_file: Option<Mutex<File>>,
}

static STATE: OnceCell<State> = OnceCell::new();

/// Initialize the logging subsystem. Idempotent — subsequent calls are
/// no-ops. Must be called once at the start of `main` before any [`t`]
/// macro invocation.
///
/// Reads the following env vars:
/// * `FLASHPASTE_QUIET=1` — disable all logging.
/// * `FLASHPASTE_LOG` — override per-invocation log path.
/// * `FLASHPASTE_TRACE=1` — enable JSON sink.
/// * `FLASHPASTE_TRACE_LOG` — override JSON sink path.
pub fn init() {
    let _ = STATE.get_or_init(|| {
        let quiet = env::var("FLASHPASTE_QUIET").ok().as_deref() == Some("1");
        let trace_enabled = env::var("FLASHPASTE_TRACE").ok().as_deref() == Some("1");

        let log_file = if quiet {
            None
        } else {
            let path = env::var_os("FLASHPASTE_LOG")
                .map(PathBuf::from)
                .unwrap_or_else(default_log_path);
            open_append(&path).map(Mutex::new)
        };
        let trace_file = if quiet || !trace_enabled {
            None
        } else {
            let path = env::var_os("FLASHPASTE_TRACE_LOG")
                .map(PathBuf::from)
                .unwrap_or_else(default_trace_log_path);
            open_append(&path).map(Mutex::new)
        };

        let now = Instant::now();
        State {
            start: now,
            prev: Mutex::new(now),
            // Bash uses "<unix_seconds>-<pid>" — match exactly.
            trace_id: format!(
                "{}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                process::id()
            ),
            quiet,
            log_file,
            trace_file,
        }
    });
}

/// Open `path` for append, creating parent dirs as needed. Returns None
/// on any IO error — logging is best-effort.
fn open_append(path: &std::path::Path) -> Option<File> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Emit a checkpoint. Use the [`t`](crate::t) macro for the canonical
/// call site; this function is the public entry point for non-macro
/// callers (e.g. dynamic step names).
pub fn emit(step: &str) {
    let Some(state) = STATE.get() else { return };
    if state.quiet {
        return;
    }
    let now = Instant::now();
    let total_ms = now.duration_since(state.start).as_millis() as u64;
    let delta_ms = {
        let mut prev = match state.prev.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let d = now.duration_since(*prev).as_millis() as u64;
        *prev = now;
        d
    };

    if let Some(file) = &state.log_file {
        // Format matches bash:
        //   [HH:MM:SS.mmm] T+ 123ms (Δ 12ms) :: <step>
        let wall_clock = format_wall_clock();
        let line = format!(
            "[{wall_clock}] T+{:>4}ms (Δ{:>3}ms) :: {step}\n",
            total_ms, delta_ms
        );
        if let Ok(mut f) = file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    if let Some(file) = &state.trace_file {
        let ts = format_iso8601_utc();
        // Escape step for JSON. step is typically a static ASCII string
        // (e.g. "fast-path after-send-text") but be safe with " and \.
        let step_escaped = escape_json(step);
        let line = format!(
            "{{\"trace\":\"{}\",\"t_ms\":{},\"delta_ms\":{},\"step\":\"{}\",\"ts\":\"{}\"}}\n",
            state.trace_id, total_ms, delta_ms, step_escaped, ts
        );
        if let Ok(mut f) = file.lock() {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Emit a final `__exit` JSON trace record. Mirrors bash's EXIT trap.
/// Only writes to the JSON sink (the human log doesn't need this).
pub fn emit_exit(rc: i32) {
    let Some(state) = STATE.get() else { return };
    if state.quiet {
        return;
    }
    let Some(file) = &state.trace_file else {
        return;
    };
    let total_ms = Instant::now().duration_since(state.start).as_millis() as u64;
    let ts = format_iso8601_utc();
    let line = format!(
        "{{\"trace\":\"{}\",\"t_ms\":{},\"delta_ms\":0,\"step\":\"__exit\",\"exit\":{},\"ts\":\"{}\"}}\n",
        state.trace_id, total_ms, rc, ts
    );
    if let Ok(mut f) = file.lock() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Format `SystemTime::now()` as `HH:MM:SS.mmm` in local time.
fn format_wall_clock() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = now.as_secs();
    let millis = now.subsec_millis();
    // Compute local time-of-day. We don't pull in chrono; use libc
    // localtime_r for the tz offset.
    let (hh, mm, ss) = local_hms(total_secs);
    format!("{hh:02}:{mm:02}:{ss:02}.{millis:03}")
}

/// Format `SystemTime::now()` as `YYYY-MM-DDTHH:MM:SS.mmmZ` in UTC.
fn format_iso8601_utc() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let (y, mo, d, hh, mm, ss) = gmtime_components(secs);
    format!("{y:04}-{mo:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
}

/// Local hours-minutes-seconds via libc::localtime_r. Async-signal-safe
/// enough for our purposes — we don't call this from a signal handler.
fn local_hms(unix_secs: u64) -> (u8, u8, u8) {
    use nix::libc;
    let t: libc::time_t = unix_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::localtime_r(&t, &mut tm) };
    if rc.is_null() {
        return (0, 0, 0);
    }
    (tm.tm_hour as u8, tm.tm_min as u8, tm.tm_sec as u8)
}

/// UTC year/month/day/hh/mm/ss via libc::gmtime_r.
fn gmtime_components(unix_secs: i64) -> (i32, u8, u8, u8, u8, u8) {
    use nix::libc;
    let t: libc::time_t = unix_secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::gmtime_r(&t, &mut tm) };
    if rc.is_null() {
        return (1970, 1, 1, 0, 0, 0);
    }
    (
        tm.tm_year + 1900,
        (tm.tm_mon + 1) as u8,
        tm.tm_mday as u8,
        tm.tm_hour as u8,
        tm.tm_min as u8,
        tm.tm_sec as u8,
    )
}

/// Minimal JSON string escaper — handles `"`, `\`, and control chars.
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Checkpoint macro. Usage: `t!("fast-path after-send-text");`.
#[macro_export]
macro_rules! t {
    ($step:literal $(,)?) => {
        $crate::log::emit($step)
    };
    ($($arg:tt)*) => {
        $crate::log::emit(&format!($($arg)*))
    };
}
