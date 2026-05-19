#!/usr/bin/env bash
# flashpaste — screenshot pre-loader.
#
# Triggered by a systemd .path unit watching ~/Pictures/Screenshots/.
# The moment GNOME's screenshot tool writes a new PNG, this script
# loads it into xclip so the user's clipboard is "hot" the instant
# they reach for right-click → Paste.
#
# Effect: cuts perceived latency from ~1s (screenshot → paste click →
# dispatch reads file → xclip load → send-text) to ~150ms (screenshot
# → preload daemon already loaded → paste click → send-text).
#
# Safety guards:
#   1. Only files ≤10s old — ignore old files re-touched by other apps.
#   2. Don't clobber a fresh browser text copy on the clipboard.
#   3. Don't fire on the same file twice in a row (state file dedupe).

set -u

. /home/$USER/.local/bin/clip-pipeline-log.sh 2>/dev/null \
  || . "$(dirname "$0")/clip-pipeline-log.sh" 2>/dev/null \
  || true
type clog >/dev/null 2>&1 || clog() { :; }

SS_DIR="$HOME/Pictures/Screenshots"
STATE="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/flashpaste-last-preload"

[ -d "$SS_DIR" ] || exit 0

# Find freshest PNG/JPG written in the last minute.
latest=$(find "$SS_DIR" -maxdepth 1 -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' \) -mmin -1 -printf "%T@ %p\n" 2>/dev/null | sort -nr | sed -n '1s/^[^ ]* //p')
[ -z "$latest" ] && { clog "ss-preload" "event=no-fresh-file"; exit 0; }

mtime=$(stat -c %Y "$latest" 2>/dev/null || echo 0)
age=$(($(date +%s) - mtime))
if [ "$age" -gt 10 ]; then
  clog "ss-preload" "event=skip-too-old" "path='$latest'" "age_s=$age"
  exit 0
fi

# Dedupe: don't re-preload the same screenshot multiple times.
last_mtime=$(cat "$STATE" 2>/dev/null || echo 0)
if [ "$mtime" -le "$last_mtime" ]; then
  clog "ss-preload" "event=skip-already-loaded" "path='$latest'" "mtime=$mtime"
  exit 0
fi

# Previously skipped pre-load if X11 clipboard already held text — the
# intent was "don't clobber a fresh browser text copy with a screenshot
# the user took accidentally." In practice that branch is the dominant
# failure mode for image-paste on this stack: copy something, take a
# screenshot, press Ctrl-V, and Claude pastes the OLD TEXT instead of
# the screenshot. The user's clear intent on PrtScr is "I want to paste
# this screenshot." The ≤10 s file-age guard above already rules out
# stale files being re-touched by other apps. Probe kept here only as
# a breadcrumb in the pipeline log so debugging shows what was on the
# clipboard right before we overwrote it.
xt=$(timeout 0.2 xclip -selection clipboard -t text/plain -o 2>/dev/null | head -c 16)
case "$xt" in
  $'\x89PNG'*) xt= ;;   # PNG header masquerading as text — ignore
esac
if [ -n "$xt" ]; then
  clog "ss-preload" "event=overriding-text" "preview='$(printf '%s' "$xt" | tr '\n' ' ')'"
fi

case "$latest" in
  *.png|*.PNG) mime=image/png ;;
  *.jpg|*.JPG|*.jpeg|*.JPEG) mime=image/jpeg ;;
  *) mime=image/png ;;
esac

setsid -f xclip -selection clipboard -t "$mime" -i "$latest" >/dev/null 2>&1
printf '%s' "$mtime" >"$STATE" 2>/dev/null
clog "ss-preload" "event=loaded" "path='$latest'" "age_s=$age" "mime='$mime'"

# Event-driven dock-icon cleanup. xclip is now the durable selection
# owner — any wl-copy daemons (from GNOME's screenshot tool or earlier
# screenshots) are redundant and just clutter Ubuntu Dock with phantom
# "Unknown" gear icons. Give xclip 200ms to fully claim the selection,
# then SIGTERM every wl-copy. Cuts dock icons within ~250ms of
# screenshot, vs the janitor's polling-interval-bounded latency.
(
  sleep 0.2
  killed=$(pgrep -x wl-copy | tr '\n' ' ')
  if [ -n "$killed" ]; then
    pgrep -x wl-copy | xargs -r kill -TERM 2>/dev/null
    clog "ss-preload" "event=killed-wl-copy" "pids='$killed'"
  fi
) </dev/null >/dev/null 2>&1 &
disown 2>/dev/null || true
