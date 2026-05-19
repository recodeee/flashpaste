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

# Don't clobber a fresh browser text copy. xclip is X11 and DOESN'T
# consume wl-copy --paste-once, so this probe is safe.
xt=$(timeout 0.2 xclip -selection clipboard -t text/plain -o 2>/dev/null | head -c 8)
case "$xt" in
  $'\x89PNG'*) xt= ;;   # PNG header masquerading as text — ignore
esac
if [ -n "$xt" ]; then
  clog "ss-preload" "event=skip-text-on-clipboard" "preview='$(printf '%s' "$xt" | tr '\n' ' ')'"
  exit 0
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
