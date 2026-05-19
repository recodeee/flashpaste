#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────
# flashpaste-trace.sh — aggregate the JSON trace stream from
# tmux-paste-dispatch.sh into a percentile table per checkpoint.
#
# Enable the trace stream first:
#   FLASHPASTE_TRACE=1
#
# The dispatcher writes one JSON line per `t "<step>"` call plus one
# `__exit` line per invocation to
#   ${FLASHPASTE_TRACE_LOG:-$HOME/.local/state/flashpaste-trace.jsonl}
#
# Usage:
#   flashpaste-trace.sh                     # last 100 invocations
#   flashpaste-trace.sh --last N            # last N invocations
#   flashpaste-trace.sh --since <iso-utc>   # filter by ts >= value
#   flashpaste-trace.sh --tail              # follow + regroup
#   flashpaste-trace.sh --raw               # cat the JSONL file
#
# Percentiles are computed across delta_ms per step, EXCEPT __exit
# which uses t_ms (per-invocation total).
# ─────────────────────────────────────────────────────────────────────
set -euo pipefail

LOG="${FLASHPASTE_TRACE_LOG:-$HOME/.local/state/flashpaste-trace.jsonl}"

LAST=100
SINCE=""
MODE="summary"

while [ $# -gt 0 ]; do
  case "$1" in
    --last)
      shift
      [ $# -gt 0 ] || { printf 'flashpaste-trace: --last requires a value\n' >&2; exit 2; }
      LAST=$1
      ;;
    --since)
      shift
      [ $# -gt 0 ] || { printf 'flashpaste-trace: --since requires a value\n' >&2; exit 2; }
      SINCE=$1
      ;;
    --tail)
      MODE="tail"
      ;;
    --raw)
      MODE="raw"
      ;;
    -h|--help)
      sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      printf 'flashpaste-trace: unknown arg: %s\n' "$1" >&2
      exit 2
      ;;
  esac
  shift
done

if [ ! -f "$LOG" ]; then
  printf 'flashpaste-trace: no log at %s\n' "$LOG" >&2
  printf 'flashpaste-trace: set FLASHPASTE_TRACE=1 and trigger a paste first\n' >&2
  exit 1
fi

if [ "$MODE" = "raw" ]; then
  exec cat "$LOG"
fi

# ─── tail mode ─────────────────────────────────────────────────────
# Group rows on the fly. Each __exit line flushes the in-flight group
# and prints a one-line summary for that invocation.
if [ "$MODE" = "tail" ]; then
  printf 'flashpaste-trace: tailing %s (Ctrl-C to stop)\n' "$LOG" >&2
  tail -n 0 -F "$LOG" 2>/dev/null | awk '
    function field(line, key,    re, m) {
      re = "\"" key "\":\"[^\"]*\""
      if (match(line, re)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^"[^"]+":"/, "", m)
        sub(/"$/, "", m)
        return m
      }
      return ""
    }
    function num_field(line, key,    re, m) {
      re = "\"" key "\":[0-9]+"
      if (match(line, re)) {
        m = substr(line, RSTART, RLENGTH)
        sub(/^"[^"]+":/, "", m)
        return m + 0
      }
      return 0
    }
    {
      tr = field($0, "trace")
      step = field($0, "step")
      tms = num_field($0, "t_ms")
      dms = num_field($0, "delta_ms")
      if (tr == "") next
      if (step == "__exit") {
        rc = num_field($0, "exit")
        steps = (groups[tr] != "" ? groups[tr] : "(no checkpoints)")
        printf "%s  total=%dms rc=%d  steps=[%s]\n", tr, tms, rc, steps
        delete groups[tr]
        fflush()
      } else {
        if (groups[tr] != "") groups[tr] = groups[tr] " "
        groups[tr] = groups[tr] step "+" dms
      }
    }
  '
  exit 0
fi

# ─── summary mode ──────────────────────────────────────────────────
# Strategy:
#   1. Optional --since filter (string compare on ISO ts is fine).
#   2. Find the last $LAST distinct trace ids (in file order).
#   3. Stream rows for those trace ids into the percentile awk.

TMP_FILTERED=$(mktemp -t flashpaste-trace.XXXXXX)
trap 'rm -f "$TMP_FILTERED"' EXIT

if [ -n "$SINCE" ]; then
  awk -v since="$SINCE" '
    {
      if (match($0, /"ts":"[^"]+"/)) {
        ts = substr($0, RSTART+6, RLENGTH-7)
        if (ts >= since) print
      }
    }
  ' "$LOG" >"$TMP_FILTERED"
else
  cp "$LOG" "$TMP_FILTERED"
fi

# Pick last N distinct trace ids.
TMP_KEEP=$(mktemp -t flashpaste-trace-keep.XXXXXX)
trap 'rm -f "$TMP_FILTERED" "$TMP_KEEP"' EXIT

awk '
  {
    if (match($0, /"trace":"[^"]+"/)) {
      tr = substr($0, RSTART+9, RLENGTH-10)
      if (!(tr in seen)) { seen[tr] = ++order; ids[order] = tr }
    }
  }
  END {
    for (i = 1; i <= order; i++) print ids[i]
  }
' "$TMP_FILTERED" | tail -n "$LAST" >"$TMP_KEEP"

KEEP_COUNT=$(wc -l <"$TMP_KEEP" | tr -d ' ')
if [ "$KEEP_COUNT" -eq 0 ]; then
  printf 'flashpaste-trace: no invocations matched\n' >&2
  exit 1
fi

# Percentile pass. Prefer gawk when present (true multi-dim arrays).
AWK_BIN='awk'
if command -v gawk >/dev/null 2>&1; then
  AWK_BIN='gawk'
fi

"$AWK_BIN" -v keep_count="$KEEP_COUNT" -v keepfile="$TMP_KEEP" '
  BEGIN {
    while ((getline line < keepfile) > 0) keep[line] = 1
    close(keepfile)
  }
  function trace_id(line,    m) {
    if (match(line, /"trace":"[^"]+"/)) {
      m = substr(line, RSTART+9, RLENGTH-10)
      return m
    }
    return ""
  }
  function step_name(line,    m) {
    if (match(line, /"step":"[^"]+"/)) {
      m = substr(line, RSTART+8, RLENGTH-9)
      return m
    }
    return ""
  }
  function num_field(line, key,    re, m) {
    re = "\"" key "\":[0-9]+"
    if (match(line, re)) {
      m = substr(line, RSTART, RLENGTH)
      sub(/^"[^"]+":/, "", m)
      return m + 0
    }
    return 0
  }
  {
    tr = trace_id($0)
    if (tr == "" || !(tr in keep)) next
    s = step_name($0)
    if (s == "") next
    if (s == "__exit") {
      ms = num_field($0, "t_ms")
    } else {
      ms = num_field($0, "delta_ms")
    }
    n[s]++
    # Flat array keyed "<step>|<index>" — works in posix awk too.
    arr[s "|" n[s]] = ms
    if (!(s in seen_step)) { seen_step[s] = ++step_order; steps[step_order] = s }
  }
  function pct(s, p,    i, vals, cnt, sorted, idx) {
    cnt = n[s]
    for (i = 1; i <= cnt; i++) vals[i] = arr[s "|" i]
    # insertion sort — n is bounded by --last (default 100)
    for (i = 2; i <= cnt; i++) {
      key = vals[i]
      j = i - 1
      while (j >= 1 && vals[j] > key) { vals[j+1] = vals[j]; j-- }
      vals[j+1] = key
    }
    idx = int(cnt * p + 0.5)
    if (idx < 1) idx = 1
    if (idx > cnt) idx = cnt
    return vals[idx]
  }
  END {
    printf "flashpaste-trace summary - %d invocations\n", keep_count
    print  "---------------------------------------------------------------"
    printf "%-30s %7s %7s %7s %7s\n", "step", "p50", "p90", "p99", "count"
    for (i = 1; i <= step_order; i++) {
      s = steps[i]
      label = s
      if (s == "__exit") label = "__exit (total)"
      printf "%-30s %7d %7d %7d %7d\n", label, pct(s, 0.50), pct(s, 0.90), pct(s, 0.99), n[s]
    }
    print  "---------------------------------------------------------------"
  }
' "$TMP_FILTERED"
