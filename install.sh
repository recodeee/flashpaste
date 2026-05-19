#!/usr/bin/env bash
# flashpaste installer — symlinks scripts into ~/.local/bin and prints
# the config snippets you need to drop into tmux + kitty.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
BIN_SRC="$REPO_DIR/bin"
BIN_DST="$HOME/.local/bin"
SYSTEMD_DST="$HOME/.config/systemd/user"

GREEN='\033[1;32m'
YELLOW='\033[1;33m'
RED='\033[1;31m'
RESET='\033[0m'

say() { printf "${GREEN}==>${RESET} %s\n" "$*"; }
warn() { printf "${YELLOW}warn:${RESET} %s\n" "$*"; }
die() { printf "${RED}error:${RESET} %s\n" "$*" >&2; exit 1; }

# ── deps ────────────────────────────────────────────────────────────
for cmd in tmux xclip wl-copy wl-paste ydotool; do
  command -v "$cmd" >/dev/null 2>&1 || warn "missing dep: $cmd (install via your package manager)"
done

# ── ~/.local/bin/ symlinks ──────────────────────────────────────────
mkdir -p "$BIN_DST"

for script in tmux-paste-dispatch.sh clipboard-set.sh clipboard-janitor.sh \
              get-clipboard-text.sh clip-pipeline-log.sh screenshot-to-clipboard \
              flashpaste-screenshot-preload.sh flashpaste-doctor.sh \
              flashpaste-trace.sh flashpaste-logs.sh; do
  src="$BIN_SRC/$script"
  # Drop the .sh suffix on the destination for the user-facing log viewer
  # so `flashpaste-logs` is what shows up on $PATH (matches the muscle
  # memory of `flashpaste-trigger`, `flashpaste-doctor`, etc.). Other
  # scripts in this loop keep their suffix because they're called
  # internally by name.
  case "$script" in
    flashpaste-logs.sh) dst="$BIN_DST/flashpaste-logs" ;;
    *)                  dst="$BIN_DST/$script"          ;;
  esac
  if [ -e "$dst" ] && [ ! -L "$dst" ]; then
    warn "$dst already exists as a real file; backing up to $dst.flashpaste-bak"
    mv "$dst" "$dst.flashpaste-bak"
  fi
  ln -sf "$src" "$dst"
  say "symlinked $script -> $src"
done

# wl-paste shim is special: replaces any existing symlink to /usr/bin/wl-paste.
WLP="$BIN_DST/wl-paste"
if [ -e "$WLP" ] && [ ! -L "$WLP" ]; then
  warn "$WLP exists as a real file; backing up to $WLP.flashpaste-bak"
  mv "$WLP" "$WLP.flashpaste-bak"
fi
ln -sf "$BIN_SRC/wl-paste" "$WLP"
say "symlinked wl-paste shim (overrides /usr/bin/wl-paste via PATH order)"

# paste_image.sh lives at $HOME (referenced by absolute path from kitty.conf).
ln -sf "$BIN_SRC/paste_image.sh" "$HOME/paste_image.sh"
say "symlinked ~/paste_image.sh -> $BIN_SRC/paste_image.sh"

# ── PATH order check ────────────────────────────────────────────────
if ! echo ":$PATH:" | grep -q ":$BIN_DST:"; then
  warn "$BIN_DST not in PATH — add this to your ~/.bashrc or ~/.zshrc:"
  echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
elif ! command -v wl-paste | grep -q "^$BIN_DST"; then
  warn "$BIN_DST is in PATH but /usr/bin precedes it — wl-paste shim won't activate."
fi

# ── desktop entries (hide phantom dock icons) ──────────────────────
APPS_DST="$HOME/.local/share/applications"
mkdir -p "$APPS_DST"
ln -sf "$REPO_DIR/share/applications/wl-clipboard.desktop" "$APPS_DST/wl-clipboard.desktop"
say "installed wl-clipboard.desktop (hides phantom dock entry)"

# ── systemd user services ──────────────────────────────────────────
mkdir -p "$SYSTEMD_DST"

cat > "$SYSTEMD_DST/clipboard-janitor.service" <<EOF
[Unit]
Description=Reap stuck wl-paste / wl-copy daemons (flashpaste)

[Service]
Type=simple
ExecStart=%h/.local/bin/clipboard-janitor.sh
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
EOF
say "wrote $SYSTEMD_DST/clipboard-janitor.service"

# Screenshot watcher — fires the preload script when ~/Pictures/Screenshots/
# changes, so xclip is hot before the user even reaches for paste.
cat > "$SYSTEMD_DST/flashpaste-screenshot-watcher.path" <<'EOF'
[Unit]
Description=Watch ~/Pictures/Screenshots/ for new PNGs (flashpaste)

[Path]
PathChanged=%h/Pictures/Screenshots
Unit=flashpaste-screenshot-watcher.service

[Install]
WantedBy=default.target
EOF
cat > "$SYSTEMD_DST/flashpaste-screenshot-watcher.service" <<'EOF'
[Unit]
Description=Pre-load fresh screenshot into xclip (flashpaste)

[Service]
Type=oneshot
ExecStart=%h/.local/bin/flashpaste-screenshot-preload.sh
EOF
say "wrote flashpaste-screenshot-watcher.{path,service}"

# ydotoold socket-path patch (Ubuntu 24.04 0.1.8 bug)
if systemctl --user list-unit-files ydotoold.service >/dev/null 2>&1; then
  cat > "$SYSTEMD_DST/ydotoold.service.d/flashpaste-socket.conf" 2>/dev/null <<EOF || true
[Service]
ExecStartPost=/bin/ln -sf /tmp/.ydotool_socket %t/.ydotool_socket
ExecStopPost=/bin/rm -f %t/.ydotool_socket
EOF
  mkdir -p "$SYSTEMD_DST/ydotoold.service.d"
  cat > "$SYSTEMD_DST/ydotoold.service.d/flashpaste-socket.conf" <<EOF
[Service]
ExecStartPost=/bin/ln -sf /tmp/.ydotool_socket %t/.ydotool_socket
ExecStopPost=/bin/rm -f %t/.ydotool_socket
EOF
  say "patched ydotoold.service with socket-path symlink (Ubuntu 24.04 fix)"
fi

systemctl --user daemon-reload
systemctl --user enable --now clipboard-janitor.service
systemctl --user enable --now flashpaste-screenshot-watcher.path
say "enabled clipboard-janitor + flashpaste-screenshot-watcher"

# ── config snippets ─────────────────────────────────────────────────
cat <<'EOF'

────────────────────────────────────────────────────────────────────
flashpaste installed. Now append these to your dotfiles:

  examples/tmux.conf.snippet     → ~/.tmux.conf
  examples/kitty.conf.snippet    → ~/.config/kitty/kitty.conf

Then reload:

  tmux source-file ~/.tmux.conf
  # restart kitty

Test:
  1. Press PrtScr to take a screenshot.
  2. Right-click in any tmux pane → Paste.
  3. Image should attach in <120 ms.

Watch the timeline live:
  tail -F ~/.local/state/tmux-paste.log | grep T+
────────────────────────────────────────────────────────────────────
EOF
