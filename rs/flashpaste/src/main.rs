//! `flashpaste` — unified CLI front-end.
//!
//! Users have had to memorize six verbs (`flashpasted`, `flashpaste-dispatch`,
//! `flashpaste-trigger`, `flashpaste-shoot`, `flashpaste-mcp`,
//! `flashpaste-doctor`). This crate exposes one verb — `flashpaste` — with
//! subcommands that dispatch to the existing binaries. The wrapped
//! binaries are unchanged; this is purely a process-launcher.
//!
//! ## Design notes
//!
//! - Every subcommand `exec`s (well, spawns + waits) an external binary
//!   with `std::process::Command`. We inherit stdio so the output of the
//!   wrapped tool flows through unchanged.
//! - We exit with the child's exit status so shell composition
//!   (`flashpaste shoot --print-path | ...`) behaves identically to the
//!   direct binary form.
//! - `daemon` subcommands wrap `systemctl --user` instead of a
//!   `flashpaste-<x>` binary, since the daemon is a service.
//! - The doctor binary is `flashpaste-doctor` on packaged installs; on
//!   bare checkouts it lives at `bin/flashpaste-doctor.sh` and is
//!   PATH-installed without the `.sh` suffix by the deb / install.sh.
//!   We try the suffix-less name first and fall back to `.sh`.
//! - We never re-exec ourselves — clap's parser owns the top-level
//!   verb; everything past it is passed through verbatim.

use std::ffi::OsString;
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};

const FLASHPASTE_VERSION: &str = env!("CARGO_PKG_VERSION");

// ─────────────────────────────────────────────────────────────────────────
// CLI
// ─────────────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(
    name = "flashpaste",
    version = FLASHPASTE_VERSION,
    about = "Unified front-end for the flashpaste tool family.",
    long_about = "\
flashpaste wraps the six existing binaries (flashpasted, flashpaste-dispatch,
flashpaste-trigger, flashpaste-shoot, flashpaste-mcp, flashpaste-doctor)
behind a single verb. Memorize one command instead of six.

Examples:
  flashpaste shoot --interactive --print-path
  flashpaste paste %4
  flashpaste daemon status
  flashpaste doctor
  flashpaste mcp
",
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Capture a screenshot via the XDG portal (wraps `flashpaste-shoot`).
    Shoot(ShootArgs),

    /// Trigger a paste of the current clipboard into a tmux pane
    /// (wraps `flashpaste-trigger <PANE>`).
    Paste(PasteArgs),

    /// Manage the flashpasted user daemon (wraps `systemctl --user`).
    #[command(subcommand)]
    Daemon(DaemonCmd),

    /// Run the 13-probe environment doctor (wraps `flashpaste-doctor`).
    Doctor(DoctorArgs),

    /// Start the flashpaste MCP server on stdio (wraps `flashpaste-mcp`).
    ///
    /// Not typically run by humans — Claude Code / Cursor / etc. spawn
    /// this via their MCP config. Exposed here for parity.
    Mcp,

    /// Run the Rust one-shot dispatcher (Tier 2; wraps `flashpaste-dispatch`).
    Dispatch(DispatchArgs),

    /// Print the flashpaste build version and exit.
    Version,
}

#[derive(Debug, Args)]
struct ShootArgs {
    /// Open the portal's interactive area picker (default: full-screen).
    #[arg(long)]
    interactive: bool,

    /// Save the PNG to this path instead of ~/Pictures/Screenshots/.
    #[arg(long, value_name = "PATH")]
    output: Option<OsString>,

    /// Print the final PNG path to stdout (for shell composition).
    #[arg(long)]
    print_path: bool,

    /// Open the result in an annotation tool (currently a passthrough —
    /// the underlying `flashpaste-shoot` decides what to do). Reserved
    /// for forward-compatibility with the Phase 4 annotation hook.
    #[arg(long)]
    annotate: bool,
}

#[derive(Debug, Args)]
struct PasteArgs {
    /// tmux pane id, e.g. `%4`. Required.
    pane: String,
}

#[derive(Debug, Subcommand)]
enum DaemonCmd {
    /// `systemctl --user start flashpasted.service`.
    Start,
    /// `systemctl --user stop flashpasted.service`.
    Stop,
    /// `systemctl --user restart flashpasted.service`.
    Restart,
    /// `systemctl --user status flashpasted.service`.
    Status,
    /// `journalctl --user -fu flashpasted.service` (follow the log).
    Logs,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    /// Emit machine-readable JSON instead of the human table.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct DispatchArgs {
    /// tmux pane id, e.g. `%4`. Required.
    pane: String,
}

// ─────────────────────────────────────────────────────────────────────────
// main / dispatch
// ─────────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    let status: ExitStatus = match cli.cmd {
        Cmd::Shoot(args) => run_shoot(args)?,
        Cmd::Paste(args) => run_paste(args)?,
        Cmd::Daemon(sub) => run_daemon(sub)?,
        Cmd::Doctor(args) => run_doctor(args)?,
        Cmd::Mcp => run_mcp()?,
        Cmd::Dispatch(args) => run_dispatch(args)?,
        Cmd::Version => {
            println!("flashpaste {FLASHPASTE_VERSION}");
            return Ok(());
        }
    };

    // Mirror the child's exit status. `code()` is None when the child
    // was killed by a signal; in that case fall back to 1 so the shell
    // sees a non-zero exit.
    std::process::exit(status.code().unwrap_or(1));
}

// ─────────────────────────────────────────────────────────────────────────
// Subcommand runners
// ─────────────────────────────────────────────────────────────────────────

fn run_shoot(args: ShootArgs) -> Result<ExitStatus> {
    let mut cmd = Command::new("flashpaste-shoot");
    if args.interactive {
        cmd.arg("--interactive");
    }
    if let Some(path) = &args.output {
        cmd.arg("--output").arg(path);
    }
    if args.print_path {
        cmd.arg("--print-path");
    }
    if args.annotate {
        // `flashpaste-shoot` does not yet implement --annotate; we pass
        // it through so a future implementation picks it up without an
        // edit to this wrapper. If the child rejects the flag the user
        // gets a clear error message from the child itself.
        cmd.arg("--annotate");
    }
    spawn("flashpaste-shoot", cmd)
}

fn run_paste(args: PasteArgs) -> Result<ExitStatus> {
    let mut cmd = Command::new("flashpaste-trigger");
    cmd.arg(args.pane);
    spawn("flashpaste-trigger", cmd)
}

fn run_daemon(sub: DaemonCmd) -> Result<ExitStatus> {
    const UNIT: &str = "flashpasted.service";

    let (bin, args): (&str, Vec<&str>) = match sub {
        DaemonCmd::Start => ("systemctl", vec!["--user", "start", UNIT]),
        DaemonCmd::Stop => ("systemctl", vec!["--user", "stop", UNIT]),
        DaemonCmd::Restart => ("systemctl", vec!["--user", "restart", UNIT]),
        DaemonCmd::Status => ("systemctl", vec!["--user", "status", UNIT]),
        // `logs` is intentionally a journalctl shortcut — `systemctl
        // logs` doesn't exist on stock systemd. Following (-f) is the
        // common case for "tail the daemon while I reproduce".
        DaemonCmd::Logs => ("journalctl", vec!["--user", "-fu", UNIT]),
    };

    let mut cmd = Command::new(bin);
    cmd.args(&args);
    spawn(bin, cmd)
}

fn run_doctor(args: DoctorArgs) -> Result<ExitStatus> {
    // Try the PATH-installed `flashpaste-doctor` first; if it isn't
    // there, fall back to the bash variant `flashpaste-doctor.sh`.
    // (The packaging strips the `.sh` suffix at install time; bare
    // checkouts keep it.) Detection via `which`-style logic would add
    // an extra syscall — instead, try-and-fallback on `ENOENT`.
    let mut primary = Command::new("flashpaste-doctor");
    if args.json {
        primary.arg("--json");
    }
    match primary.spawn() {
        Ok(mut child) => Ok(child.wait().context("waiting for flashpaste-doctor")?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut fallback = Command::new("flashpaste-doctor.sh");
            if args.json {
                fallback.arg("--json");
            }
            spawn("flashpaste-doctor.sh", fallback)
        }
        Err(e) => Err(e).context("failed to spawn flashpaste-doctor"),
    }
}

fn run_mcp() -> Result<ExitStatus> {
    let cmd = Command::new("flashpaste-mcp");
    spawn("flashpaste-mcp", cmd)
}

fn run_dispatch(args: DispatchArgs) -> Result<ExitStatus> {
    let mut cmd = Command::new("flashpaste-dispatch");
    cmd.arg(args.pane);
    spawn("flashpaste-dispatch", cmd)
}

// ─────────────────────────────────────────────────────────────────────────
// Spawning helper
// ─────────────────────────────────────────────────────────────────────────

/// Spawn a child with inherited stdio (the default for `Command`) and
/// wait. Stdio inheritance is what makes `flashpaste doctor` look
/// indistinguishable from running `flashpaste-doctor` directly — colors,
/// TTY detection, prompts, all flow through.
fn spawn(name: &str, mut cmd: Command) -> Result<ExitStatus> {
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `{name}` — is it on $PATH?"))?;
    let status = child
        .wait()
        .with_context(|| format!("waiting for `{name}`"))?;
    Ok(status)
}
