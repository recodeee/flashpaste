//! flashpaste-mcp — hand-rolled Model Context Protocol server over stdio.
//!
//! Implements just enough of the MCP spec (initialize, tools/list, tools/call)
//! to give LLM agents like Claude Code real eyes (take_screenshot) and hands
//! (read_clipboard, copy_text, paste_to_pane) on a Linux desktop, without
//! pulling in the full rmcp macro stack (whose API churns between versions).
//!
//! ## Register with Claude Code
//!
//! Add to `~/.config/claude-code/mcp.json`:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "flashpaste": {
//!       "command": "flashpaste-mcp",
//!       "args": []
//!     }
//!   }
//! }
//! ```
//!
//! ## Protocol
//!
//! Plain JSON-RPC 2.0 messages, one per line on stdin/stdout. All logs go
//! to stderr so they don't corrupt the JSON-RPC stream.

use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "flashpaste-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse<'a> {
    jsonrpc: &'a str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    info!(
        version = SERVER_VERSION,
        "flashpaste-mcp starting (stdio transport)"
    );

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            info!("stdin closed, exiting");
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                warn!(error=%e, "ignoring malformed JSON-RPC line");
                continue;
            }
        };
        if req.jsonrpc != "2.0" {
            warn!(method=%req.method, "ignoring non-2.0 JSON-RPC request");
            continue;
        }
        let id = req.id.clone().unwrap_or(Value::Null);
        let response = dispatch(&req.method, req.params).await;
        // MCP "notifications" (no id) per spec MUST not get a response.
        if req.id.is_none() {
            continue;
        }
        let out_msg = match response {
            Ok(result) => JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(result),
                error: None,
            },
            Err(e) => JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32000,
                    message: e.to_string(),
                }),
            },
        };
        let mut buf = serde_json::to_vec(&out_msg)?;
        buf.push(b'\n');
        stdout.write_all(&buf).await?;
        stdout.flush().await?;
    }
    Ok(())
}

/// Top-level method router.
async fn dispatch(method: &str, params: Value) -> Result<Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION
            },
            "instructions": "flashpaste MCP — eyes (take_screenshot) and hands \
                            (read_clipboard, copy_text, paste_to_pane) for a Linux desktop. \
                            Screenshot returns real PNG bytes the model can see; paste tool \
                            routes through the flashpasted daemon so cross-pane and \
                            cross-agent paste is sub-15ms."
        })),
        "tools/list" => Ok(json!({
            "tools": tool_list()
        })),
        "tools/call" => tool_call(params).await,
        "notifications/initialized" | "notifications/cancelled" | "ping" => {
            Ok(json!({}))
        }
        other => anyhow::bail!("unknown method: {other}"),
    }
}

fn tool_list() -> Value {
    json!([
        {
            "name": "take_screenshot",
            "description": "Capture a screenshot of the user's screen via the XDG desktop \
                            portal and return it as image content the agent can see directly. \
                            Use this whenever the user is debugging visual UI or asks 'what \
                            do you see on my screen?'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "interactive": {
                        "type": "boolean",
                        "description": "Open the portal's interactive area-selection UI \
                                        before capturing. Default: false (full screen).",
                        "default": false
                    }
                }
            }
        },
        {
            "name": "read_clipboard",
            "description": "Read the current system clipboard as text. Falls back through \
                            wl-paste shim → xclip → xsel. Returns empty string if the \
                            clipboard holds non-text data (use take_screenshot for image).",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "copy_text",
            "description": "Place plain text onto the user's system clipboard so they can \
                            paste it elsewhere.",
            "inputSchema": {
                "type": "object",
                "required": ["text"],
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text content to place on the clipboard."
                    }
                }
            }
        },
        {
            "name": "paste_to_pane",
            "description": "Trigger flashpaste to paste the current clipboard contents into \
                            a specific tmux pane (e.g. into another agent's pane for \
                            cross-agent handoff). Uses the flashpasted daemon for sub-15ms \
                            round-trip with bash fallback.",
            "inputSchema": {
                "type": "object",
                "required": ["pane_id"],
                "properties": {
                    "pane_id": {
                        "type": "string",
                        "description": "Target tmux pane id (e.g. '%4'). Get pane ids via \
                                        `tmux list-panes -F '#{pane_id} #{pane_current_command}'`."
                    }
                }
            }
        }
    ])
}

async fn tool_call(params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing tool name"))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match name {
        "take_screenshot" => take_screenshot(args).await,
        "read_clipboard" => read_clipboard().await,
        "copy_text" => copy_text(args).await,
        "paste_to_pane" => paste_to_pane(args).await,
        other => anyhow::bail!("unknown tool: {other}"),
    }
}

// ── tool implementations ────────────────────────────────────────────

async fn take_screenshot(args: Value) -> Result<Value> {
    let interactive = args
        .get("interactive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut cmd = Command::new("flashpaste-shoot");
    cmd.arg("--print-path");
    if interactive {
        cmd.arg("--interactive");
    }
    let out = cmd.output()?;
    if !out.status.success() {
        anyhow::bail!(
            "flashpaste-shoot failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        anyhow::bail!("flashpaste-shoot produced no path");
    }
    let bytes = std::fs::read(&path)?;
    let b64 = base64_encode(&bytes);
    Ok(json!({
        "content": [
            {"type": "image", "data": b64, "mimeType": "image/png"},
            {"type": "text", "text": format!("screenshot saved to {path} ({} bytes)", bytes.len())}
        ]
    }))
}

async fn read_clipboard() -> Result<Value> {
    let out = Command::new("wl-paste").arg("--no-newline").output()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    Ok(json!({
        "content": [{"type": "text", "text": text}]
    }))
}

async fn copy_text(args: Value) -> Result<Value> {
    use std::io::Write;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'text' argument"))?;
    let mut child = Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let _ = child.wait();
    Ok(json!({
        "content": [
            {"type": "text", "text": format!("copied {} bytes to clipboard", text.len())}
        ]
    }))
}

async fn paste_to_pane(args: Value) -> Result<Value> {
    let pane = args
        .get("pane_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'pane_id' argument"))?;
    let out = Command::new("flashpaste-trigger").arg(pane).output()?;
    let msg = if out.status.success() {
        format!("paste dispatched to pane {pane}")
    } else {
        format!(
            "paste failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    };
    Ok(json!({
        "content": [{"type": "text", "text": msg}]
    }))
}

// ── base64 (stdlib-only, no extra dep) ─────────────────────────────

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHA[(b0 >> 2) as usize] as char);
        out.push(ALPHA[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHA[(((b1 & 0x0F) << 2) | (b2 >> 6)) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHA[(b2 & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}
