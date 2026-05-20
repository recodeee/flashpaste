#!/usr/bin/env bash
# flashpaste one-line installer.
# Run with:
#
#   curl -fsSL https://raw.githubusercontent.com/NagyVikt/flashpaste/main/bootstrap.sh | bash
#
# Or if you don't trust pipe-to-bash (good instinct):
#
#   curl -fsSL https://raw.githubusercontent.com/NagyVikt/flashpaste/main/bootstrap.sh -o bootstrap.sh
#   less bootstrap.sh
#   bash bootstrap.sh

set -euo pipefail

GREEN='\033[1;32m'
YELLOW='\033[1;33m'
RED='\033[1;31m'
CYAN='\033[1;36m'
RESET='\033[0m'

say()  { printf "${GREEN}==>${RESET} %s\n" "$*"; }
warn() { printf "${YELLOW}warn:${RESET} %s\n" "$*"; }
die()  { printf "${RED}error:${RESET} %s\n" "$*" >&2; exit 1; }
info() { printf "${CYAN} ?${RESET}  %s\n" "$*"; }

REPO="https://github.com/NagyVikt/flashpaste.git"
DEST="${FLASHPASTE_DIR:-$HOME/.local/share/flashpaste}"

# ── platform sanity ────────────────────────────────────────────────
if [ "$(uname)" != "Linux" ]; then
  die "flashpaste targets Linux (GNOME Wayland). Detected: $(uname)"
fi

if [ -z "${XDG_SESSION_TYPE:-}" ] || [ "$XDG_SESSION_TYPE" != "wayland" ]; then
  warn "Not in a Wayland session (XDG_SESSION_TYPE='$XDG_SESSION_TYPE'). flashpaste targets GNOME Wayland; X11 might work but is untested."
fi

# ── dependency check ──────────────────────────────────────────────
need=()
for cmd in git tmux xclip wl-copy wl-paste ydotool kitty; do
  command -v "$cmd" >/dev/null 2>&1 || need+=("$cmd")
done

if [ ${#need[@]} -gt 0 ]; then
  info "missing dependencies: ${need[*]}"
  if command -v apt >/dev/null 2>&1; then
    info "install on Ubuntu/Debian:"
    echo "    sudo apt install -y wl-clipboard xclip xsel ydotool ydotoold tmux kitty git"
  elif command -v pacman >/dev/null 2>&1; then
    info "install on Arch:"
    echo "    sudo pacman -S wl-clipboard xclip xsel ydotool tmux kitty git"
  elif command -v dnf >/dev/null 2>&1; then
    info "install on Fedora:"
    echo "    sudo dnf install wl-clipboard xclip xsel ydotool tmux kitty git"
  fi
  read -rp "Continue anyway? [y/N] " ans
  case "$ans" in [yY]*) : ;; *) exit 0 ;; esac
fi

# ── clone or pull ─────────────────────────────────────────────────
if [ -d "$DEST/.git" ]; then
  say "updating existing checkout at $DEST"
  git -C "$DEST" pull --ff-only --quiet
else
  say "cloning $REPO -> $DEST"
  mkdir -p "$(dirname "$DEST")"
  git clone --depth=1 --quiet "$REPO" "$DEST"
fi

# ── pre-flight doctor (parallel env checks) ──────────────────────
say "running doctor (parallel environment checks)"
if ! bash "$DEST/bin/flashpaste-doctor.sh"; then
  echo
  warn "doctor reported failures. Fix the ✗ items above and re-run:"
  echo "    bash $DEST/bootstrap.sh"
  exit 1
fi

# ── run installer ────────────────────────────────────────────────
say "running installer"
bash "$DEST/install.sh"

# ── post-install hint ─────────────────────────────────────────────
cat <<EOF

${GREEN}flashpaste${RESET} cloned to ${CYAN}$DEST${RESET}

Next steps:

  1. Append the tmux + kitty snippets to your dotfiles:
     ${CYAN}cat $DEST/examples/tmux.conf.snippet  >> ~/.tmux.conf${RESET}
     ${CYAN}cat $DEST/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf${RESET}

  2. Reload:
     ${CYAN}tmux source-file ~/.tmux.conf${RESET}
     ${CYAN}# restart kitty${RESET}

  3. Test: PrtScr → right-click in any tmux pane → Paste

  Overlay daemon:
     ${CYAN}systemctl --user status flashpaste-overlayd.service${RESET}

  Run with FLASHPASTE_QUIET=1 to disable logging (~10ms speedup).

EOF
