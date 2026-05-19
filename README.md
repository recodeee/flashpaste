<div align="center">

<img src="assets/logo.svg" alt="FlashPaste logo" width="120">

# FlashPaste

**Paste screenshots into Claude Code (and other terminal AI agents) on GNOME Wayland.**

`PrtScr` → right-click → **Paste**. Image attached in under 20 ms.

<a href="LICENSE"><img alt="MIT" src="https://img.shields.io/badge/license-MIT-blue.svg?style=flat-square"></a>
<img alt="Stack" src="https://img.shields.io/badge/stack-GNOME%20Wayland%20%C2%B7%20kitty%20%C2%B7%20tmux-79c0ff?style=flat-square">
<a href="https://github.com/NagyVikt/flashpaste/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/NagyVikt/flashpaste?style=flat-square&color=1f6feb"></a>

<br>

<img src="assets/hero-problem-solution.png" alt="Before FlashPaste: chaotic terminal, dock full of phantom icons, paste pressed 5+ times. After FlashPaste: one right-click, image cleanly attached." width="100%">

</div>

---

## The problem

On **GNOME Wayland + kitty + tmux**, pasting a screenshot into a terminal AI like Claude Code is broken in five compounding ways:

- **mutter** refuses to give clipboard data to background tmux panes — `wl-paste -t image/png` returns 0 bytes.
- **kitty's** `map ctrl+v` eats the keystroke before the inner TUI ever sees it.
- **tmux's** `bind -n C-v` recurses on synthesized paste and loses the byte.
- **GNOME Screenshot** *saves* a PNG but never *copies* it to the clipboard.
- Every `wl-paste` fork shows up as a phantom "Unknown" icon in the Ubuntu Dock.

End result: you press paste 5–15 times, the dock fills with gear icons, mutter wedges, you give up.

## The solution

FlashPaste glues the five layers together so the paste *just works*:

- A systemd `.path` unit preloads each new `PrtScr` PNG into the clipboard the instant the file lands.
- A small Rust daemon claims the Wayland selection once with a stable `app_id` (no more dock spam).
- The tmux/kitty binding unbinds `C-v`, sends `\026` over `kitty @ send-text`, then rebinds — bypassing every interception.

<div align="center">
<img src="assets/paste-closeup.png" alt="PrtScr keycap → image snapping into a terminal chat input in ~15 ms" width="80%">
</div>

`PrtScr` → right-click → **Paste**. ~15 ms with the daemon, ~120 ms on the bash fallback. Done.

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/NagyVikt/flashpaste/main/bootstrap.sh | bash
```

Then append the snippets, reload, and you're done:

```bash
cat ~/.local/share/flashpaste/examples/tmux.conf.snippet  >> ~/.tmux.conf
cat ~/.local/share/flashpaste/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf
tmux source-file ~/.tmux.conf      # restart kitty too
flashpaste-doctor                  # 13 green checks = ready
```

Debian / Ubuntu also has a `.deb`:

```bash
curl -fsSL -o /tmp/flashpaste.deb \
  https://github.com/NagyVikt/flashpaste/releases/latest/download/flashpaste_all.deb
sudo apt install /tmp/flashpaste.deb
```

Requirements: `wl-clipboard xclip xsel ydotool ydotoold tmux kitty`.

## Verify

1. Open kitty → tmux → Claude Code (or Codex, Aider, …).
2. Press **PrtScr**.
3. Right-click in the pane → **Paste**.

The image attaches. If it doesn't, `flashpaste-doctor` tells you which probe is red.

---

## More

- [`docs/`](docs/README.md) — install guide, architecture, FAQ, troubleshooting, ADRs.
- [`CHANGELOG.md`](CHANGELOG.md) — release history.
- [`AGENTS.md`](AGENTS.md) — contributor + AI-agent guide (release workflow lives here).
- [`flashpaste-mcp`](docs/architecture.md) — MCP server so agents can take screenshots, read your clipboard, and paste into other tmux panes.

MIT — see [LICENSE](LICENSE).
