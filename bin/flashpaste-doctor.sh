#!/usr/bin/env bash
# flashpaste-doctor — parallel environment checks. Runs every probe in
# the background, then collates results into a single table so the
# total wall-clock is bounded by the slowest single check (~500ms)
# instead of summing across them.
#
# Exits 0 if every required check passes, 1 otherwise. Warnings ("⚠")
# don't fail; only critical missing pieces do.

set -u

GREEN='\033[1;32m'
YELLOW='\033[1;33m'
RED='\033[1;31m'
DIM='\033[2m'
RESET='\033[0m'

ok()    { printf "${GREEN}✅${RESET} %-28s %s\n" "$1" "$2"; }
warn()  { printf "${YELLOW}⚠️${RESET} %-28s %s\n" "$1" "$2"; }
fail()  { printf "${RED}❌${RESET} %-28s %s\n" "$1" "$2"; }
hdr()   { printf "\n${DIM}── %s ──${RESET}\n" "$1"; }

overlay_socket_path() {
  if [ -n "${XDG_RUNTIME_DIR:-}" ]; then
    printf '%s/flashpaste-overlay.sock' "$XDG_RUNTIME_DIR"
  else
    printf '/tmp/flashpaste-overlay.sock'
  fi
}

run_timeout() {
  local duration=$1
  shift
  if command -v timeout >/dev/null 2>&1; then
    timeout "$duration" "$@"
  else
    "$@"
  fi
}

one_line() {
  tr '\n' ' ' | sed 's/[[:space:]][[:space:]]*/ /g' | head -c 180
}

# Result dir — each parallel check writes one file there.
RDIR=$(mktemp -d -t flashpaste-doctor.XXXXXX)
trap 'rm -rf "$RDIR"' EXIT

# Emit a single result row from a worker. Format: <status>\t<label>\t<msg>
# status ∈ {ok,warn,fail}
emit() {
  local status=$1 label=$2 msg=$3 slot=$4
  printf '%s\t%s\t%s\n' "$status" "$label" "$msg" >"$RDIR/$slot"
}

# ── parallel probes ────────────────────────────────────────────────

# 1. Wayland session
( if [ "${XDG_SESSION_TYPE:-}" = "wayland" ]; then
    emit ok "Wayland session" "XDG_SESSION_TYPE=$XDG_SESSION_TYPE, WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-?}" 10
  elif [ -n "${WAYLAND_DISPLAY:-}" ]; then
    emit warn "Wayland session" "WAYLAND_DISPLAY set but XDG_SESSION_TYPE='${XDG_SESSION_TYPE:-}'" 10
  else
    emit fail "Wayland session" "not in a Wayland session (got '${XDG_SESSION_TYPE:-}') — flashpaste is Wayland-only" 10
  fi
) &

# 2. Compositor — is it mutter?
( if command -v gnome-shell >/dev/null 2>&1 && pgrep -x gnome-shell >/dev/null 2>&1; then
    ver=$(gnome-shell --version 2>/dev/null | head -c 60)
    emit ok "GNOME Shell / mutter" "$ver" 20
  elif pgrep -x sway >/dev/null 2>&1; then
    emit warn "Compositor" "sway detected — flashpaste targets mutter; may work but untested" 20
  elif pgrep -x Hyprland >/dev/null 2>&1; then
    emit warn "Compositor" "Hyprland detected — flashpaste targets mutter; may work but untested" 20
  else
    emit warn "Compositor" "not GNOME Shell — flashpaste's quirk workarounds may be overkill" 20
  fi
) &

# 3. kitty installed
( if command -v kitty >/dev/null 2>&1; then
    ver=$(kitty --version 2>/dev/null | head -c 40)
    emit ok "kitty installed" "$ver" 30
  else
    emit fail "kitty installed" "missing — install via your package manager (apt/pacman/dnf)" 30
  fi
) &

# 4. kitty running AND has a remote-control socket
( sock=""
  for sock_path in /run/user/$(id -u)/kitty-main-* /run/user/$(id -u)/kitty*; do
    [ -S "$sock_path" ] && sock="$sock_path" && break
  done
  if [ -n "$sock" ]; then
    emit ok "kitty IPC socket" "$sock" 40
  elif pgrep -x kitty >/dev/null 2>&1; then
    emit warn "kitty IPC socket" "kitty running but no listen socket — add 'allow_remote_control yes' + 'listen_on unix:@kitty-main-{kitty_pid}' to kitty.conf" 40
  else
    emit warn "kitty IPC socket" "kitty not running — start kitty before image-paste tests" 40
  fi
) &

# 5. tmux installed and reachable
( if command -v tmux >/dev/null 2>&1; then
    ver=$(tmux -V 2>/dev/null | head -c 40)
    if tmux list-sessions >/dev/null 2>&1; then
      sess=$(tmux list-sessions 2>/dev/null | wc -l)
      emit ok "tmux installed + running" "$ver, $sess session(s)" 50
    else
      emit warn "tmux installed" "$ver, but no live sessions (start one before testing)" 50
    fi
  else
    emit fail "tmux installed" "missing — install tmux 3.0 or newer" 50
  fi
) &

# 6. tmux running INSIDE kitty (the supported topology)
( if [ -n "${TMUX:-}" ]; then
    parent_term="${KITTY_PID:-}${TERM_PROGRAM:-}${TERM:-}"
    if [ -n "${KITTY_WINDOW_ID:-}" ] || [ -n "${KITTY_PID:-}" ]; then
      emit ok "tmux inside kitty" "TMUX=$TMUX, KITTY_WINDOW_ID=${KITTY_WINDOW_ID:-?}" 60
    elif [ "${TERM:-}" = "tmux-256color" ] && command -v kitty >/dev/null 2>&1; then
      emit warn "tmux inside kitty" "TMUX set but no KITTY_* env — may have launched tmux outside a kitty window" 60
    else
      emit warn "tmux inside kitty" "tmux running but not inside kitty (parent_term='$parent_term'); flashpaste works best in kitty" 60
    fi
  else
    emit warn "tmux inside kitty" "not currently inside a tmux session (run inside tmux to actually test paste)" 60
  fi
) &

# 7. wl-clipboard
( if command -v wl-paste >/dev/null 2>&1 && command -v wl-copy >/dev/null 2>&1; then
    ver=$(wl-paste --version 2>/dev/null | head -c 40)
    emit ok "wl-clipboard" "$ver" 70
  else
    emit fail "wl-clipboard" "missing — apt install wl-clipboard" 70
  fi
) &

# 8. xclip (XWayland fallback)
( if command -v xclip >/dev/null 2>&1; then
    emit ok "xclip (XWayland)" "$(xclip -version 2>&1 | head -1 | head -c 40)" 80
  else
    emit fail "xclip" "missing — apt install xclip (xclip is flashpaste's primary fallback when mutter is wedged)" 80
  fi
) &

# 9. ydotool + ydotoold socket
( if ! command -v ydotool >/dev/null 2>&1; then
    emit fail "ydotool" "missing — apt install ydotool ydotoold" 90
  else
    sock="${YDOTOOL_SOCKET:-${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/.ydotool_socket}"
    if [ -S "$sock" ]; then
      emit ok "ydotool socket" "$sock" 90
    elif [ -S "/tmp/.ydotool_socket" ]; then
      emit warn "ydotool socket" "/tmp/.ydotool_socket exists but $sock missing — install.sh patches this via systemd drop-in" 90
    else
      emit fail "ydotool socket" "ydotool installed but ydotoold not running — systemctl --user enable --now ydotoold.service" 90
    fi
  fi
) &

# 10. ~/Pictures/Screenshots/ exists (for the auto-pickup path)
( if [ -d "$HOME/Pictures/Screenshots" ]; then
    n=$(find "$HOME/Pictures/Screenshots" -maxdepth 1 -type f -name '*.png' 2>/dev/null | wc -l)
    emit ok "Screenshots directory" "$HOME/Pictures/Screenshots/ ($n PNG files)" 100
  else
    emit warn "Screenshots directory" "$HOME/Pictures/Screenshots/ doesn't exist — GNOME's PrtScr will create it on first use" 100
  fi
) &

# 11. tmux-paste-dispatch.sh already installed?
( if [ -x "$HOME/.local/bin/tmux-paste-dispatch.sh" ]; then
    emit ok "flashpaste installed" "$HOME/.local/bin/tmux-paste-dispatch.sh" 110
  else
    emit warn "flashpaste installed" "not yet — bootstrap.sh / install.sh will fix this" 110
  fi
) &

# 12. systemd user services
( if systemctl --user is-active clipboard-janitor.service >/dev/null 2>&1; then
    emit ok "clipboard-janitor.service" "running" 120
  else
    emit warn "clipboard-janitor.service" "not running (install.sh enables it)" 120
  fi
) &

# 13. clipboard-poll.service must NOT be running (clobbers clipboard).
( if systemctl --user is-active clipboard-poll.service >/dev/null 2>&1; then
    emit fail "clipboard-poll.service" "RUNNING — this poller will clobber your clipboard. Disable with: systemctl --user disable --now clipboard-poll.service" 130
  else
    emit ok "clipboard-poll.service" "disabled (correct)" 130
  fi
) &

# 14. flashpaste-overlayd installed.
( if path=$(command -v flashpaste-overlayd 2>/dev/null); then
    ver=$(flashpaste-overlayd --version 2>/dev/null | one_line)
    if [ -n "$ver" ]; then
      emit ok "flashpaste-overlayd" "$ver ($path)" 140
    else
      emit ok "flashpaste-overlayd" "$path" 140
    fi
  else
    emit fail "flashpaste-overlayd" "missing from PATH — install/rebuild with overlayd enabled and ensure ~/.local/bin or package bin is on PATH" 140
  fi
) &

# 15. flashpaste-overlayd daemon socket exists.
( sock=$(overlay_socket_path)
  if [ -S "$sock" ]; then
    emit ok "overlayd socket" "$sock" 150
  elif [ -e "$sock" ]; then
    emit fail "overlayd socket" "$sock exists but is not a socket — remove it and restart flashpaste-overlayd" 150
  else
    emit fail "overlayd socket" "missing at $sock — start with: systemctl --user enable --now flashpaste-overlayd.service" 150
  fi
) &

# 16. compositor can host the overlay surface.
( if ! command -v flashpaste-overlayd >/dev/null 2>&1; then
    emit fail "overlay compositor" "skipped — flashpaste-overlayd missing from PATH" 160
  else
    out=$(run_timeout 3s flashpaste-overlayd --probe 2>&1)
    rc=$?
    summary=$(printf '%s' "$out" | one_line)
    if [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q "LayerShell"; then
      emit ok "overlay compositor" "layer-shell capable ($summary)" 160
    elif [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q "XdgToplevelFallback"; then
      emit warn "overlay compositor" "layer-shell unavailable; fallback surface OK ($summary) — on GNOME ensure xdg-desktop-portal is installed" 160
    elif [ "$rc" -eq 124 ]; then
      emit fail "overlay compositor" "flashpaste-overlayd --probe timed out — check WAYLAND_DISPLAY/XDG_RUNTIME_DIR and compositor health" 160
    else
      emit fail "overlay compositor" "probe failed (exit $rc): $summary — check WAYLAND_DISPLAY/XDG_RUNTIME_DIR and compositor support" 160
    fi
  fi
) &

# 17. draw_rect ttl=100ms IPC round-trip.
( sock=$(overlay_socket_path)
  if [ ! -S "$sock" ]; then
    emit fail "overlay draw_rect" "skipped — daemon socket missing at $sock" 170
  else
    if command -v flashpaste-overlay >/dev/null 2>&1; then
      out=$(run_timeout 2s flashpaste-overlay rect --x 8 --y 8 --w 24 --h 16 --color '#ffae00' --ttl-ms 100 2>&1)
    elif command -v python3 >/dev/null 2>&1; then
      out=$(run_timeout 2s python3 - "$sock" 2>&1 <<'PY'
import json
import socket
import sys
import uuid

sock_path = sys.argv[1]
payload = {
    "type": "draw_rect",
    "id": str(uuid.uuid4()),
    "x": 8.0,
    "y": 8.0,
    "w": 24.0,
    "h": 16.0,
    "color": "#ffae00",
    "ttl_ms": 100,
}
client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.settimeout(1.5)
client.connect(sock_path)
client.sendall(json.dumps(payload, separators=(",", ":")).encode("utf-8") + b"\n")
response = client.makefile("rb").readline().decode("utf-8", "replace").strip()
print(response)
sys.exit(0 if json.loads(response).get("ok") is True else 1)
PY
)
    else
      emit fail "overlay draw_rect" "no flashpaste-overlay client or python3 fallback — install/rebuild overlayd package or install python3" 170
      exit 0
    fi
    rc=$?
    summary=$(printf '%s' "$out" | one_line)
    compact=$(printf '%s' "$out" | tr -d '[:space:]')
    if [ "$rc" -eq 0 ] && printf '%s' "$compact" | grep -q '"ok":true'; then
      emit ok "overlay draw_rect" "ttl=100ms round-trip OK ($summary)" 170
    elif [ "$rc" -eq 124 ]; then
      emit fail "overlay draw_rect" "timed out talking to $sock — restart flashpaste-overlayd and retry" 170
    else
      emit fail "overlay draw_rect" "round-trip failed (exit $rc): $summary — restart flashpaste-overlayd and check socket permissions" 170
    fi
  fi
) &

# 18. tesseract — OPTIONAL; powers `flashpaste-shoot --ocr` and --ocr-only.
( if command -v tesseract >/dev/null 2>&1; then
    ver=$(tesseract --version 2>&1 | head -1 | head -c 40)
    emit ok "tesseract installed" "$ver" 180
  else
    emit warn "tesseract installed" "missing — apt install tesseract-ocr (optional; only used for --ocr / --ocr-only)" 180
  fi
) &

# 19. swappy / satty — OPTIONAL; either one powers `flashpaste-shoot --annotate`.
( if command -v swappy >/dev/null 2>&1; then
    emit ok "annotate editor" "swappy ($(command -v swappy))" 190
  elif command -v satty >/dev/null 2>&1; then
    emit ok "annotate editor" "satty ($(command -v satty))" 190
  else
    emit warn "annotate editor" "neither swappy nor satty on PATH — --annotate falls back to raw capture (apt install swappy)" 190
  fi
) &

# ── collate ────────────────────────────────────────────────────────
wait

hdr "flashpaste doctor — environment check"
fails=0
warns=0
core_checks=0
optional_checks=0
for f in $(ls "$RDIR" | sort -n); do
  IFS=$'\t' read -r status label msg <"$RDIR/$f"
  if [ "$f" -lt 180 ]; then
    core_checks=$((core_checks + 1))
  else
    optional_checks=$((optional_checks + 1))
  fi
  case "$status" in
    ok)   ok   "$label" "$msg" ;;
    warn) warn "$label" "$msg"; warns=$((warns + 1)) ;;
    fail) fail "$label" "$msg"; fails=$((fails + 1)) ;;
  esac
done

echo
if [ "$fails" -eq 0 ] && [ "$warns" -eq 0 ]; then
  printf "${GREEN}All $core_checks core checks passed.${RESET} $optional_checks optional probe(s) also passed. flashpaste should work out of the box.\n"
  exit 0
elif [ "$fails" -eq 0 ]; then
  printf "${YELLOW}$warns warning(s)${RESET}, no failures across $core_checks core checks (+$optional_checks optional probe(s)). flashpaste will still install; address warnings if image paste misbehaves.\n"
  exit 0
else
  printf "${RED}$fails failure(s)${RESET}, $warns warning(s) across $core_checks core checks (+$optional_checks optional probe(s)). Fix the ${RED}❌${RESET} items above before installing.\n"
  exit 1
fi
