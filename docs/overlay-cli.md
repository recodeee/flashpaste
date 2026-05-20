# FlashPaste Overlay CLI

`flashpaste-overlay` is the human and shell scripting client for
`flashpaste-overlayd`. It builds one protocol message, connects to
`$XDG_RUNTIME_DIR/flashpaste-overlay.sock` or `/tmp/flashpaste-overlay.sock`
when `XDG_RUNTIME_DIR` is unset, sends the JSON line, prints the daemon response,
and exits `0` only when the response has `{"ok":true,...}`.

The CLI currently generates draw annotation ids automatically. Use the JSON
response id if a later script needs to clear the annotation.

## Draw Options

These options are shared by `rect`, `circle`, `arrow`, and `label`:

| Option | Default | Notes |
| --- | --- | --- |
| `--color #rrggbb` or `--color #rrggbbaa` | `#ffae00` | Shell-quote colors so `#` is not treated as a comment. |
| `--ttl-ms MS` | `3000` | Must be `<= 30000`. |

The client sends protocol `stroke_width` as the default `2.0`; the current CLI
does not expose a stroke-width flag.

## `rect`

Draw a rectangle.

```bash
flashpaste-overlay rect --x 420 --y 220 --w 260 --h 120 --color '#ffae00' --ttl-ms 5000
```

## `circle`

Draw an ellipse bounded by `x`, `y`, `w`, and `h`.

```bash
flashpaste-overlay circle --x 840 --y 260 --w 180 --h 180 --color '#00aaffcc'
```

## `arrow`

Draw an arrow from `(x1, y1)` to `(x2, y2)`.

```bash
flashpaste-overlay arrow --x1 300 --y1 180 --x2 560 --y2 360 --color '#ff3b30' --ttl-ms 8000
```

## `label`

Draw a text label. `--text` is capped at 200 characters by the protocol and
client.

```bash
flashpaste-overlay label --x 520 --y 420 --text 'click here next' --color '#ffffff'
```

## `clear`

Clear all annotations:

```bash
flashpaste-overlay clear
```

Clear one annotation by response id:

```bash
flashpaste-overlay clear --id 018f4c7d-7f2e-7c80-8e6f-b5c1eb1d2d0f
```

## Why Not Gromit-MPX Or Wayscriber

[Gromit-MPX](https://github.com/bk138/gromit-mpx) and
[wayscriber](https://github.com/devmobasa/wayscriber) are strong human-driven
screen annotation tools. FlashPaste credits them as reference projects for
overlay behavior, compositor constraints, and annotation UX.

The design difference is the control surface. Gromit-MPX and wayscriber are
interactive applications: a person toggles tools, hotkeys, trays, toolbars,
devices, boards, presets, and drawing modes. `flashpaste-overlayd` is a tiny
programmatic endpoint: agents, MCP tools, tests, and shell scripts send a
bounded JSON command such as "draw this arrow for 5 seconds" and receive a
machine-readable response. It is not trying to replace a presenter drawing app;
it is the automation layer FlashPaste needs for precise agent callouts.

## License Notes

FlashPaste is MIT licensed; see [`../LICENSE`](../LICENSE). Gromit-MPX and
wayscriber retain their own licenses and copyrights. FlashPaste does not vendor
their code. Overlay dependency and reference-project notices are listed in
[`../THIRD_PARTY.md`](../THIRD_PARTY.md).
