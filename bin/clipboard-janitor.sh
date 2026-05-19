#!/usr/bin/env bash
# Reap stuck wl-paste / wl-copy processes left behind by other clipboard
# clients (cliphist, xdg portals, Kiro CLI, Claude Code's image read,
# anything else that calls wl-paste on a wedged GNOME clipboard).
#
# WHY THIS EXISTS as a separate service rather than running inside
# clipboard-poll.sh: the poller had to invoke `wl-paste --type text`
# every cycle to feed cliphist, and every invocation briefly registered
# a Wayland client — surfacing in the Ubuntu dock as a transient
# "Unknown" gear icon. The user wanted zero such icons. This script
# uses ONLY `ps` + `kill` — no Wayland connection, no dock icon, ever.
#
# REAPING POLICY: any wl-paste / wl-copy older than REAP_AFTER seconds.
# On this box (GNOME Shell 46 + wl-clipboard 2.0 with no wlroots
# data-control), a wl-paste alive >10s is always stuck on a clipboard
# generation that's already moved on. wl-copy older than REAP_AFTER
# means a daemon that didn't exit when superseded — also safe to TERM
# (the active owner is whoever called wl-copy most recently).
#
# IDEMPOTENT: flock guarantees a single instance.

set -u

readonly LOCK="/run/user/$(id -u)/clipboard-janitor.lock"
# Asymmetric reaping (TUNED v1.14 for the dock-icon pile-up):
#   wl-paste (reader): a healthy read returns in <1s. Anything alive
#     >REAP_PASTE_AFTER is stuck on mutter's focus-gating failure
#     mode (it'll never get the selection). Aggressively reaping
#     these unsticks Claude Code's paste flow within seconds.
#   wl-copy (owner): WAS 300s (5 minutes) to preserve clipboard for
#     "user might paste later" cases. PROBLEM: every wl-copy is a
#     Wayland client that Ubuntu Dock shows as an "Unknown" gear icon
#     (because wl-copy is surfaceless and can't be matched to a
#     .desktop). After 10 screenshots the dock fills up with 10+
#     phantom icons that stay for 5 minutes.
#     FIX: drop to 10s. flashpaste's pipeline (early-preload + image
#     pre-stage to xclip) makes xclip the durable selection owner
#     within 150ms of any screenshot, so the wl-copy daemon is
#     redundant after that. Killing it 10s later just clears the
#     dock icon — the image stays on xclip's unlimited-reads owner.
# Sweep every INTERVAL seconds.
readonly INTERVAL="${CLIPBOARD_JANITOR_INTERVAL:-1}"
readonly REAP_PASTE_AFTER="${CLIPBOARD_JANITOR_REAP_PASTE_AFTER:-8}"
readonly REAP_COPY_AFTER="${CLIPBOARD_JANITOR_REAP_COPY_AFTER:-3}"
# Back-compat: if old REAP_AFTER is set, treat as wl-paste threshold.
[ -n "${CLIPBOARD_JANITOR_REAP_AFTER:-}" ] && REAP_PASTE_AFTER="$CLIPBOARD_JANITOR_REAP_AFTER"

exec 9>"$LOCK" || exit 0
flock -n 9 || exit 0
trap 'rm -f "$LOCK" 2>/dev/null' EXIT

. /home/deadpool/.local/bin/clip-pipeline-log.sh 2>/dev/null || true
type clog >/dev/null 2>&1 || clog() { :; }
log() {
  printf '[clipboard-janitor] %s\n' "$*" >&2
  clog "clipboard-janitor" "$*"
}

running=1
trap 'running=0' TERM INT HUP

nap() {
  sleep "$1" &
  local pid=$!
  wait "$pid" 2>/dev/null
}

reap() {
  local killed
  killed=$(ps -eo pid=,etimes=,comm= -u "$(id -u)" 2>/dev/null \
    | awk -v pt="$REAP_PASTE_AFTER" -v ct="$REAP_COPY_AFTER" '
        $3=="wl-paste" && $2>pt {print $1" "$3" "$2}
        $3=="wl-copy"  && $2>ct {print $1" "$3" "$2}')
  [ -z "$killed" ] && return 0
  while read -r p name age; do
    [ -n "$p" ] || continue
    log "reap pid=$p comm=$name age=${age}s sig=TERM"
  done <<<"$killed"
  printf '%s\n' "$killed" | awk '{print $1}' | xargs -r kill -TERM 2>/dev/null
  sleep 0.5
  local still
  still=$(printf '%s\n' "$killed" | awk '{print $1}' | while read -r p; do kill -0 "$p" 2>/dev/null && echo "$p"; done)
  if [ -n "$still" ]; then
    log "reap escalating to KILL pids=$(printf '%s' "$still" | tr '\n' ' ')"
    printf '%s\n' "$still" | xargs -r kill -KILL 2>/dev/null
  fi
}

log "started (interval=${INTERVAL}s reap_paste_after=${REAP_PASTE_AFTER}s reap_copy_after=${REAP_COPY_AFTER}s)"

while [ "$running" -eq 1 ]; do
  reap
  nap "$INTERVAL"
done

log "exiting on signal"
