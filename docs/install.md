---
title: How to install FlashPaste on Ubuntu, Debian, Fedora, and Pop!_OS
description: Complete installation guide for FlashPaste on GNOME Wayland. Covers the Debian / Ubuntu .deb package, the curl-bootstrap one-liner, source builds, per-distro notes, post-install activation, the flashpaste-doctor pre-flight, and verification with kitty and tmux.
keywords:
  - install flashpaste
  - flashpaste ubuntu
  - flashpaste debian
  - flashpaste fedora
  - flashpaste pop os
  - image paste claude code linux install
  - kitty tmux image paste install
last_updated: 2026-05-19
canonical: https://github.com/NagyVikt/flashpaste/blob/main/docs/install.md
---

# Install FlashPaste

This page covers every supported install path in depth. The 30-second version lives in the [project README](../README.md#install). Use this page when something didn't work, when you're on a less-common distro, or when you want to know exactly what the installer is doing.

## Pre-flight requirements

FlashPaste is only useful on the **GNOME Wayland + kitty + tmux** stack. If you tick all three, you're in:

- **GNOME on Wayland** (mutter compositor) — Ubuntu 24.04+, Fedora 40+, Debian 13, Pop!_OS 24.04+
- **[kitty](https://sw.kovidgoyal.net/kitty/)** terminal with `allow_remote_control yes`
- **[tmux](https://github.com/tmux/tmux)** running inside kitty
- A **terminal LLM agent**: Claude Code, Codex CLI, Aider, or another TUI with a known image-attach command

System dependencies:

```bash
# Debian / Ubuntu / Pop!_OS
sudo apt install wl-clipboard xclip xsel ydotool ydotoold tmux kitty

# Fedora
sudo dnf install wl-clipboard xclip xsel ydotool tmux kitty

# Arch (community + AUR)
sudo pacman -S wl-clipboard xclip xsel ydotool tmux kitty
```

## Option A — Debian / Ubuntu `.deb` (recommended)

```bash
curl -fsSL -o /tmp/flashpaste.deb \
  https://github.com/NagyVikt/flashpaste/releases/latest/download/flashpaste_all.deb
sudo apt install /tmp/flashpaste.deb
```

Per-user activation (one time):

```bash
systemctl --user daemon-reload
systemctl --user enable --now flashpasted.service
systemctl --user enable --now clipboard-janitor.service
systemctl --user enable --now flashpaste-screenshot-watcher.path
cat /usr/share/flashpaste/examples/tmux.conf.snippet  >> ~/.tmux.conf
cat /usr/share/flashpaste/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf
ln -sf /usr/share/flashpaste/paste_image.sh ~/paste_image.sh
flashpaste-doctor
```

Then reload tmux (`tmux source-file ~/.tmux.conf`) and restart kitty.

### Building the `.deb` yourself

```bash
git clone https://github.com/NagyVikt/flashpaste.git
cd flashpaste
make deb                                  # → dist/flashpaste_*_all.deb
sudo apt install ./dist/flashpaste_*_all.deb
```

The `make deb` target auto-includes the Rust binaries if `rs/target/release/*` already exists. Build the Rust workspace first (`cargo build --release --locked --manifest-path rs/Cargo.toml`) to get a `.deb` with Tier 2 and Tier 3 bundled.

To include the agent overlay daemon and `flashpaste-overlay` client in a local `.deb`, install the Cairo/Pango/GLib development headers and build the overlay package with the Wayland renderer before `make deb`:

```bash
sudo apt install pkg-config libcairo2-dev libglib2.0-dev libpango1.0-dev
cargo build --release --locked --manifest-path rs/Cargo.toml -p flashpaste-overlayd --features wayland
make deb
```

## Option B — One-line dotfile install (no apt, no root)

```bash
curl -fsSL https://raw.githubusercontent.com/NagyVikt/flashpaste/main/bootstrap.sh | bash
```

The bootstrap script clones to `$FLASHPASTE_DIR` (default `~/.local/share/flashpaste`), runs `flashpaste-doctor` for a 17-check pre-flight, then `install.sh` to symlink scripts into `~/.local/bin/` and drop systemd user units. No root required; nothing under `/etc/` or `/usr/` is touched.

Override the install location:

```bash
FLASHPASTE_DIR=$HOME/code/flashpaste bash bootstrap.sh
```

### Cautious variant — clone first, install second

```bash
git clone https://github.com/NagyVikt/flashpaste.git ~/.local/share/flashpaste
cd ~/.local/share/flashpaste
./bin/flashpaste-doctor.sh    # pre-flight only — touches nothing
./install.sh                  # symlinks + systemd units
```

## Option C — Build the Rust tiers from source

The bash hot path (Tier 1) is always installed. Tier 2 (`flashpaste-dispatch`) and Tier 3 (`flashpasted` + `flashpaste-trigger`) require Rust:

```bash
cd ~/.local/share/flashpaste/rs
cargo build --release --locked
install -m 0755 target/release/flashpaste-{dispatch,trigger,shoot} \
                target/release/flashpasted \
                ~/.local/bin/

# Enable the daemon (Tier 3)
cat > ~/.config/systemd/user/flashpasted.service <<'EOF'
[Unit]
Description=flashpaste daemon (clipboard owner + paste dispatcher)
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=%h/.local/bin/flashpasted
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload
systemctl --user enable --now flashpasted.service
```

The tmux + kitty snippets in `examples/` already invoke `flashpaste-trigger` with a fallback to the bash dispatcher — once the daemon is up, Tier 3 takes over automatically with no dotfile edits required.

> **Note:** `cargo build` hits crates.io. Run it interactively when you trust the workspace, not as part of an unattended install.

## The `ydotoold` socket-path drop-in (mandatory on Ubuntu 24.04)

Ubuntu 24.04 ships `ydotool 0.1.8`, which ignores `--socket-path` and always opens `/tmp/.ydotool_socket`. FlashPaste assumes `$XDG_RUNTIME_DIR/.ydotool_socket`. The `.deb` installs a systemd drop-in; the bootstrap installer creates one too. To do it manually:

```ini
# ~/.config/systemd/user/ydotoold.service.d/socket.conf
[Service]
ExecStartPost=ln -sf /tmp/.ydotool_socket %t/.ydotool_socket
ExecStopPost=rm -f %t/.ydotool_socket
```

Then:

```bash
systemctl --user daemon-reload
systemctl --user restart ydotoold.service
```

## Verify the install

```bash
flashpaste-doctor       # 17 core environment checks — all should be green
```

Smoke test:

1. Open kitty, attach to a tmux session, run your terminal AI agent inside it.
2. Press **PrtScr**. GNOME drops a PNG into `~/Pictures/Screenshots/`.
3. Right-click in the tmux pane → **Paste**.
4. The image attaches in ~120 ms (Tier 1) or ~15 ms (Tier 3).

If something is off, see [troubleshooting.md](troubleshooting.md).

## Per-distro notes

### Ubuntu 24.04 LTS (Noble Numbat)

The reference target. The `.deb` is built and tested against Noble. The `ydotool` socket-path drop-in is required (see above) and is bundled in the `.deb`.

### Debian 13 (Trixie)

Same as Ubuntu. The `.deb` installs cleanly via `sudo apt install ./flashpaste_all.deb`.

### Fedora 40+

No `.deb`, so use the bootstrap installer. `ydotool` is recent enough that the socket-path drop-in is not required, but installing it is a no-op.

### Pop!_OS 24.04+

Identical to Ubuntu — the `.deb` installs cleanly.

### Arch / Manjaro

Use the bootstrap installer. Optional AUR packaging is on the [roadmap](../ROADMAP.md).

### NixOS

Not officially supported. The Rust binaries are static enough to drop into a flake; the bash dispatchers depend on `bash`, `xclip`, `wl-clipboard`, `kitty`, and `tmux` being on `PATH`.

## Uninstall

```bash
# Dotfile install
cd ~/.local/share/flashpaste && make uninstall

# .deb install
sudo apt remove flashpaste
```

Your dotfile snippets stay where they are — remove them by hand from `~/.tmux.conf` and `~/.config/kitty/kitty.conf` if you want a clean slate.
