# MCP Tool Patterns

Source of truth: `rs/flashpaste-mcp/src/main.rs`.

`flashpaste-mcp` is a hand-rolled MCP server over stdio. It handles plain JSON-RPC 2.0 messages, logs only to stderr, and implements `initialize`, `tools/list`, `tools/call`, `notifications/initialized`, `notifications/cancelled`, and `ping`.

## Registration

Tools are registered in `tool_list()`. The function returns a `serde_json::json!` array of tool objects, and `dispatch()` exposes that array from the `tools/list` method:

```rust
"tools/list" => Ok(json!({
    "tools": tool_list()
})),
```

Tool execution is wired separately in `tool_call(params)`. It reads `params.name`, defaults missing `params.arguments` to `{}`, and matches the tool name to an async implementation:

```rust
let args = params.get("arguments").cloned().unwrap_or(json!({}));
match name {
    "take_screenshot" => take_screenshot(args).await,
    "read_clipboard" => read_clipboard().await,
    "copy_text" => copy_text(args).await,
    "paste_to_pane" => paste_to_pane(args).await,
    other => anyhow::bail!("unknown tool: {other}"),
}
```

Adding a tool therefore requires three matching changes in the same file:

1. Add its object to `tool_list()`.
2. Add its name to the `tool_call()` match.
3. Add an async implementation returning `anyhow::Result<serde_json::Value>`.

## Existing Tools

`take_screenshot` has one optional boolean argument, `interactive`, defaulting to `false`. It runs `flashpaste-shoot --print-path`, adds `--interactive` when requested, treats a non-zero exit status or empty stdout path as an error, reads the PNG from disk, base64-encodes it with the local stdlib-only helper, and returns both image content and a text status message.

`read_clipboard` takes no arguments. It runs `wl-paste --no-newline`, converts stdout to UTF-8 lossily, and returns one text content item. The current implementation does not inspect the process exit status.

`copy_text` requires a string `text` argument. It starts `wl-copy` with piped stdin, writes the provided bytes, waits for the child, and returns one text status item with the byte count. The current implementation ignores the child exit status after waiting.

`paste_to_pane` requires a string `pane_id` argument. It runs `flashpaste-trigger <pane_id>` and always returns one text content item: a success message when the command exits successfully, or a failure message containing stderr when it does not. Unlike `take_screenshot`, command failure is represented as tool content rather than an MCP error.

## JSON Schema Shape

Each tool object in `tool_list()` uses this shape:

```json
{
  "name": "tool_name",
  "description": "Human-readable tool guidance.",
  "inputSchema": {
    "type": "object",
    "required": ["required_arg"],
    "properties": {
      "required_arg": {
        "type": "string",
        "description": "Argument description."
      }
    }
  }
}
```

Schema conventions in the existing tools:

- Use `inputSchema`, not `input_schema`.
- Use JSON Schema object shape directly inside the Rust `json!` macro.
- Always set `"type": "object"` for the tool input.
- Put arguments under `"properties"`.
- Add `"required": [...]` only for required arguments.
- Omit `"required"` for optional-only or no-argument tools.
- Use `"default"` for documented optional defaults, as `take_screenshot.interactive` does.
- Do not set `$schema`, `additionalProperties`, `oneOf`, `anyOf`, custom schema metadata, or Rust-side schema structs.
- Empty-input tools use `{"type": "object", "properties": {}}`.

## Result Shape

Tool implementations return MCP tool results as JSON values with a `content` array.

Text-only tools return:

```json
{
  "content": [
    { "type": "text", "text": "message" }
  ]
}
```

`take_screenshot` returns image content first, then a text status item:

```json
{
  "content": [
    { "type": "image", "data": "<base64 PNG>", "mimeType": "image/png" },
    { "type": "text", "text": "screenshot saved to <path> (<bytes> bytes)" }
  ]
}
```

Errors produced with `anyhow::bail!` bubble up through the JSON-RPC response as code `-32000`. Existing tools are not uniform about command failures: some bail, while `paste_to_pane` returns a text failure message. New tools should follow the nearest existing tool's behavior for the same kind of operation.
