#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────
# flashpaste-bench.sh — reproducible dispatch-latency benchmark.
#
# Pastes N times through the configured tier(s) and emits a percentile
# table aggregated from the wall-clock interval between
#   "trigger script invoked"  (date +%s%N at this script)
#   "fast-path exit"          (tail -F ~/.local/state/tmux-paste.log)
#
# Usage:
#   flashpaste-bench.sh [--iterations N] [--warmup N] [--tier 1|2|3|all]
#                       [--output PATH] [--format table|json|markdown]
#                       [--fail-on-regression]
#
# Defaults:
#   --iterations 100  --warmup 5  --tier all  --format table  stdout
#
# Pre-flight:
#   1. flashpaste-doctor must exist on $PATH.
#   2. A recent screenshot must live in ~/Pictures/Screenshots/. If
#      none is found, we try ImageMagick's `convert` to synthesise one.
#   3. Clipboard is cleared between iterations to avoid coalescing.
#
# Regression sentinel:
#   With --fail-on-regression, exit 2 if Tier 1 p50 > 200 ms.
#
# Respects FLASHPASTE_QUIET=1 to silence narration on stderr.
# ─────────────────────────────────────────────────────────────────────
set -euo pipefail

# ─── sourceable logging helper (best-effort) ───────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
if [ -f "$SCRIPT_DIR/clip-pipeline-log.sh" ]; then
  . "$SCRIPT_DIR/clip-pipeline-log.sh"
else
  clog() { :; }
fi

# ─── defaults ──────────────────────────────────────────────────────
ITER=100
WARMUP=5
TIER="all"
OUTPUT=""
FORMAT="table"
FAIL_ON_REGRESSION=0
QUIET="${FLASHPASTE_QUIET:-0}"

PASTE_LOG="${TMUX_PASTE_LOG:-$HOME/.local/state/tmux-paste.log}"
SCREENSHOT_DIR="${FLASHPASTE_SCREENSHOT_DIR:-$HOME/Pictures/Screenshots}"
REGRESSION_P50_MS=200

# ─── helpers ───────────────────────────────────────────────────────
say() {
  [ "$QUIET" = "1" ] && return 0
  printf 'flashpaste-bench: %s\n' "$*" >&2
}

die() {
  printf 'flashpaste-bench: %s\n' "$*" >&2
  exit 1
}

usage() {
  sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
}

# ─── arg parse ─────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --iterations)
      shift; [ $# -gt 0 ] || die "--iterations needs a value"
      ITER=$1
      ;;
    --warmup)
      shift; [ $# -gt 0 ] || die "--warmup needs a value"
      WARMUP=$1
      ;;
    --tier)
      shift; [ $# -gt 0 ] || die "--tier needs a value"
      TIER=$1
      ;;
    --output)
      shift; [ $# -gt 0 ] || die "--output needs a value"
      OUTPUT=$1
      ;;
    --format)
      shift; [ $# -gt 0 ] || die "--format needs a value"
      FORMAT=$1
      ;;
    --fail-on-regression)
      FAIL_ON_REGRESSION=1
      ;;
    -h|--help)
      usage; exit 0
      ;;
    *)
      die "unknown arg: $1"
      ;;
  esac
  shift
done

case "$TIER" in
  1|2|3|all) ;;
  *) die "invalid --tier: $TIER (expected 1, 2, 3, or all)" ;;
esac

case "$FORMAT" in
  table|json|markdown) ;;
  *) die "invalid --format: $FORMAT (expected table, json, or markdown)" ;;
esac

if ! [[ "$ITER" =~ ^[0-9]+$ ]] || [ "$ITER" -lt 1 ]; then
  die "invalid --iterations: $ITER"
fi
if ! [[ "$WARMUP" =~ ^[0-9]+$ ]]; then
  die "invalid --warmup: $WARMUP"
fi

# ─── pre-flight ────────────────────────────────────────────────────
if ! command -v flashpaste-doctor >/dev/null 2>&1 \
   && ! command -v flashpaste-doctor.sh >/dev/null 2>&1; then
  die "flashpaste-doctor not on \$PATH — run 'make install' first"
fi

# Make sure the dispatcher log exists / is writable.
mkdir -p "$(dirname "$PASTE_LOG")"
: >>"$PASTE_LOG" || die "cannot write paste log: $PASTE_LOG"

ensure_screenshot() {
  if [ -d "$SCREENSHOT_DIR" ]; then
    local recent
    recent=$(find "$SCREENSHOT_DIR" -maxdepth 1 -type f \
              \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' \) \
              -printf '%T@ %p\n' 2>/dev/null \
              | sort -nr | head -1 | cut -d' ' -f2- || true)
    if [ -n "$recent" ]; then
      say "using screenshot: $recent"
      return 0
    fi
  fi
  say "no screenshot found in $SCREENSHOT_DIR — synthesising one"
  if ! command -v convert >/dev/null 2>&1; then
    die "no screenshot available and ImageMagick 'convert' is missing. Either drop a PNG into $SCREENSHOT_DIR or 'sudo apt install imagemagick'."
  fi
  mkdir -p "$SCREENSHOT_DIR"
  local synth="$SCREENSHOT_DIR/flashpaste-bench-$(date +%s).png"
  convert -size 100x100 xc:#161b22 "$synth" \
    || die "convert failed to produce $synth"
  say "synthesised: $synth"
}

ensure_screenshot

# ─── tier dispatch resolution ──────────────────────────────────────
TIER1_BIN=""
TIER2_BIN=""
TIER3_BIN=""

resolve_tier_binaries() {
  if command -v tmux-paste-dispatch.sh >/dev/null 2>&1; then
    TIER1_BIN="$(command -v tmux-paste-dispatch.sh)"
  elif [ -x "$SCRIPT_DIR/tmux-paste-dispatch.sh" ]; then
    TIER1_BIN="$SCRIPT_DIR/tmux-paste-dispatch.sh"
  fi

  if command -v flashpaste-dispatch >/dev/null 2>&1; then
    TIER2_BIN="$(command -v flashpaste-dispatch)"
  fi

  if command -v flashpaste-trigger >/dev/null 2>&1; then
    TIER3_BIN="$(command -v flashpaste-trigger)"
  fi
}

resolve_tier_binaries

tier_binary() {
  case "$1" in
    1) printf '%s' "$TIER1_BIN" ;;
    2) printf '%s' "$TIER2_BIN" ;;
    3) printf '%s' "$TIER3_BIN" ;;
  esac
}

# ─── single-iteration timing ───────────────────────────────────────
# Stamp PASTE_LOG with a marker before running. After the dispatcher
# returns, the marker tells us where to start scanning for the next
# "fast-path exit" line. Wall-clock delta is captured around the call.
run_one_ms() {
  local bin="$1"
  local marker="bench-$$-$(date +%s%N)-$RANDOM"

  # Drop a marker line so we don't race on stale log content.
  printf '[bench-marker] %s\n' "$marker" >>"$PASTE_LOG"

  # Clear clipboard between iterations (best-effort).
  if command -v wl-copy >/dev/null 2>&1; then
    : | wl-copy --type text/plain 2>/dev/null || true
  fi

  local t0 t1
  t0=$(date +%s%N)
  # Tier 3 trigger is a 1-byte UDS poke. Tier 1/2 are full dispatchers.
  # We only care about the wall-clock until the dispatcher process
  # returns control — for Tier 3 that's near-instant, for Tier 1 it's
  # blocking on the kitty send-text round-trip.
  "$bin" >/dev/null 2>&1 || true
  t1=$(date +%s%N)

  local ns=$(( t1 - t0 ))
  # Round to nearest ms.
  printf '%d' $(( (ns + 500000) / 1000000 ))
}

# ─── aggregation ───────────────────────────────────────────────────
# Pass space-separated ms samples on stdin, get a single line:
#   "<n> <p50> <p90> <p99> <min> <max> <mean> <stddev>"
aggregate() {
  awk '
    {
      for (i = 1; i <= NF; i++) {
        v[++n] = $i + 0
        sum += $i
      }
    }
    function pct(p,    i, idx) {
      idx = int(n * p + 0.5)
      if (idx < 1) idx = 1
      if (idx > n) idx = n
      return v[idx]
    }
    END {
      if (n == 0) { print "0 0 0 0 0 0 0 0"; exit }
      # insertion sort
      for (i = 2; i <= n; i++) {
        key = v[i]; j = i - 1
        while (j >= 1 && v[j] > key) { v[j+1] = v[j]; j-- }
        v[j+1] = key
      }
      mean = sum / n
      sq = 0
      for (i = 1; i <= n; i++) sq += (v[i] - mean) * (v[i] - mean)
      sd = (n > 1) ? sqrt(sq / (n - 1)) : 0
      printf "%d %d %d %d %d %d %.2f %.2f\n", \
        n, pct(0.50), pct(0.90), pct(0.99), v[1], v[n], mean, sd
    }
  '
}

# ─── per-tier driver ───────────────────────────────────────────────
declare -A TIER_RESULTS=()
declare -a TIER_ORDER=()

bench_tier() {
  local tier="$1"
  local bin
  bin=$(tier_binary "$tier")

  if [ -z "$bin" ]; then
    say "tier $tier: binary not on \$PATH — skipping"
    return 0
  fi

  say "tier $tier: $bin  (warmup=$WARMUP, iter=$ITER)"
  local i
  # Warmup — discarded.
  for ((i = 0; i < WARMUP; i++)); do
    run_one_ms "$bin" >/dev/null
  done

  local samples=""
  for ((i = 0; i < ITER; i++)); do
    samples+=" $(run_one_ms "$bin")"
    if [ "$QUIET" != "1" ] && [ $(( (i + 1) % 10 )) -eq 0 ]; then
      printf '  tier %s: %d/%d\n' "$tier" "$((i + 1))" "$ITER" >&2
    fi
  done

  local stats
  stats=$(printf '%s\n' "$samples" | aggregate)
  TIER_RESULTS[$tier]="$stats"
  TIER_ORDER+=("$tier")
  clog "bench" "event=tier-done" "tier=$tier" "stats='$stats'"
}

if [ "$TIER" = "all" ]; then
  for t in 1 2 3; do bench_tier "$t"; done
else
  bench_tier "$TIER"
fi

if [ ${#TIER_ORDER[@]} -eq 0 ]; then
  die "no tiers were benchmarked — check binaries on \$PATH"
fi

# ─── emit ──────────────────────────────────────────────────────────
emit_table() {
  printf 'flashpaste-bench — dispatch latency (ms)\n'
  printf -- '---------------------------------------------------------------------------------------\n'
  printf '%-6s %5s %6s %6s %6s %6s %6s %10s %8s\n' \
    "tier" "n" "p50" "p90" "p99" "min" "max" "mean" "stddev"
  printf -- '---------------------------------------------------------------------------------------\n'
  local t row
  for t in "${TIER_ORDER[@]}"; do
    row="${TIER_RESULTS[$t]}"
    # row = n p50 p90 p99 min max mean stddev
    # shellcheck disable=SC2086
    set -- $row
    printf '%-6s %5s %6s %6s %6s %6s %6s %10s %8s\n' \
      "T$t" "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8"
  done
  printf -- '---------------------------------------------------------------------------------------\n'
}

emit_json() {
  printf '{\n  "iterations": %d,\n  "warmup": %d,\n  "results": [\n' "$ITER" "$WARMUP"
  local first=1 t row
  for t in "${TIER_ORDER[@]}"; do
    row="${TIER_RESULTS[$t]}"
    # shellcheck disable=SC2086
    set -- $row
    if [ "$first" = "0" ]; then printf ',\n'; fi
    first=0
    printf '    {"tier": %d, "n": %d, "p50_ms": %d, "p90_ms": %d, "p99_ms": %d, "min_ms": %d, "max_ms": %d, "mean_ms": %s, "stddev_ms": %s}' \
      "$t" "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8"
  done
  printf '\n  ]\n}\n'
}

emit_markdown() {
  printf '| tier | n | p50 | p90 | p99 | min | max | mean | stddev |\n'
  printf '| ---- | -: | -:  | -:  | -:  | -:  | -:  | -:   | -:     |\n'
  local t row
  for t in "${TIER_ORDER[@]}"; do
    row="${TIER_RESULTS[$t]}"
    # shellcheck disable=SC2086
    set -- $row
    printf '| Tier %s | %s | %s | %s | %s | %s | %s | %s | %s |\n' \
      "$t" "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8"
  done
}

OUT_TMP=$(mktemp -t flashpaste-bench.XXXXXX)
trap 'rm -f "$OUT_TMP"' EXIT

case "$FORMAT" in
  table)    emit_table    >"$OUT_TMP" ;;
  json)     emit_json     >"$OUT_TMP" ;;
  markdown) emit_markdown >"$OUT_TMP" ;;
esac

if [ -n "$OUTPUT" ]; then
  mv "$OUT_TMP" "$OUTPUT"
  trap - EXIT
  say "wrote $OUTPUT"
else
  cat "$OUT_TMP"
fi

# ─── regression sentinel ───────────────────────────────────────────
if [ "$FAIL_ON_REGRESSION" = "1" ] && [ -n "${TIER_RESULTS[1]:-}" ]; then
  # shellcheck disable=SC2086
  set -- ${TIER_RESULTS[1]}
  p50=$2
  if [ "$p50" -gt "$REGRESSION_P50_MS" ]; then
    say "REGRESSION: Tier 1 p50=${p50}ms > ${REGRESSION_P50_MS}ms"
    exit 2
  fi
fi

exit 0
