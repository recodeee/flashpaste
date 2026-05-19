#!/usr/bin/env bash
# Sourceable logging helper for the clipboard pipeline.
# Every clipboard/paste script sources this so they all write to the same
# log file with a consistent format. Tail with:
#   tail -F ~/.local/state/clipboard-pipeline.log

CLIP_PIPELINE_LOG="${CLIP_PIPELINE_LOG:-$HOME/.local/state/clipboard-pipeline.log}"
mkdir -p "$(dirname "$CLIP_PIPELINE_LOG")" 2>/dev/null

# clog <script-tag> <event-key=val ...>
# Example: clog set "event=write" "bytes=42" "preview='hello world'"
clog() {
  local tag="$1"; shift
  printf '[%s] %-22s pid=%-6s %s\n' \
    "$(date '+%H:%M:%S.%3N')" "$tag" "$$" "$*" \
    >>"$CLIP_PIPELINE_LOG" 2>/dev/null
}

# clog_preview <var> — print first 100 chars with newlines escaped, for log lines.
# Usage: clog set "data='$(clog_preview "$buf")'"
clog_preview() {
  printf '%s' "${1:-}" | head -c 100 | tr '\n\r\t' '   '
}
