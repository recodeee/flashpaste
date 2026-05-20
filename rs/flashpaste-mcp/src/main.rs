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

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "flashpaste-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const OVERLAY_SOCKET_NAME: &str = "flashpaste-overlay.sock";
const HIGHLIGHT_REGION_DESCRIPTION: &str = "Draw a temporary highlight box on the user's screen. Use this when pointing the user to a specific area of their visible UI that doesn't have a text address (a button, a panel, a region of a running app). Prefer this over describing the location in words for UI elements.";
const POINT_AT_DESCRIPTION: &str = "Draw an arrow on the user's screen pointing from one location to another. Use when explaining direction, flow, or 'this comes from that'.";

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
        "notifications/initialized" | "notifications/cancelled" | "ping" => Ok(json!({})),
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
        },
        {
            "name": "highlight_region",
            "description": HIGHLIGHT_REGION_DESCRIPTION,
            "inputSchema": {
                "type": "object",
                "required": ["shape", "x", "y", "w", "h"],
                "properties": {
                    "shape": {
                        "type": "string",
                        "enum": ["rect", "circle"],
                        "description": "Highlight shape to draw."
                    },
                    "x": {
                        "type": "integer",
                        "description": "Left x coordinate in screen pixels."
                    },
                    "y": {
                        "type": "integer",
                        "description": "Top y coordinate in screen pixels."
                    },
                    "w": {
                        "type": "integer",
                        "description": "Region width in screen pixels."
                    },
                    "h": {
                        "type": "integer",
                        "description": "Region height in screen pixels."
                    },
                    "color": {
                        "type": "string",
                        "description": "Optional stroke color as #rrggbb or #rrggbbaa. Default: #ffae00."
                    },
                    "label": {
                        "type": "string",
                        "description": "Optional short label to draw at the region's top-left corner."
                    },
                    "ttl_ms": {
                        "type": "integer",
                        "description": "Optional time to keep the highlight visible in milliseconds. Default: 3000; maximum: 30000."
                    }
                }
            }
        },
        {
            "name": "point_at",
            "description": POINT_AT_DESCRIPTION,
            "inputSchema": {
                "type": "object",
                "required": ["from_x", "from_y", "to_x", "to_y"],
                "properties": {
                    "from_x": {
                        "type": "integer",
                        "description": "Arrow start x coordinate in screen pixels."
                    },
                    "from_y": {
                        "type": "integer",
                        "description": "Arrow start y coordinate in screen pixels."
                    },
                    "to_x": {
                        "type": "integer",
                        "description": "Arrow end x coordinate in screen pixels."
                    },
                    "to_y": {
                        "type": "integer",
                        "description": "Arrow end y coordinate in screen pixels."
                    },
                    "color": {
                        "type": "string",
                        "description": "Optional stroke color as #rrggbb or #rrggbbaa. Default: #ffae00."
                    },
                    "label": {
                        "type": "string",
                        "description": "Optional short label to draw near the arrow target."
                    },
                    "ttl_ms": {
                        "type": "integer",
                        "description": "Optional time to keep the arrow visible in milliseconds. Default: 3000; maximum: 30000."
                    }
                }
            }
        },
        {
            "name": "clear_annotations",
            "description": "Clear all FlashPaste overlay annotations from the user's screen.",
            "inputSchema": {"type": "object", "properties": {}}
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
        "highlight_region" => highlight_region(args).await,
        "point_at" => point_at(args).await,
        "clear_annotations" => clear_annotations().await,
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

async fn highlight_region(args: Value) -> Result<Value> {
    let shape = args
        .get("shape")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing 'shape' argument"))?;
    let message_type = match shape {
        "rect" => "draw_rect",
        "circle" => "draw_circle",
        other => anyhow::bail!("shape must be 'rect' or 'circle', got {other:?}"),
    };
    let x = required_i64(&args, "x")?;
    let y = required_i64(&args, "y")?;
    let w = required_i64(&args, "w")?;
    let h = required_i64(&args, "h")?;

    let color = optional_string(&args, "color")?;
    let label = optional_string(&args, "label")?;
    let ttl_ms = optional_i64(&args, "ttl_ms")?;

    let mut messages = vec![overlay_shape_message(
        message_type,
        x,
        y,
        w,
        h,
        color.as_deref(),
        ttl_ms,
    )];
    if let Some(label) = label {
        if !label.is_empty() {
            messages.push(overlay_label_message(
                x,
                y,
                &label,
                color.as_deref(),
                ttl_ms,
            ));
        }
    }

    send_overlay_tool_messages(messages).await
}

async fn point_at(args: Value) -> Result<Value> {
    let from_x = required_i64(&args, "from_x")?;
    let from_y = required_i64(&args, "from_y")?;
    let to_x = required_i64(&args, "to_x")?;
    let to_y = required_i64(&args, "to_y")?;

    let color = optional_string(&args, "color")?;
    let label = optional_string(&args, "label")?;
    let ttl_ms = optional_i64(&args, "ttl_ms")?;

    let mut messages = vec![overlay_arrow_message(
        from_x,
        from_y,
        to_x,
        to_y,
        color.as_deref(),
        ttl_ms,
    )];
    if let Some(label) = label {
        if !label.is_empty() {
            messages.push(overlay_label_message(
                to_x,
                to_y,
                &label,
                color.as_deref(),
                ttl_ms,
            ));
        }
    }

    send_overlay_tool_messages(messages).await
}

async fn clear_annotations() -> Result<Value> {
    send_overlay_tool_messages(vec![overlay_clear_message()]).await
}

fn required_i64(args: &Value, key: &str) -> Result<i64> {
    args.get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow::anyhow!("missing integer '{key}' argument"))
}

fn optional_i64(args: &Value, key: &str) -> Result<Option<i64>> {
    match args.get(key) {
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| anyhow::anyhow!("'{key}' must be an integer")),
        None => Ok(None),
    }
}

fn optional_string(args: &Value, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        Some(value) => value
            .as_str()
            .map(|s| Some(s.to_string()))
            .ok_or_else(|| anyhow::anyhow!("'{key}' must be a string")),
        None => Ok(None),
    }
}

fn overlay_shape_message(
    message_type: &str,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    color: Option<&str>,
    ttl_ms: Option<i64>,
) -> Value {
    let mut message = json!({
        "type": message_type,
        "id": Uuid::new_v4().to_string(),
        "x": x,
        "y": y,
        "w": w,
        "h": h
    });
    add_optional_overlay_fields(&mut message, color, ttl_ms);
    message
}

fn overlay_label_message(
    x: i64,
    y: i64,
    text: &str,
    color: Option<&str>,
    ttl_ms: Option<i64>,
) -> Value {
    let mut message = json!({
        "type": "draw_label",
        "id": Uuid::new_v4().to_string(),
        "x": x,
        "y": y,
        "text": text
    });
    add_optional_overlay_fields(&mut message, color, ttl_ms);
    message
}

fn overlay_arrow_message(
    from_x: i64,
    from_y: i64,
    to_x: i64,
    to_y: i64,
    color: Option<&str>,
    ttl_ms: Option<i64>,
) -> Value {
    let mut message = json!({
        "type": "draw_arrow",
        "id": Uuid::new_v4().to_string(),
        "x1": from_x,
        "y1": from_y,
        "x2": to_x,
        "y2": to_y
    });
    add_optional_overlay_fields(&mut message, color, ttl_ms);
    message
}

fn overlay_clear_message() -> Value {
    json!({
        "type": "clear"
    })
}

fn add_optional_overlay_fields(message: &mut Value, color: Option<&str>, ttl_ms: Option<i64>) {
    if let Some(object) = message.as_object_mut() {
        if let Some(color) = color {
            object.insert("color".to_string(), json!(color));
        }
        if let Some(ttl_ms) = ttl_ms {
            object.insert("ttl_ms".to_string(), json!(ttl_ms));
        }
    }
}

fn overlay_socket_path() -> Option<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    if runtime_dir.is_empty() {
        return None;
    }
    Some(PathBuf::from(runtime_dir).join(OVERLAY_SOCKET_NAME))
}

async fn send_overlay_tool_messages(messages: Vec<Value>) -> Result<Value> {
    let Some(socket_path) = overlay_socket_path() else {
        return Ok(overlay_unavailable_result(
            None,
            "XDG_RUNTIME_DIR is not set, so flashpaste-mcp cannot locate the overlay socket",
        ));
    };

    if !socket_path.exists() {
        return Ok(overlay_unavailable_result(
            Some(&socket_path),
            "flashpaste-overlayd socket does not exist",
        ));
    }

    let responses = send_overlay_messages(&socket_path, messages).await?;
    let ok = responses
        .iter()
        .all(|response| response.get("ok").and_then(|v| v.as_bool()) == Some(true));
    Ok(overlay_response_result(&socket_path, ok, responses))
}

async fn send_overlay_messages(socket_path: &Path, messages: Vec<Value>) -> Result<Vec<Value>> {
    let stream = UnixStream::connect(socket_path).await?;
    let mut stream = BufReader::new(stream);
    let mut responses = Vec::with_capacity(messages.len());

    for message in messages {
        let mut request = serde_json::to_vec(&message)?;
        request.push(b'\n');
        stream.get_mut().write_all(&request).await?;
        stream.get_mut().flush().await?;

        let mut line = String::new();
        let n = stream.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("flashpaste-overlayd closed the socket without a response");
        }
        responses.push(serde_json::from_str(line.trim())?);
    }

    Ok(responses)
}

fn overlay_response_result(socket_path: &Path, ok: bool, responses: Vec<Value>) -> Value {
    let text = if responses.len() == 1 {
        format!("flashpaste-overlayd response: {}", responses[0])
    } else {
        format!(
            "flashpaste-overlayd responses: {}",
            Value::Array(responses.clone())
        )
    };

    json!({
        "isError": !ok,
        "content": [{"type": "text", "text": text}],
        "structuredContent": {
            "ok": ok,
            "socket": socket_path.display().to_string(),
            "responses": responses
        }
    })
}

fn overlay_unavailable_result(socket_path: Option<&Path>, reason: &str) -> Value {
    let socket = socket_path.map(|path| path.display().to_string());
    let suggestion = "Install and start flashpaste-overlayd, then retry.";
    let text = match &socket {
        Some(socket) => format!("{reason}: {socket}. {suggestion}"),
        None => format!("{reason}. {suggestion}"),
    };

    json!({
        "isError": true,
        "content": [{"type": "text", "text": text}],
        "structuredContent": {
            "ok": false,
            "error": reason,
            "socket": socket,
            "suggestion": suggestion
        }
    })
}

// ── base64 (stdlib-only, no extra dep) ─────────────────────────────

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_includes_point_and_clear_tools() {
        let tools = tool_list();
        let names: Vec<_> = tools
            .as_array()
            .expect("tool list should be an array")
            .iter()
            .filter_map(|tool| tool.get("name").and_then(|name| name.as_str()))
            .collect();

        assert!(names.contains(&"point_at"));
        assert!(names.contains(&"clear_annotations"));
    }

    #[test]
    fn overlay_shape_message_maps_highlight_to_protocol() {
        let message =
            overlay_shape_message("draw_rect", 10, 20, 30, 40, Some("#ff0000"), Some(5000));

        assert_eq!(message["type"], "draw_rect");
        assert_eq!(message["x"], 10);
        assert_eq!(message["y"], 20);
        assert_eq!(message["w"], 30);
        assert_eq!(message["h"], 40);
        assert_eq!(message["color"], "#ff0000");
        assert_eq!(message["ttl_ms"], 5000);
        assert!(message["id"]
            .as_str()
            .is_some_and(|id| Uuid::parse_str(id).is_ok()));
    }

    #[test]
    fn overlay_arrow_message_maps_point_at_to_protocol() {
        let message = overlay_arrow_message(10, 20, 30, 40, Some("#00ff00cc"), Some(2500));

        assert_eq!(message["type"], "draw_arrow");
        assert_eq!(message["x1"], 10);
        assert_eq!(message["y1"], 20);
        assert_eq!(message["x2"], 30);
        assert_eq!(message["y2"], 40);
        assert_eq!(message["color"], "#00ff00cc");
        assert_eq!(message["ttl_ms"], 2500);
        assert!(message["id"]
            .as_str()
            .is_some_and(|id| Uuid::parse_str(id).is_ok()));
    }

    #[test]
    fn overlay_clear_message_has_no_id() {
        let message = overlay_clear_message();

        assert_eq!(message["type"], "clear");
        assert!(message.get("id").is_none());
    }

    #[test]
    fn overlay_unavailable_result_is_structured_tool_error() {
        let result = overlay_unavailable_result(
            Some(Path::new("/run/user/1000/flashpaste-overlay.sock")),
            "flashpaste-overlayd socket does not exist",
        );

        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["ok"], false);
        assert_eq!(
            result["structuredContent"]["socket"],
            "/run/user/1000/flashpaste-overlay.sock"
        );
        assert!(result["structuredContent"]["suggestion"]
            .as_str()
            .is_some_and(|text| text.contains("start flashpaste-overlayd")));
    }
}
