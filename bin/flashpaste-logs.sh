#!/usr/bin/env bash
# flashpaste-logs — unified live log viewer across the three streams the
# flashpaste pipeline writes to. Wraps the journalctl + tail invocations
# you'd otherwise have to type by hand, colorizes by source, and prefixes
# each line so you can see what fired in what order.
#
# Streams (toggle with --no-*):
#   [daemon]   journalctl --user -u flashpasted.service        (cyan)
#   [trigger]  ~/.local/state/flashpaste-trigger.log           (yellow)
#   [pipe]     ~/.local/state/clipboard-pipeline.log           (magenta)
#
# Usage:
#   flashpaste-logs                       # follow all three streams (default)
#   flashpaste-logs --since '1 hour ago'  # journalctl-compatible window
#   flashpaste-logs -n 100                # last 100 lines, then follow
#   flashpaste-logs --no-follow           # one-shot snapshot
#   flashpaste-logs --no-pipeline         # drop the pipeline stream
#   flashpaste-logs -g 'pane=%37'         # grep regex across all streams
#   flashpaste-logs --debug               # set RUST_LOG=debug live (requires
#                                         # daemon restart — prompts first)
#
# Exit: Ctrl+C — the trap kills child tails / journalctl cleanly.

set -u

# ── colors (auto-off when stdout isn't a TTY) ────────────────────────────
if [ -t 1 ]; then
  CYN=$'\e[36m' YEL=$'\e[33m' MAG=$'\e[35m' DIM=$'\e[2m' OFF=$'\e[0m' BOLD=$'\e[1m'
else
  CYN='' YEL='' MAG='' DIM='' OFF='' BOLD=''
fi

# ── defaults ─────────────────────────────────────────────────────────────
SINCE=""                # passed to journalctl when set
TAIL_N=50               # initial backfill on log files
FOLLOW=1
WANT_DAEMON=1
WANT_TRIGGER=1
WANT_PIPELINE=1
WANT_SHOT=0             # [shot] flashpaste-screenshot-watcher.service journal
WANT_CLIPBOARD=0        # [clipboard] poller — xclip TARGETS change events (Wayland off by default)
WANT_CLIP_WAYLAND=0     # opt-in: also poll wl-paste (causes dock flash on Mutter)
WANT_KITTY_WIN=0        # [kitty] kitty window list poller — focus / count changes
WANT_CLAUDE=0           # [claude] Claude TUI busy/idle state poller
CLAUDE_PANE=""          # which tmux pane to watch; auto-detect if empty
CLIPBOARD_INTERVAL=1    # seconds between clipboard probes
GREP_REGEX=""
DEBUG_MODE=0

TRIGGER_LOG="${HOME}/.local/state/flashpaste-trigger.log"
PIPELINE_LOG="${HOME}/.local/state/clipboard-pipeline.log"
UNIT="flashpasted.service"
SHOT_UNIT="flashpaste-screenshot-watcher.service"

# ── arg parse ────────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --since)            SINCE="$2"; shift 2;;
    -n|--lines)         TAIL_N="$2"; shift 2;;
    --no-follow|-1)     FOLLOW=0; shift;;
    --no-daemon)        WANT_DAEMON=0; shift;;
    --no-trigger)       WANT_TRIGGER=0; shift;;
    --no-pipeline)      WANT_PIPELINE=0; shift;;
    --screenshot|--shot) WANT_SHOT=1; shift;;
    --clipboard|--clip) WANT_CLIPBOARD=1; shift;;
    --clip-wayland)     WANT_CLIPBOARD=1; WANT_CLIP_WAYLAND=1; shift;;
    --kitty)            WANT_KITTY_WIN=1; shift;;
    --claude)           WANT_CLAUDE=1; shift;;
    --claude-pane)      WANT_CLAUDE=1; CLAUDE_PANE="$2"; shift 2;;
    --clip-interval)    CLIPBOARD_INTERVAL="$2"; shift 2;;
    --all)              WANT_SHOT=1; WANT_CLIPBOARD=1; WANT_KITTY_WIN=1; WANT_CLAUDE=1; shift;;
    -g|--grep)          GREP_REGEX="$2"; shift 2;;
    --debug)            DEBUG_MODE=1; shift;;
    -h|--help)
      sed -n '2,32p' "$0" | sed -e 's/^# \{0,1\}//'
      cat >&2 <<'HELP'

Extra streams (opt-in; --all enables them all):
  --screenshot, --shot     [shot]      systemd flashpaste-screenshot-watcher.service journal
  --clipboard,  --clip     [clipboard] poller — emits on every change to
                                       Wayland TARGETS or X11 TARGETS. Use to see
                                       "image was on clipboard, then text took over."
  --kitty                  [kitty]     poller — kitty windows JSON, emit on focus
                                       or count change. Use to debug "paste went
                                       to the wrong kitty window."
  --clip-interval SECS     polling interval for --clipboard / --kitty (default 1)
HELP
      exit 0;;
    *)
      echo "flashpaste-logs: unknown arg: $1" >&2
      echo "try --help" >&2
      exit 2;;
  esac
done

# ── optional: bump daemon log level to debug, with consent ───────────────
if [ "$DEBUG_MODE" = 1 ]; then
  echo "${YEL}flashpaste-logs --debug:${OFF} this will restart flashpasted with RUST_LOG=debug." >&2
  printf "continue? [y/N] " >&2
  read -r ans
  case "$ans" in
    y|Y|yes)
      mkdir -p "${HOME}/.config/systemd/user/flashpasted.service.d"
      cat >"${HOME}/.config/systemd/user/flashpasted.service.d/debug.conf" <<EOF
[Service]
Environment=RUST_LOG=debug
EOF
      systemctl --user daemon-reload
      systemctl --user restart "$UNIT"
      echo "${YEL}restarted with RUST_LOG=debug; drop file ${HOME}/.config/systemd/user/flashpasted.service.d/debug.conf to revert.${OFF}" >&2
      ;;
    *) echo "skipped." >&2;;
  esac
fi

# ── line prefixers (sed -u keeps streaming, doesn't buffer) ──────────────
# Two more colors for the new streams.
if [ -t 1 ]; then GRN=$'\e[32m' RED=$'\e[31m' WHT=$'\e[37m'; else GRN= RED= WHT=; fi
pfx_daemon()    { sed -u  "s|^|${CYN}[daemon] ${OFF}|"; }
pfx_trigger()   { sed -u  "s|^|${YEL}[trigger]${OFF} |"; }
pfx_pipeline()  { sed -u  "s|^|${MAG}[pipe]   ${OFF}|"; }
pfx_shot()      { sed -u  "s|^|${GRN}[shot]   ${OFF}|"; }
pfx_clipboard() { sed -u  "s|^|${RED}[clip]   ${OFF}|"; }
pfx_kitty()     { sed -u  "s|^|${WHT}[kitty]  ${OFF}|"; }
pfx_claude()    { sed -u  "s|^|${BOLD}${CYN}[claude] ${OFF}|"; }

# Claude TUI state poller: scan each pane running `claude` or `node` for
# the spinner pattern ("Verb-ing... (" — Claudding, Whirring, Pollinating,
# Catapulting, Garnishing, Sautéing, Cogitating, etc.). Emit a single line
# on every state transition per pane: busy → idle or idle → busy. Use to
# correlate paste failures with Claude's generation state without staring
# at the visual spinner. Note: this poller uses `tmux capture-pane` and
# does NOT touch the Wayland/X11 clipboard, so no dock flash.
#
# The detector matches the spinner present-participle pattern instead of
# the old "<N> tokens" heuristic — token counts appear in both live and
# idle status lines, so the old detector hit false-positives on every
# idle pane. The "...ing... (" form is unique to the live status.
claude_state_poller() {
  # If user didn't pin a pane, watch every pane whose current command is
  # "claude" or "node" (the two binaries Claude Code TUI runs under).
  declare -A prev=()
  while true; do
    local ts
    ts=$(date +%H:%M:%S.%3N)
    local panes
    if [ -n "$CLAUDE_PANE" ]; then
      panes="$CLAUDE_PANE"
    else
      panes=$(tmux list-panes -a -F '#{pane_id} #{pane_current_command}' 2>/dev/null \
              | awk '$2=="claude" || $2=="node" {print $1}')
    fi
    [ -z "$panes" ] && { sleep "$CLIPBOARD_INTERVAL"; continue; }
    for p in $panes; do
      # Capture the last 12 rendered lines; the status line lives near the
      # bottom but exact row depends on input-box height.
      local cap
      cap=$(tmux capture-pane -t "$p" -pS -12 2>/dev/null)
      local state="idle"
      # Match present-participle spinner followed by "... (".
      if printf '%s' "$cap" | grep -qE '\b[A-Z][a-zéüôî]+ing\.\.\.[[:space:]]*\(' ; then
        state="busy"
      fi
      local was="${prev[$p]:-unknown}"
      if [ "$state" != "$was" ]; then
        printf '%s pane=%s state=%s (was %s)\n' "$ts" "$p" "$state" "$was"
        prev[$p]=$state
      fi
    done
    sleep "$CLIPBOARD_INTERVAL"
  done
}

# Clipboard poller: emit a line whenever the X11 TARGETS list changes.
# DELIBERATELY xclip-only — invoking /usr/bin/wl-paste once per poll on
# this Mutter box makes the GNOME Shell dock flash a "wl-clipboard ready"
# entry on every iteration. The daemon's X11 owner serves the same bytes
# the Wayland clipboard would, so xclip captures every meaningful change
# without the dock noise. Pass --clip-wayland to opt back in to Wayland
# polling (only useful on wlroots / KDE / sway where the dock doesn't
# react to surfaceless wl-paste).
clipboard_poller() {
  local prev_x11="" prev_wl=""
  local ts cur_x11 cur_wl
  while true; do
    ts=$(date +%H:%M:%S.%3N)
    cur_x11=$(timeout 0.2 xclip -selection clipboard -o -t TARGETS 2>/dev/null | sort -u | tr '\n' ',' | sed 's/,$//')
    if [ "$cur_x11" != "$prev_x11" ]; then
      printf '%s x11_types=[%s] (was [%s])\n' "$ts" "${cur_x11:-<empty>}" "${prev_x11:-<empty>}"
      prev_x11=$cur_x11
    fi
    if [ "$WANT_CLIP_WAYLAND" = "1" ]; then
      cur_wl=$(timeout 0.2 /usr/bin/wl-paste --list-types 2>/dev/null | sort -u | tr '\n' ',' | sed 's/,$//')
      if [ "$cur_wl" != "$prev_wl" ]; then
        printf '%s wayland_types=[%s] (was [%s])\n' "$ts" "${cur_wl:-<empty>}" "${prev_wl:-<empty>}"
        prev_wl=$cur_wl
      fi
    fi
    sleep "$CLIPBOARD_INTERVAL"
  done
}

# Kitty window state poller: emit on focus/count change. Use to debug
# "send-text landed in the wrong window".
kitty_poller() {
  local sock prev=""
  for s in "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"/kitty-main-*; do
    [ -S "$s" ] && sock="unix:$s" && break
  done
  if [ -z "$sock" ]; then
    echo "(no kitty socket — kitty stream skipped)"
    return
  fi
  while true; do
    local ts focused count
    ts=$(date +%H:%M:%S.%3N)
    # ls returns a JSON tree of OS-windows / tabs / windows. We project
    # down to "focused id + total window count" — a low-cardinality summary
    # that still catches focus steals and tab opens.
    local cur
    cur=$(kitty @ --to "$sock" ls 2>/dev/null | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("error"); raise SystemExit
focused, active, count = None, None, 0
for ow in d:
    for tab in ow.get("tabs", []):
        for w in tab.get("windows", []):
            count += 1
            if w.get("is_focused"): focused = w.get("id")
            if w.get("is_active"):  active  = w.get("id")
print(f"focused={focused} active={active} count={count}")
' 2>/dev/null)
    if [ -n "$cur" ] && [ "$cur" != "$prev" ]; then
      printf '%s %s\n' "$ts" "$cur"
      prev=$cur
    fi
    sleep "$CLIPBOARD_INTERVAL"
  done
}

# Optional grep wrapper applied per-stream so colored prefixes survive.
maybe_grep() {
  if [ -n "$GREP_REGEX" ]; then
    grep --line-buffered -E -- "$GREP_REGEX" || true
  else
    cat
  fi
}

# ── lifecycle: kill all children on Ctrl+C / exit ────────────────────────
PIDS=()
cleanup() {
  for p in "${PIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  wait 2>/dev/null
}
trap cleanup EXIT INT TERM

# ── banner ───────────────────────────────────────────────────────────────
streams=""
[ "$WANT_DAEMON"    = 1 ] && streams+="${CYN}daemon${OFF} "
[ "$WANT_TRIGGER"   = 1 ] && streams+="${YEL}trigger${OFF} "
[ "$WANT_PIPELINE"  = 1 ] && streams+="${MAG}pipeline${OFF} "
[ "$WANT_SHOT"      = 1 ] && streams+="${GRN}shot${OFF} "
[ "$WANT_CLIPBOARD" = 1 ] && streams+="${RED}clip${OFF} "
[ "$WANT_KITTY_WIN" = 1 ] && streams+="${WHT}kitty${OFF} "
[ "$WANT_CLAUDE"    = 1 ] && streams+="${BOLD}${CYN}claude${OFF} "
follow_str="follow"; [ "$FOLLOW" = 0 ] && follow_str="snapshot"
since_str=""; [ -n "$SINCE" ] && since_str=" since='${SINCE}'"
grep_str="";  [ -n "$GREP_REGEX" ] && grep_str=" grep='${GREP_REGEX}'"
printf '%sflashpaste-logs%s streams=%s mode=%s n=%s%s%s\n' \
  "$BOLD" "$OFF" "$streams" "$follow_str" "$TAIL_N" "$since_str" "$grep_str" >&2
echo "${DIM}(Ctrl+C to stop)${OFF}" >&2

# ── stream 1: daemon journal ─────────────────────────────────────────────
if [ "$WANT_DAEMON" = 1 ]; then
  jc_args=(--user -u "$UNIT" -o short-precise --no-hostname --no-pager)
  if [ -n "$SINCE" ]; then
    jc_args+=(--since "$SINCE")
  else
    jc_args+=(-n "$TAIL_N")
  fi
  [ "$FOLLOW" = 1 ] && jc_args+=(-f)
  ( journalctl "${jc_args[@]}" 2>&1 | maybe_grep | pfx_daemon ) &
  PIDS+=($!)
fi

# ── stream 2: trigger log ────────────────────────────────────────────────
if [ "$WANT_TRIGGER" = 1 ] && [ -r "$TRIGGER_LOG" ]; then
  if [ "$FOLLOW" = 1 ]; then
    ( tail -n "$TAIL_N" -F "$TRIGGER_LOG" 2>&1 | maybe_grep | pfx_trigger ) &
  else
    ( tail -n "$TAIL_N"    "$TRIGGER_LOG" 2>&1 | maybe_grep | pfx_trigger ) &
  fi
  PIDS+=($!)
elif [ "$WANT_TRIGGER" = 1 ]; then
  echo "${DIM}(no $TRIGGER_LOG yet — trigger stream skipped)${OFF}" >&2
fi

# ── stream 3: clipboard pipeline log ─────────────────────────────────────
if [ "$WANT_PIPELINE" = 1 ] && [ -r "$PIPELINE_LOG" ]; then
  if [ "$FOLLOW" = 1 ]; then
    ( tail -n "$TAIL_N" -F "$PIPELINE_LOG" 2>&1 | maybe_grep | pfx_pipeline ) &
  else
    ( tail -n "$TAIL_N"    "$PIPELINE_LOG" 2>&1 | maybe_grep | pfx_pipeline ) &
  fi
  PIDS+=($!)
elif [ "$WANT_PIPELINE" = 1 ]; then
  echo "${DIM}(no $PIPELINE_LOG yet — pipeline stream skipped)${OFF}" >&2
fi

# ── stream 4: screenshot-watcher journal ─────────────────────────────────
if [ "$WANT_SHOT" = 1 ]; then
  sh_args=(--user -u "$SHOT_UNIT" -o short-precise --no-hostname --no-pager)
  if [ -n "$SINCE" ]; then
    sh_args+=(--since "$SINCE")
  else
    sh_args+=(-n "$TAIL_N")
  fi
  [ "$FOLLOW" = 1 ] && sh_args+=(-f)
  ( journalctl "${sh_args[@]}" 2>&1 | maybe_grep | pfx_shot ) &
  PIDS+=($!)
fi

# ── stream 5: clipboard-state poller ─────────────────────────────────────
if [ "$WANT_CLIPBOARD" = 1 ]; then
  ( clipboard_poller 2>&1 | maybe_grep | pfx_clipboard ) &
  PIDS+=($!)
fi

# ── stream 6: kitty window state poller ──────────────────────────────────
if [ "$WANT_KITTY_WIN" = 1 ]; then
  ( kitty_poller 2>&1 | maybe_grep | pfx_kitty ) &
  PIDS+=($!)
fi

# ── stream 7: Claude TUI busy/idle poller ────────────────────────────────
if [ "$WANT_CLAUDE" = 1 ]; then
  ( claude_state_poller 2>&1 | maybe_grep | pfx_claude ) &
  PIDS+=($!)
fi

if [ "${#PIDS[@]}" = 0 ]; then
  echo "flashpaste-logs: nothing to follow (all streams disabled or missing)" >&2
  exit 1
fi

wait
