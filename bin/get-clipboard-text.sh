#!/usr/bin/env bash
# Print the system clipboard as text. If the live clipboard has no text
# (e.g. it holds a screenshot / image), fall back to the most recent text
# entry stored by cliphist. Used by tmux's @paste fallback chain.
#
# WHY EVERY wl-paste CALL IS WRAPPED IN `timeout`:
# On Wayland + GNOME mutter, `wl-paste -t <MIME>` against an image-only
# or empty clipboard can hang indefinitely instead of exiting non-zero.
# Each hung process holds a wl_data_device.receive() fd open. They pile
# up over time and starve future paste attempts (the user's "15 ctrl+v
# presses" symptom — every prior attempt left a zombie wl-paste behind).
# A 1-second ceiling guarantees forward progress and lets the cleanup
# logic in clipboard-poll.sh reap them.
#
# WHY THE MIME PRE-CHECK:
# Even with timeouts, blindly trying 4 text MIME variants wastes 4
# seconds when none of them are offered. One `--list-types` call is
# cheap and tells us exactly which (if any) to try.
#
# WHY THE is_text VALIDATION IS PARANOID:
# On Wayland with image-only clipboards, BOTH of these are unreliable —
#   - `xsel -b`: exits 0 with empty output (false positive on "I got text")
#   - `xclip -selection clipboard -o -t UTF8_STRING`: returns raw PNG bytes
#     even though you asked for text (it ignores `-t` when the only
#     advertised type is image/png on XWayland).
# Both of these would otherwise dump binary into your shell, which corrupts
# the prompt and produces "received a misencoded char" garbage. So every
# branch's output is funneled through `is_text` before being accepted.
set -u

. /home/deadpool/.local/bin/clip-pipeline-log.sh 2>/dev/null || true
type clog >/dev/null 2>&1 || clog() { :; }
clog "get-clipboard" "event=invoked"

readonly WL_TIMEOUT="${CLIPBOARD_GET_TIMEOUT:-1.0}"
readonly X11_TIMEOUT="${CLIPBOARD_GET_X11_TIMEOUT:-0.5}"

# If we were spawned by a tmux server that lost track of WAYLAND_DISPLAY /
# DISPLAY (e.g. tmux was started from a tty before the graphical session
# came up), borrow them from a running kitty. Otherwise every wl-paste /
# xclip call below fails with "couldn't connect to a Wayland server" and
# the caller silently gets an empty paste.
ensure_env() {
  [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ] && return
  for pid in $(pgrep -u "$(id -u)" -x kitty 2>/dev/null); do
    [ -r "/proc/$pid/environ" ] || continue
    while IFS= read -r -d '' kv; do
      case "$kv" in
        WAYLAND_DISPLAY=*|DISPLAY=*|XDG_RUNTIME_DIR=*|DBUS_SESSION_BUS_ADDRESS=*|XAUTHORITY=*)
          export "$kv" ;;
      esac
    done < "/proc/$pid/environ"
    break
  done
}
ensure_env

is_text() {
  # $1 is a file path. Reject if empty, if its first 4 KiB contain a NUL
  # byte (a near-perfect "is binary" signal for non-empty text), or if it
  # starts with a known image/binary magic number.
  #
  # We avoid `grep -q $'\0'` because bash strips the NUL during expansion,
  # leaving `grep -q ''` which matches every non-empty file. Instead, hex-
  # dump the first 4 KiB and look for the byte `00`.
  [ -s "$1" ] || return 1
  if LC_ALL=C head -c 4096 "$1" | LC_ALL=C od -An -tx1 -v | tr -d ' \n' | LC_ALL=C grep -q '00'; then
    return 1
  fi
  magic="$(LC_ALL=C head -c 4 "$1" | LC_ALL=C od -An -tx1 -v | tr -d ' \n')"
  case "$magic" in
    89504e47)   return 1 ;; # PNG
    ffd8ff*)    return 1 ;; # JPEG
    47494638)   return 1 ;; # GIF
    52494646)   return 1 ;; # RIFF (WebP / WAV)
    25504446)   return 1 ;; # PDF
    1f8b*)      return 1 ;; # gzip
    504b0304)   return 1 ;; # ZIP / docx / xlsx
  esac
  return 0
}

# Run a command with a timeout, capturing stdout to a tempfile, then
# emit it (and exit 0) iff is_text approves. Caller is responsible for
# checking exit status.
try_capture() {
  local timeout_s="$1"; shift
  local tmp
  tmp="$(mktemp -t clipget.XXXXXX)" || return 1
  if timeout "$timeout_s" "$@" >"$tmp" 2>/dev/null && is_text "$tmp"; then
    cat "$tmp"
    rm -f "$tmp"
    return 0
  fi
  rm -f "$tmp"
  return 1
}

# Probe which MIME types the current clipboard advertises. One round-trip,
# bounded by WL_TIMEOUT — if mutter's clipboard is wedged we move on to
# the X11 fallbacks instead of stacking blocked wl-paste processes.
wayland_types=""
if [ -n "${WAYLAND_DISPLAY:-}" ] && command -v wl-paste >/dev/null 2>&1; then
  wayland_types="$(timeout "$WL_TIMEOUT" wl-paste --list-types 2>/dev/null || true)"
fi
clog "get-clipboard" "event=wayland-types-probed" "types='$(printf '%s' "$wayland_types" | tr '\n' ',')'"

# Only request a Wayland text type that the clipboard actually offers.
if [ -n "$wayland_types" ]; then
  for want in 'text/plain;charset=utf-8' 'text/plain' 'UTF8_STRING' 'STRING'; do
    if printf '%s\n' "$wayland_types" | grep -Fxq "$want"; then
      clog "get-clipboard" "event=try-wayland" "mime='$want'"
      if try_capture "$WL_TIMEOUT" wl-paste --no-newline -t "$want"; then
        clog "get-clipboard" "event=success" "source=wayland" "mime='$want'"
        exit 0
      fi
    fi
  done
  while IFS= read -r t; do
    case "$t" in
      text/*)
        clog "get-clipboard" "event=try-wayland-fallback" "mime='$t'"
        if try_capture "$WL_TIMEOUT" wl-paste --no-newline -t "$t"; then
          clog "get-clipboard" "event=success" "source=wayland-fallback" "mime='$t'"
          exit 0
        fi
        ;;
    esac
  done <<<"$wayland_types"
fi

if [ -n "${DISPLAY:-}" ]; then
  command -v xclip >/dev/null 2>&1 && {
    clog "get-clipboard" "event=try-xclip" "target=UTF8_STRING"
    if try_capture "$X11_TIMEOUT" xclip -selection clipboard -o -t UTF8_STRING; then
      clog "get-clipboard" "event=success" "source=xclip" "target=UTF8_STRING"
      exit 0
    fi
    clog "get-clipboard" "event=try-xclip" "target=text/plain"
    if try_capture "$X11_TIMEOUT" xclip -selection clipboard -o -t text/plain; then
      clog "get-clipboard" "event=success" "source=xclip" "target=text/plain"
      exit 0
    fi
  }
  command -v xsel >/dev/null 2>&1 && {
    clog "get-clipboard" "event=try-xsel"
    if try_capture "$X11_TIMEOUT" xsel -b; then
      clog "get-clipboard" "event=success" "source=xsel"
      exit 0
    fi
  }
fi

# Final safety net: cliphist history fallback.
if command -v cliphist >/dev/null 2>&1; then
  tmp="$(mktemp -t cliphist.XXXXXX)" || exit 0
  cliphist list 2>/dev/null | head -1 | cliphist decode >"$tmp" 2>/dev/null
  if is_text "$tmp"; then
    clog "get-clipboard" "event=success" "source=cliphist-fallback" "preview='$(head -c 100 "$tmp" | tr '\n\r\t' '   ')'"
    cat "$tmp"
  else
    clog "get-clipboard" "event=cliphist-not-text"
  fi
  rm -f "$tmp"
fi
clog "get-clipboard" "event=no-source-yielded-text"
