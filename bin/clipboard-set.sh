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
          export "${kv?}"
          ;;
      esac
    done < "/proc/$pid/environ"
    break
  done
}

ensure_env
clog "clipboard-set" "event=env-resolved" "WAYLAND_DISPLAY='${WAYLAND_DISPLAY:-}'" "DISPLAY='${DISPLAY:-}'"

# ── v1.19+ daemon path ────────────────────────────────────────────
# If flashpasted is running, stage the text into the daemon's persistent
# Wayland + X11 selection owners. No wl-copy fork = no phantom
# "wl-clipboard" entry in the Ubuntu Dock. The daemon serves unlimited
# reads from in-memory bytes, exactly what the bash dispatcher's image
# branch already relies on for screenshots.
_sock="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/flashpaste.sock"
if [ -S "$_sock" ] && command -v flashpaste-trigger >/dev/null 2>&1; then
  clog "clipboard-set" "event=backend-chosen" "backend=flashpasted"
  if FLASHPASTE_STAGE_FROM="clipboard-set.sh" \
       flashpaste-trigger --stage-text <"$_tmp"; then
    clog "clipboard-set" "event=done" "backend=flashpasted" "rc=0"
    exit 0
  fi
  # Daemon refused — fall through to the wl-copy / xclip / xsel chain.
  # This preserves the rule "clipboard MUST be set, even if the daemon
  # is wedged" — at worst we get the v1.14 phantom-dock-icon behavior.
  clog "clipboard-set" "event=daemon-declined" "backend=flashpasted"
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# AGENT-LOCKED — DO NOT REVERT THIS GATE  (locked 2026-05-19 by user)
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# This block is intentionally gated behind FLASHPASTE_USE_WL_COPY=1.
#
# WHY (live diagnosis on this box, GNOME 46 + Mutter):
#   • wl-copy is surfaceless. Mutter cannot bind it to the NoDisplay
#     `wl-copy.desktop` (no app_id, no X11 WM_CLASS), so every spawn
#     leaves a phantom "Unknown" gear icon in the Ubuntu Dock for up
#     to 5 minutes (until clipboard-janitor reaps).
#   • Wayland clipboard is already WAYLAND_WEDGED on Mutter (latched
#     in rs/flashpasted/src/wayland.rs — Mutter ships neither
#     ext-data-control nor wlr-data-control). So wl-copy can't even
#     act as a durable selection owner here. It was ALL COST, NO
#     BENEFIT: dock noise + 4–5 extra fork-execs per paste.
#   • The xclip fallback below is the durable owner on this box. The
#     wl-paste shim in ~/.local/bin/wl-paste already routes through it.
#
# OTHER AGENTS / FUTURE EDITORS — READ FIRST:
#   ✗ Do NOT remove this gate.
#   ✗ Do NOT lower the default (the explicit "=1" matters — an empty
#     env must fall through to xclip).
#   ✗ Do NOT "fix" by adding the wl-copy.desktop again — already there;
#     Mutter can't bind a surfaceless client to a .desktop file.
#   ✓ To re-enable on wlroots / KDE / a sway box where wl-copy works,
#     export FLASHPASTE_USE_WL_COPY=1 in the shell that calls this.
#
# Speed: removing this path also drops 4–5 fork-execs per paste
# (pgrep + cat + ps + kill + setsid wl-copy) → straight to xclip.
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
if [ "${FLASHPASTE_USE_WL_COPY:-0}" = "1" ] && [ -n "${WAYLAND_DISPLAY:-}" ] && command -v wl-copy >/dev/null 2>&1; then
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
  # IMPORTANT: wl-copy does NOT auto-daemonize on this box (man wl-copy:
  # "stays in the foreground until the clipboard contents change"). The
  # v1.14 janitor reaps wl-copy >3s old, so any synchronous `wl-copy`
  # call here would exit 143 (SIGTERM) ~3s after we returned — except
  # we already exited rc=$? from the foreground process, so tmux's
  # run-shell binding shows "...clipboard-set.sh' returned 143" in the
  # pane. Fix: explicitly background wl-copy, record its pid, and exit
  # 0. wl-copy lives until janitor reap; clipboard contents stay set.
  setsid wl-copy <"$_tmp" >/dev/null 2>&1 &
  _new=$!
  disown 2>/dev/null || true
  printf '%s\n' "$_new" > "$_pidfile" 2>/dev/null
  clog "clipboard-set" "event=done" "backend=wl-copy" "rc=0" "pid=${_new}"
  exit 0
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
