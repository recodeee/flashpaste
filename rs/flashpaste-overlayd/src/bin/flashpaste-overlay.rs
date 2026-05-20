use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use flashpaste_overlayd::{
    protocol::{
        Color, DrawArrow, DrawCircle, DrawLabel, DrawRect, DrawStyle, Message,
        DEFAULT_STROKE_WIDTH, DEFAULT_TTL_MS, MAX_LABEL_CHARS, MAX_TTL_MS,
    },
    socket_path,
};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(
    name = "flashpaste-overlay",
    version,
    about = "Send drawing commands to flashpaste-overlayd"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Draw a rectangle.
    Rect(RectArgs),
    /// Draw an ellipse bounded by x/y/w/h.
    Circle(RectArgs),
    /// Draw an arrow from one point to another.
    Arrow(ArrowArgs),
    /// Draw a text label.
    Label(LabelArgs),
    /// Clear one annotation by id or all annotations.
    Clear(ClearArgs),
    /// Hidden integration-test command: query daemon store state.
    #[command(hide = true)]
    Status,
    /// Hidden integration-test command: send an arbitrary JSON line.
    #[command(hide = true)]
    Raw(RawArgs),
    /// Hidden integration-test command: send many rect messages on one socket.
    #[command(hide = true)]
    Flood(FloodArgs),
}

#[derive(Debug, Args)]
struct RectArgs {
    #[arg(long)]
    x: f64,
    #[arg(long)]
    y: f64,
    #[arg(long)]
    w: f64,
    #[arg(long)]
    h: f64,
    #[command(flatten)]
    draw: DrawOptions,
}

#[derive(Debug, Args)]
struct ArrowArgs {
    #[arg(long)]
    x1: f64,
    #[arg(long)]
    y1: f64,
    #[arg(long)]
    x2: f64,
    #[arg(long)]
    y2: f64,
    #[command(flatten)]
    draw: DrawOptions,
}

#[derive(Debug, Args)]
struct LabelArgs {
    #[arg(long)]
    x: f64,
    #[arg(long)]
    y: f64,
    #[arg(long)]
    text: String,
    #[command(flatten)]
    draw: DrawOptions,
}

#[derive(Debug, Args)]
struct ClearArgs {
    #[arg(long)]
    id: Option<Uuid>,
}

#[derive(Debug, Args)]
struct RawArgs {
    #[arg(long)]
    payload: String,
}

#[derive(Debug, Args)]
struct FloodArgs {
    #[arg(long, default_value_t = 1_000)]
    count: usize,
}

#[derive(Clone, Copy, Debug, Args)]
struct DrawOptions {
    /// Annotation color as #rrggbb or #rrggbbaa.
    #[arg(long)]
    color: Option<Color>,
    /// Annotation time to live in milliseconds.
    #[arg(long, value_name = "MS")]
    ttl_ms: Option<u32>,
}

fn main() -> ExitCode {
    match run() {
        Ok(ok) if ok => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("flashpaste-overlay: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool> {
    let cli = Cli::parse();
    let response = match cli.command {
        Command::Status => send_payload(br#"{"type":"debug_store"}"#)?,
        Command::Raw(args) => send_payload(args.payload.as_bytes())?,
        Command::Flood(args) => return run_flood(args),
        command => send_message(&command.into_message()?)?,
    };

    print!("{response}");
    if !response.ends_with('\n') {
        println!();
    }

    Ok(response_is_ok(&response))
}

impl Command {
    fn into_message(self) -> Result<Message> {
        match self {
            Self::Rect(args) => Ok(Message::DrawRect(DrawRect {
                style: args.draw.style()?,
                x: args.x,
                y: args.y,
                w: args.w,
                h: args.h,
            })),
            Self::Circle(args) => Ok(Message::DrawCircle(DrawCircle {
                style: args.draw.style()?,
                x: args.x,
                y: args.y,
                w: args.w,
                h: args.h,
            })),
            Self::Arrow(args) => Ok(Message::DrawArrow(DrawArrow {
                style: args.draw.style()?,
                x1: args.x1,
                y1: args.y1,
                x2: args.x2,
                y2: args.y2,
            })),
            Self::Label(args) => {
                if args.text.chars().count() > MAX_LABEL_CHARS {
                    bail!("--text must be {MAX_LABEL_CHARS} characters or fewer");
                }

                Ok(Message::DrawLabel(DrawLabel {
                    style: args.draw.style()?,
                    x: args.x,
                    y: args.y,
                    text: args.text,
                }))
            }
            Self::Clear(args) => Ok(Message::Clear(flashpaste_overlayd::protocol::Clear {
                id: args.id,
            })),
            Self::Status | Self::Raw(_) | Self::Flood(_) => {
                unreachable!("hidden test commands are handled before message conversion")
            }
        }
    }
}

impl DrawOptions {
    fn style(self) -> Result<DrawStyle> {
        let ttl_ms = self.ttl_ms.unwrap_or(DEFAULT_TTL_MS);
        if ttl_ms > MAX_TTL_MS {
            bail!("--ttl-ms must be <= {MAX_TTL_MS}");
        }

        Ok(DrawStyle {
            id: Uuid::new_v4(),
            ttl_ms,
            color: self.color.unwrap_or_default(),
            stroke_width: DEFAULT_STROKE_WIDTH,
            current_opacity: flashpaste_overlayd::protocol::DEFAULT_CURRENT_OPACITY,
        })
    }
}

fn send_message(message: &Message) -> Result<String> {
    let payload = serde_json::to_vec(message).context("serialize request")?;
    send_payload(&payload)
}

fn send_payload(payload: &[u8]) -> Result<String> {
    let path = socket_path();
    let mut stream =
        UnixStream::connect(&path).with_context(|| format!("connect {}", path.display()))?;
    let mut payload = payload.to_vec();
    payload.push(b'\n');
    stream.write_all(&payload).context("write request")?;
    stream.flush().context("flush request")?;

    let mut response = String::new();
    let bytes = BufReader::new(stream)
        .read_line(&mut response)
        .context("read response")?;
    if bytes == 0 {
        bail!("daemon closed the socket without a response");
    }

    Ok(response)
}

fn run_flood(args: FloodArgs) -> Result<bool> {
    let path = socket_path();
    let mut stream =
        UnixStream::connect(&path).with_context(|| format!("connect {}", path.display()))?;
    let reader_stream = stream.try_clone().context("clone flood socket")?;
    let mut reader = BufReader::new(reader_stream);
    let mut response = String::new();
    let mut processed = 0usize;

    for index in 0..args.count {
        let message = Message::DrawRect(DrawRect {
            style: DrawStyle {
                id: Uuid::new_v4(),
                ttl_ms: DEFAULT_TTL_MS,
                color: Color::default(),
                stroke_width: DEFAULT_STROKE_WIDTH,
                current_opacity: flashpaste_overlayd::protocol::DEFAULT_CURRENT_OPACITY,
            },
            x: index as f64,
            y: 1.0,
            w: 2.0,
            h: 3.0,
        });
        serde_json::to_writer(&mut stream, &message).context("serialize flood request")?;
        stream.write_all(b"\n").context("write flood request")?;
        stream.flush().context("flush flood request")?;

        response.clear();
        let bytes = reader
            .read_line(&mut response)
            .context("read flood response")?;
        if bytes == 0 {
            bail!("daemon closed the socket after {processed} flood responses");
        }
        let value: serde_json::Value =
            serde_json::from_str(&response).context("parse flood response")?;
        if value.get("ok").and_then(|ok| ok.as_bool()) != Some(true) {
            bail!("flood request failed: {value}");
        }
        processed += 1;
    }

    println!("{}", serde_json::json!({ "ok": true, "count": processed }));
    Ok(true)
}

fn response_is_ok(response: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(response)
        .ok()
        .and_then(|value| value.get("ok").and_then(|ok| ok.as_bool()))
        .unwrap_or(false)
}
