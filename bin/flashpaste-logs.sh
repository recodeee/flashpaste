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
  CYN= YEL= MAG= DIM= OFF= BOLD=
fi

# ── defaults ─────────────────────────────────────────────────────────────
SINCE=""                # passed to journalctl when set
TAIL_N=50               # initial backfill on log files
FOLLOW=1
WANT_DAEMON=1
WANT_TRIGGER=1
WANT_PIPELINE=1
GREP_REGEX=""
DEBUG_MODE=0

TRIGGER_LOG="${HOME}/.local/state/flashpaste-trigger.log"
PIPELINE_LOG="${HOME}/.local/state/clipboard-pipeline.log"
UNIT="flashpasted.service"

# ── arg parse ────────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --since)        SINCE="$2"; shift 2;;
    -n|--lines)     TAIL_N="$2"; shift 2;;
    --no-follow|-1) FOLLOW=0; shift;;
    --no-daemon)    WANT_DAEMON=0; shift;;
    --no-trigger)   WANT_TRIGGER=0; shift;;
    --no-pipeline)  WANT_PIPELINE=0; shift;;
    -g|--grep)      GREP_REGEX="$2"; shift 2;;
    --debug)        DEBUG_MODE=1; shift;;
    -h|--help)
      sed -n '2,32p' "$0" | sed -e 's/^# \{0,1\}//'
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
pfx_daemon()   { sed -u  "s|^|${CYN}[daemon] ${OFF}|"; }
pfx_trigger()  { sed -u  "s|^|${YEL}[trigger]${OFF} |"; }
pfx_pipeline() { sed -u  "s|^|${MAG}[pipe]   ${OFF}|"; }

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
[ "$WANT_DAEMON"   = 1 ] && streams+="${CYN}daemon${OFF} "
[ "$WANT_TRIGGER"  = 1 ] && streams+="${YEL}trigger${OFF} "
[ "$WANT_PIPELINE" = 1 ] && streams+="${MAG}pipeline${OFF} "
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

if [ "${#PIDS[@]}" = 0 ]; then
  echo "flashpaste-logs: nothing to follow (all streams disabled or missing)" >&2
  exit 1
fi

wait
