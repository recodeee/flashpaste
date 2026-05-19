#!/usr/bin/env bash
# Write stdin to the system clipboard with env-resilience.
#
# Called from tmux via `set -g @clip '...clipboard-set.sh'`. The reason this
# script exists rather than calling `wl-copy` directly:
#
#   - tmux's server inherits the environment of whatever shell first launched
#     `tmux` (or `tmux attach`). If that shell was outside the graphical
#     session, WAYLAND_DISPLAY / DISPLAY are unset and `wl-copy` silently
#     exits 1 with "couldn't connect to a Wayland server" -> the user's copy
#     vanishes. tmux's `set-clipboard on` (which emits OSC 52) saves it from
#     total loss, but tools that read the system clipboard later (apps outside
#     kitty, the cliphist watcher, etc.) won't see anything.
#
#   - On Wayland-only boxes `xclip` and `xsel` are XWayland-backed. We still
#     try them as a last resort because XWayland forwards writes to the
#     Wayland clipboard via the mutter bridge.
set -u

# Pipeline logging.
. /home/deadpool/.local/bin/clip-pipeline-log.sh 2>/dev/null || true
type clog >/dev/null 2>&1 || clog() { :; }
type clog_preview >/dev/null 2>&1 || clog_preview() { printf '%s' "${1:-}"; }

# Capture stdin to a tempfile so we can both log it AND forward to the backend.
_tmp="$(mktemp -t clipset.XXXXXX)"
trap 'rm -f "$_tmp" 2>/dev/null' EXIT
cat >"$_tmp"
_size=$(wc -c <"$_tmp" 2>/dev/null || echo 0)
_preview="$(head -c 100 "$_tmp" 2>/dev/null | tr '\n\r\t' '   ')"
clog "clipboard-set" "event=invoked" "bytes=$_size" "preview='$_preview'" "WAYLAND_DISPLAY='${WAYLAND_DISPLAY:-}'" "DISPLAY='${DISPLAY:-}'"

ensure_env() {
  [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ] && return
  for pid in $(pgrep -u "$(id -u)" -x kitty 2>/dev/null); do
    [ -r "/proc/$pid/environ" ] || continue
    while IFS= read -r -d '' kv; do
      case "$kv" in
        WAYLAND_DISPLAY=*|DISPLAY=*|XDG_RUNTIME_DIR=*|DBUS_SESSION_BUS_ADDRESS=*|XAUTHORITY=*)
          export "$kv"
          ;;
      esac
    done < "/proc/$pid/environ"
    break
  done
}

ensure_env
clog "clipboard-set" "event=env-resolved" "WAYLAND_DISPLAY='${WAYLAND_DISPLAY:-}'" "DISPLAY='${DISPLAY:-}'"

if [ -n "${WAYLAND_DISPLAY:-}" ] && command -v wl-copy >/dev/null 2>&1; then
  clog "clipboard-set" "event=backend-chosen" "backend=wl-copy"
  # Reap the previous wl-copy daemon this script spawned. wl-copy stays
  # alive holding the selection until something supersedes it; on this
  # GNOME box each live daemon registers as an "Unknown" gear icon in
  # the Ubuntu dock, and the system doesn't reliably exit superseded
  # owners — so without this every copy adds another icon until the
  # janitor reaps them at 5min. Tracking just OUR pid avoids killing
  # wl-copy daemons spawned by other apps (gnome-screenshot, cliphist, …).
  _pidfile="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/clipboard-set.wl-copy.pid"
  if [ -r "$_pidfile" ]; then
    _prev=$(cat "$_pidfile" 2>/dev/null) || _prev=""
    if [ -n "$_prev" ] && [ "$(ps -p "$_prev" -o comm= 2>/dev/null)" = "wl-copy" ]; then
      kill -TERM "$_prev" 2>/dev/null
    fi
  fi
  wl-copy <"$_tmp"
  rc=$?
  # wl-copy default-daemonizes — parent exits, the surviving daemon
  # holds the clipboard. pgrep -n returns the newest matching pid,
  # which is the daemon we just spawned (races with another concurrent
  # caller are tolerable: worst case we reap someone else's wl-copy
  # next time, identical to what the janitor already does).
  _new=$(pgrep -u "$(id -u)" -n -x wl-copy 2>/dev/null)
  [ -n "$_new" ] && printf '%s\n' "$_new" > "$_pidfile" 2>/dev/null
  clog "clipboard-set" "event=done" "backend=wl-copy" "rc=$rc" "pid=${_new:-?}"
  exit $rc
fi
if [ -n "${DISPLAY:-}" ] && command -v xclip >/dev/null 2>&1; then
  clog "clipboard-set" "event=backend-chosen" "backend=xclip"
  xclip -selection clipboard -i <"$_tmp"
  rc=$?
  clog "clipboard-set" "event=done" "backend=xclip" "rc=$rc"
  exit $rc
fi
if [ -n "${DISPLAY:-}" ] && command -v xsel >/dev/null 2>&1; then
  clog "clipboard-set" "event=backend-chosen" "backend=xsel"
  xsel -b -i <"$_tmp"
  rc=$?
  clog "clipboard-set" "event=done" "backend=xsel" "rc=$rc"
  exit $rc
fi

clog "clipboard-set" "event=no-backend-available" "rc=1"
exit 1
