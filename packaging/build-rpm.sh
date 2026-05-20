#!/usr/bin/env bash
# Build an RPM package for flashpaste.
#
# Usage:
#   ./packaging/build-rpm.sh
#   VERSION=1.2.3 ./packaging/build-rpm.sh
#
# Requires: rpmbuild.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="${VERSION:-$(git -C "$REPO_DIR" describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || echo "1.32")}"
RPM_TOP="${RPM_TOP:-$REPO_DIR/dist/rpmbuild}"
OUT_DIR="$REPO_DIR/dist"
SPEC="$REPO_DIR/packaging/rpm/flashpaste.spec"

GREEN='\033[1;32m'; YEL='\033[1;33m'; RED='\033[1;31m'; RESET='\033[0m'
say()  { printf "${GREEN}==>${RESET} %s\n" "$*"; }
warn() { printf "${YEL}warn:${RESET} %s\n" "$*"; }
die()  { printf "${RED}error:${RESET} %s\n" "$*" >&2; exit 1; }

command -v rpmbuild >/dev/null || die "rpmbuild not found — install rpm-build/rpmdevtools"
[ -f "$SPEC" ] || die "missing spec: $SPEC"

mkdir -p "$RPM_TOP"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS} "$OUT_DIR"

RS_RELEASE="$REPO_DIR/rs/target/release"
if [ ! -x "$RS_RELEASE/flashpaste-overlayd" ] || [ ! -x "$RS_RELEASE/flashpaste-overlay" ]; then
  warn "release overlay binaries not found — build them first:"
  warn "  cargo build --release --manifest-path rs/Cargo.toml -p flashpaste-overlayd --features wayland"
fi

say "version=$VERSION"
rpmbuild -bb "$SPEC" \
  --define "_topdir $RPM_TOP" \
  --define "repo_dir $REPO_DIR" \
  --define "pkg_version $VERSION"

find "$RPM_TOP/RPMS" -type f -name 'flashpaste-*.rpm' -exec cp -f {} "$OUT_DIR/" \;
say "done:"
find "$OUT_DIR" -maxdepth 1 -type f -name 'flashpaste-*.rpm' -printf '  %p (%s bytes)\n' | sort
