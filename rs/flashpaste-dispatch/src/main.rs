//! flashpaste-dispatch — Rust replacement for `tmux-paste-dispatch.sh`'s
//! fast path. Phase 1 of the flashpaste perf plan.
//!
//! Target: <40ms paste-to-byte latency (bash baseline ~127ms).
//!
//! See `/home/deadpool/Documents/flashpaste/bin/tmux-paste-dispatch.sh`
//! for the canonical behaviour we are matching. The four hard-won facts
//! from the bash edit log are preserved here — see comments on each
//! flow step.

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use anyhow::Result;
use clap::{Parser, Subcommand};
use flashpaste_common::{
    clipboard, kitty_ipc, log as flog, paths, recursion_guard, screenshot, t, tmux,
};

mod hold_selection;

/// CLI surface. The default mode is the public dispatcher; the hidden
/// `__hold-selection` subcommand is internal and re-exec'd by
/// `clipboard::stage_image`.
#[derive(Parser)]
#[command(
    name = "flashpaste-dispatch",
    about = "Sub-40ms tmux paste dispatcher (Rust fast path).",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<HiddenCmd>,

    /// Tmux pane id (e.g. `%4`). Required for the default dispatch mode.
    /// When the hidden subcommand is given, this is ignored.
    pane: Option<String>,
}

#[derive(Subcommand)]
enum HiddenCmd {
    /// Internal: become the long-lived X11 selection owner for an image.
    /// Re-exec'd by `clipboard::stage_image`. Not meant for users.
    #[command(name = "__hold-selection", hide = true)]
    HoldSelection {
        #[arg(long)]
        mime: String,
        #[arg(long)]
        path: PathBuf,
        /// Pipe fd inherited from the parent. We write one byte to it
        /// AFTER `SetSelectionOwner` succeeds, then enter the event loop.
        #[arg(long = "ready-fd")]
        ready_fd: Option<i32>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Some(HiddenCmd::HoldSelection {
        mime,
        path,
        ready_fd,
    }) = cli.command
    {
        return match hold_selection::run(&path, &mime, ready_fd) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("flashpaste-dispatch __hold-selection failed: {e:#}");
                ExitCode::FAILURE
            }
        };
    }

    let pane = match cli.pane {
        Some(p) if !p.is_empty() => p,
        _ => {
            eprintln!("flashpaste-dispatch: missing pane id (e.g. '%4')");
            return ExitCode::FAILURE;
        }
    };

    flog::init();
    t!("script-start");

    let rc = match dispatch(&pane) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("flashpaste-dispatch: {e:#}");
            1
        }
    };
    flog::emit_exit(rc);
    if rc == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Main dispatch flow. Mirrors the bash FAST PATH plus the fallback to
/// the bash slow path when no fresh screenshot is available.
fn dispatch(pane: &str) -> Result<()> {
    // 1. Recursion guard — hard-won fact #2's mitigation. If the lock
    //    exists and is <2s old, exit silently (this is a re-entry from
    //    kitty `send-text \x16` re-firing tmux's `bind -n C-v`).
    match recursion_guard::acquire() {
        Ok(None) => return Ok(()),
        Ok(Some(_guard)) => {
            // Drop is a no-op; let the lock age out (see module docs).
        }
        Err(e) => {
            // Non-fatal — proceed without the guard. The bash script
            // also tolerates `: >"$LOCK"` IO errors via `2>/dev/null`.
            tracing::warn!("recursion_guard::acquire failed: {e}");
        }
    }
    t!("recursion-guard-passed");

    // 2. Select target pane (UX nicety; failure is ignored).
    tmux::select_pane(pane);
    t!("select-pane");

    // 3. Try to find a fresh screenshot. Hard-won fact #4: GNOME PrtScr
    //    saves a file but doesn't copy; we auto-pickup if ≤30s old AND
    //    the clipboard text is empty.
    let mut early_loaded = false;
    let mut early_mime: &str = "image/png";

    if let Some(ss_dir) = paths::screenshots_dir() {
        if let Some((path, age, mime)) = screenshot::find_latest(&ss_dir, 30) {
            // Skip if there's already text on the clipboard — the user
            // probably copied a URL/snippet between PrtScr and Ctrl-V.
            if !clipboard::clipboard_has_text() {
                t!("early-preload before-xclip");
                match clipboard::stage_image(&path, mime) {
                    Ok(()) => {
                        early_loaded = true;
                        early_mime = mime;
                        t!("early-preload after-stage");
                        tracing::info!(
                            "early-preload: {} ({}s old) -> X11 CLIPBOARD ({})",
                            path.display(),
                            age,
                            mime
                        );
                    }
                    Err(e) => {
                        // stage_image failed (no DISPLAY, no x11rb, holder
                        // child died, etc.). Fall through to the bash
                        // slow-path which has its own `setsid xclip`.
                        tracing::warn!("stage_image failed: {e:#}");
                    }
                }
            }
        }
    }

    // 4. If the image is staged, jump straight to send-text.
    if early_loaded {
        return fast_path(pane, early_mime);
    }

    // 5. No fresh image → defer to the bash dispatcher for the slow path.
    //    We deliberately don't reimplement the slow path in Phase 1: it
    //    handles many edge cases (text-paste branches, ydotool, divergence
    //    forensics) and reusing the proven bash code keeps the surface
    //    area of this Phase 1 binary small.
    t!("fallback-to-bash");
    tracing::info!("no fresh screenshot — invoking bash slow path");
    let slow_path = std::path::PathBuf::from("/home/deadpool/.local/bin/tmux-paste-dispatch.sh");
    let status = Command::new(&slow_path)
        .arg(pane)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!("bash slow-path exited with {status}");
    }
    Ok(())
}

/// Fast path proper: unbind tmux, send `\x16` via kitty IPC, schedule
/// rebind, exit. Preserves hard-won facts #1 and #2.
fn fast_path(_pane: &str, _early_mime: &str) -> Result<()> {
    let Some(sock) = paths::kitty_socket() else {
        anyhow::bail!("no kitty IPC socket found in /run/user/$(uid)/kitty-main-*");
    };

    t!("fast-path before-unbind");
    // Hard-won fact #2: unbind BEFORE send-text so tmux doesn't catch
    // the synthesized \x16 byte and recursively re-invoke us.
    if let Err(e) = tmux::unbind_ctrl_v() {
        tracing::warn!("tmux unbind -n C-v failed: {e}");
    }
    t!("fast-path after-unbind");

    // Hard-won fact #1: kitty @ send-text is the only transport that
    // triggers Claude Code's image-paste handler. We speak the kitty
    // RC protocol directly (avoiding the ~25ms python startup).
    kitty_ipc::send_text_focused(&sock, b"\x16")?;
    t!("fast-path after-send-text");

    // Hard-won fact #2 (rebind side): the rebind must be detached so
    // it survives this binary exiting. tmux::schedule_rebind uses
    // setsid via pre_exec.
    if let Err(e) = tmux::schedule_rebind() {
        tracing::warn!("schedule_rebind failed: {e}");
    }
    t!("fast-path exit");
    Ok(())
}
