#!/usr/bin/env bash
# Build a .deb package for flashpaste.
#
# Usage:
#   ./packaging/build-deb.sh           # builds dist/flashpaste_<ver>_all.deb
#   VERSION=1.2.3 ./packaging/build-deb.sh
#
# Requires: dpkg-deb (preinstalled on Debian/Ubuntu).
#
# After building, install with:
#   sudo apt install ./dist/flashpaste_*_all.deb
# OR
#   sudo dpkg -i dist/flashpaste_*_all.deb && sudo apt-get install -f
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${VERSION:-$(git -C "$REPO_DIR" describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || echo "0.1.0")}"
ARCH="${ARCH:-all}"     # all = noarch (pure bash)
STAGE="${STAGE:-$REPO_DIR/dist/staging}"
OUT_DIR="$REPO_DIR/dist"
OUT_DEB="$OUT_DIR/flashpaste_${VERSION}_${ARCH}.deb"

GREEN='\033[1;32m'; YEL='\033[1;33m'; RED='\033[1;31m'; RESET='\033[0m'
say()  { printf "${GREEN}==>${RESET} %s\n" "$*"; }
warn() { printf "${YEL}warn:${RESET} %s\n" "$*"; }
die()  { printf "${RED}error:${RESET} %s\n" "$*" >&2; exit 1; }

command -v dpkg-deb >/dev/null || die "dpkg-deb not found — apt install dpkg"

say "version=$VERSION arch=$ARCH"

# Clean previous staging tree.
rm -rf "$STAGE"
mkdir -p "$OUT_DIR"
mkdir -p "$STAGE/DEBIAN"
mkdir -p "$STAGE/usr/bin"
mkdir -p "$STAGE/usr/share/applications"
mkdir -p "$STAGE/usr/share/doc/flashpaste"
mkdir -p "$STAGE/usr/share/flashpaste/examples"
mkdir -p "$STAGE/usr/lib/systemd/user"

# ── filesystem payload ───────────────────────────────────────────
# Scripts go to /usr/bin/ (executable, no .sh suffix to be PATH-friendly).
for src in "$REPO_DIR"/bin/*.sh "$REPO_DIR"/bin/wl-paste "$REPO_DIR"/bin/screenshot-to-clipboard; do
  [ -f "$src" ] || continue
  base=$(basename "$src" .sh)
  install -m 0755 "$src" "$STAGE/usr/bin/$base"
done

# Special-case wl-paste shim: keep extension-less name and ensure it shadows /usr/bin/wl-paste
# by living in /usr/local/bin (NB: dpkg-deb in this profile keeps it under /usr/bin).
# A symlink installed by `update-alternatives` would be cleaner; keeping it simple here.

# ── Rust binaries (if built) ────────────────────────────────────
# v1.16+: ship the Rust tier-2 / tier-3 binaries when `cargo build --release`
# has been run. Falls back gracefully to bash-only if they don't exist.
RS_RELEASE="$REPO_DIR/rs/target/release"
if [ -d "$RS_RELEASE" ]; then
  for bin in flashpasted flashpaste-dispatch flashpaste-shoot flashpaste-trigger flashpaste-mcp flashpaste; do
    if [ -x "$RS_RELEASE/$bin" ]; then
      install -m 0755 "$RS_RELEASE/$bin" "$STAGE/usr/bin/$bin"
      say "  + Rust binary: $bin ($(stat -c%s "$RS_RELEASE/$bin") bytes)"
    fi
  done
  # flashpasted systemd user unit
  if [ -f "$REPO_DIR/systemd/flashpasted.service" ]; then
    install -m 0644 "$REPO_DIR/systemd/flashpasted.service" "$STAGE/usr/lib/systemd/user/"
  fi
else
  warn "rs/target/release not found — packaging bash-only (run 'cargo build --release' first for Rust tiers)"
fi

# paste_image.sh installed to /usr/share/flashpaste/ — kitty.conf must reference that path
# (or the postinst can symlink it to $HOME/paste_image.sh per user — handled below).
install -m 0755 "$REPO_DIR/bin/paste_image.sh" "$STAGE/usr/share/flashpaste/paste_image.sh"

# Desktop entries — glob every *.desktop under share/applications/ so new
# entries (e.g. org.flashpaste.daemon.desktop matching the flashpasted
# daemon's Wayland app_id) ship without a build-script edit.
for desk in "$REPO_DIR"/share/applications/*.desktop; do
  [ -f "$desk" ] || continue
  install -m 0644 "$desk" "$STAGE/usr/share/applications/"
done

# systemd user units.
install -m 0644 "$REPO_DIR/systemd/clipboard-janitor.service"               "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$REPO_DIR/systemd/flashpaste-screenshot-watcher.path"      "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$REPO_DIR/systemd/flashpaste-screenshot-watcher.service"   "$STAGE/usr/lib/systemd/user/"

# Examples + docs.
install -m 0644 "$REPO_DIR/examples/tmux.conf.snippet"  "$STAGE/usr/share/flashpaste/examples/"
install -m 0644 "$REPO_DIR/examples/kitty.conf.snippet" "$STAGE/usr/share/flashpaste/examples/"
install -m 0644 "$REPO_DIR/README.md"                   "$STAGE/usr/share/doc/flashpaste/"
install -m 0644 "$REPO_DIR/ROADMAP.md"                  "$STAGE/usr/share/doc/flashpaste/" 2>/dev/null || true
install -m 0644 "$REPO_DIR/LICENSE"                     "$STAGE/usr/share/doc/flashpaste/copyright"

# ── control file ──────────────────────────────────────────────────
cat > "$STAGE/DEBIAN/control" <<EOF
Package: flashpaste
Version: $VERSION
Section: x11
Priority: optional
Architecture: $ARCH
Maintainer: Viktor Nagy <webubusiness@gmail.com>
Depends: bash (>= 5.0), wl-clipboard, xclip, xsel, tmux (>= 3.0), ydotool, ydotoold
Recommends: kitty
Suggests: cliphist, inotify-tools
Homepage: https://github.com/NagyVikt/flashpaste
Description: sub-120ms image paste for terminal AI agents on GNOME Wayland
 flashpaste makes screenshot-into-Claude-Code / Codex / other TUI agents
 actually work on GNOME 46 Wayland. It papers over mutter's surfaceless-
 client clipboard refusal, wl-copy --paste-once drainage by probes,
 tmux bind -n C-v key interception, kitty map ctrl+v interception,
 and the Ubuntu 24.04 ydotool 0.1.8 socket-path bug.
 .
 After installation, append the tmux + kitty snippets from
 /usr/share/flashpaste/examples/ to your dotfiles and enable the
 systemd user services:
 .
   systemctl --user enable --now clipboard-janitor.service
   systemctl --user enable --now flashpaste-screenshot-watcher.path
EOF

# ── post-install hook ─────────────────────────────────────────────
# Run as root during apt install. Best practice: do NOT auto-enable
# per-user systemd services from a root package — print instructions
# so the user opts in.
cat > "$STAGE/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
echo
echo "flashpaste installed. To activate for your user:"
echo
echo "  systemctl --user daemon-reload"
echo "  systemctl --user enable --now clipboard-janitor.service"
echo "  systemctl --user enable --now flashpaste-screenshot-watcher.path"
echo
echo "  # Append the snippets to your dotfiles:"
echo "  cat /usr/share/flashpaste/examples/tmux.conf.snippet  >> ~/.tmux.conf"
echo "  cat /usr/share/flashpaste/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf"
echo "  ln -sf /usr/share/flashpaste/paste_image.sh ~/paste_image.sh"
echo
echo "Run the doctor to verify your environment:"
echo "  flashpaste-doctor"
echo
exit 0
EOF
chmod 0755 "$STAGE/DEBIAN/postinst"

# ── pre-remove hook ───────────────────────────────────────────────
cat > "$STAGE/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
# We don't stop per-user services here — apt runs as root and can't
# touch user services anyway. Print a hint instead.
echo
echo "flashpaste is being removed. To clean up per-user state:"
echo "  systemctl --user disable --now clipboard-janitor.service flashpaste-screenshot-watcher.path"
echo "  rm -f ~/paste_image.sh"
echo
exit 0
EOF
chmod 0755 "$STAGE/DEBIAN/prerm"

# ── build ─────────────────────────────────────────────────────────
say "building $OUT_DEB"
dpkg-deb --root-owner-group --build "$STAGE" "$OUT_DEB"

# Cleanup staging.
rm -rf "$STAGE"

# ── lint ──────────────────────────────────────────────────────────
if command -v lintian >/dev/null 2>&1; then
  say "lintian:"
  lintian --no-tag-display-limit "$OUT_DEB" 2>&1 | head -20 || true
fi

say "done: $OUT_DEB ($(stat -c%s "$OUT_DEB" 2>/dev/null) bytes)"
echo
echo "Install with:"
echo "  sudo apt install $OUT_DEB"
echo "or"
echo "  sudo dpkg -i $OUT_DEB && sudo apt-get install -f"
