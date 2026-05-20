# FlashPaste Overlay Protocol

`flashpaste-overlayd` uses newline-delimited JSON over a Unix-domain socket for
agent-driven screen annotations. The public wire API is implemented by
`rs/flashpaste-overlayd/src/protocol.rs`.

## Transport

Default socket path:

```text
$XDG_RUNTIME_DIR/flashpaste-overlay.sock
```

If `XDG_RUNTIME_DIR` is unset or empty, the Rust client helper falls back to:

```text
/tmp/flashpaste-overlay.sock
```

Each request is one UTF-8 JSON object followed by `\n`. Each response is one
UTF-8 JSON object followed by `\n`.

## Requests

The protocol has exactly five request message types:

- `draw_rect`
- `draw_circle`
- `draw_arrow`
- `draw_label`
- `clear`

Draw requests share these fields:

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `type` | string | required | One of `draw_rect`, `draw_circle`, `draw_arrow`, or `draw_label`. |
| `id` | UUID string | required | Caller-generated annotation id. |
| `ttl_ms` | integer | `3000` | Time to live in milliseconds. Must be `<= 30000`. |
| `color` | string | `#ffae00` | `#rrggbb` or `#rrggbbaa`. |
| `stroke_width` | number | `2.0` | Stroke width in pixels. |

`current_opacity` is internal render state. It is not accepted as a wire field
and is skipped during serialization.

### `draw_rect`

Draws a rectangle.

| Field | Type | Notes |
| --- | --- | --- |
| `x` | number | Left edge in screen pixels. |
| `y` | number | Top edge in screen pixels. |
| `w` | number | Width in pixels. |
| `h` | number | Height in pixels. |

### `draw_circle`

Draws an ellipse bounded by `x`, `y`, `w`, and `h`.

| Field | Type | Notes |
| --- | --- | --- |
| `x` | number | Left edge of the bounding box. |
| `y` | number | Top edge of the bounding box. |
| `w` | number | Bounding width in pixels. |
| `h` | number | Bounding height in pixels. |

### `draw_arrow`

Draws an arrow from `(x1, y1)` to `(x2, y2)`.

| Field | Type | Notes |
| --- | --- | --- |
| `x1` | number | Start x coordinate. |
| `y1` | number | Start y coordinate. |
| `x2` | number | End x coordinate. |
| `y2` | number | End y coordinate. |

### `draw_label`

Draws a text label.

| Field | Type | Notes |
| --- | --- | --- |
| `x` | number | Label anchor x coordinate. |
| `y` | number | Label anchor y coordinate. |
| `text` | string | Maximum `200` Unicode scalar values. |

### `clear`

Clears annotations.

| Field | Type | Default | Notes |
| --- | --- | --- | --- |
| `id` | UUID string | omitted | When present, clear one annotation. When omitted, clear all annotations. |

## Responses

Success:

```json
{"ok":true,"id":"018f4c7d-7f2e-7c80-8e6f-b5c1eb1d2d0f"}
```

Failure:

```json
{"ok":false,"error":"invalid color"}
```

For draw requests, `id` is the annotation id. For `clear` with an `id`, `id` is
the cleared annotation id. For clear-all, the success response still uses the
same success shape and should return the daemon's clear operation id.

## Examples

Draw a rectangle with default style:

```json
{"type":"draw_rect","id":"018f4c7d-7f2e-7c80-8e6f-b5c1eb1d2d0f","x":400,"y":300,"w":200,"h":100}
```

Draw a semi-transparent red arrow for five seconds:

```json
{"type":"draw_arrow","id":"018f4c7d-7f2e-7c80-8e6f-b5c1eb1d2d10","ttl_ms":5000,"color":"#ff0000cc","stroke_width":3,"x1":100,"y1":100,"x2":300,"y2":220}
```

Clear all annotations:

```json
{"type":"clear"}
```

Example responses:

```json
{"ok":true,"id":"018f4c7d-7f2e-7c80-8e6f-b5c1eb1d2d0f"}
{"ok":true,"id":"018f4c7d-7f2e-7c80-8e6f-b5c1eb1d2d10"}
{"ok":false,"error":"unknown annotation id"}
```

## License Notes

FlashPaste is MIT licensed; see [`../LICENSE`](../LICENSE). Third-party
notices for overlay dependencies and reference projects are kept in
[`../THIRD_PARTY.md`](../THIRD_PARTY.md).
