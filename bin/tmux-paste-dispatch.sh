#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────
# WORKING VERSION: v1.0 — 2026-05-19
#
# Verified end-to-end on this box (GNOME 46 / mutter / Wayland, kitty +
# GNOME Terminal + tmux): both text paste AND image paste attach
# correctly inside Claude Code. Companion pieces required for this to
# work as a system:
#   - ~/.local/bin/wl-paste            (xclip-fallback shim, also v1.0)
#   - ~/paste_image.sh                 (kitty Ctrl+V/Ctrl+Alt+V helper, v1.0)
#   - ~/.config/systemd/user/ydotoold.service  (must symlink the socket)
#   - clipboard-poll.service must stay DISABLED (it overwrites clipboard)
#
# Do NOT revert the following without a regression test:
#   * ydotool key/type uses 0.1.8 name-syntax (`ctrl+v`), NOT `29:1 47:1`
#   * Image branch in kitty uses `kitty @ send-text --stdin` (NOT
#     ydotool), because kitty's `map ctrl+v` would otherwise intercept
#     a synthesized keystroke before it reaches the inner TUI
#   * Auto-pickup of fresh ~/Pictures/Screenshots/ files (PrtScr saves,
#     never copies — without auto-pickup the clipboard stays empty)
#   * Wayland-authoritative has_image policy (stale X11 mirror is a
#     known mutter wedge; trusting both sides re-creates obs 6881's
#     "GitHub URL → [Image #9]" bug)
# Edit log:
#   2026-05-19  ydotool 0.1.8 syntax fix + image bridge + auto-pickup +
#               kitty send-text path. Confirmed working with screenshot
#               paste into Claude Code via tmux right-click → Paste.
#   2026-05-19  v1.1 — gate auto-pickup on (a) clipboard genuinely empty
#               (no text on either side), (b) screenshot mtime newer
#               than last-auto-pickup state file. Without these guards
#               every Ctrl+V within 30s re-attached the same screenshot
#               and any browser text-copy got clobbered by stale images.
#   2026-05-19  v1.2 — restore `kitty @ send-text` for the image branch
#               (proven to trigger Claude's image-paste handler; tmux
#               send-keys does NOT, verified log 11:20:17). Added a
#               1.5s recursion guard so the re-entrant invocation from
#               tmux's `bind -n C-v` is a no-op instead of consuming
#               the paste-once clipboard.
#   2026-05-19  v1.3 — IMPORTANT FIX: temporarily `tmux unbind -n C-v`
#               before sending the synthesized Ctrl-V via kitty @
#               send-text, rebind 0.6s later. Without the unbind tmux's
#               root C-v binding consumes the synthesized byte before
#               it can reach Claude Code's pty, so the image-paste
#               handler never fires. v1.2's recursion guard helped but
#               wasn't enough — the byte still got eaten.
#   2026-05-19  v1.4 — rebind via `setsid -f sh -c 'sleep 0.1; tmux
#               bind ...'`. v1.3's backgrounded `( ... ) &` subshell
#               got SIGHUPed when the dispatch script exited, leaving
#               C-v unbound permanently after the first paste. setsid
#               -f detaches into a new session so the rebind survives.
#               100ms sleep is enough for tmux to process the in-
#               flight Ctrl-V byte; user perceives no delay before
#               next paste is ready (down from v1.3's 1-second wait).
#   2026-05-19  v1.5 — fix "needs 3 paste presses" bug by pre-staging
#               the image into xclip before unbind+send-text. Root
#               cause (per parallel agent investigation): screenshot
#               source is `wl-copy --paste-once`, which serves exactly
#               ONE clipboard read then exits. The dispatch script's
#               own probes (clip_types_wl, wl_text, diagnostic
#               snapshot) consume that one read, so Claude Code's
#               subsequent `wl-paste -t image/png` finds the clipboard
#               empty. Re-piping wl-paste into xclip -i installs an
#               unlimited-reads owner before Claude's read happens.
#   2026-05-19  v1.6 — two fixes:
#               (a) pre-stage now writes wl-paste output to a tmp file
#                   SYNCHRONOUSLY, then `xclip -i file`. The async
#                   `wl-paste | xclip -i` pipe didn't always finish
#                   before send-text fired (kept needing 3 presses).
#                   File-based load means xclip is the durable owner
#                   the instant we sleep 150ms.
#               (b) removed the `last_attached` mtime gate that
#                   blocked re-pickup of the same screenshot. The
#                   user wants to paste the same image multiple times
#                   in a row. The 2s recursion guard at the top
#                   already prevents the rapid-fire flood case.
#   2026-05-19  v1.7 — REAL fix for "needs 3 presses": move screenshot
#               pre-load to the VERY BEGINNING of the script, before
#               any wl-paste/wl-paste --list-types probe. Root cause
#               confirmed by log 11:49:57 — `wl-paste --list-types`
#               saw `image/png`, but 35ms later `wl-paste -t image/png`
#               returned 0 bytes. The dispatch's own probes were
#               draining wl-copy --paste-once before the prestage
#               could read. By loading the screenshot file into xclip
#               first, the probes hit xclip's unlimited-reads owner
#               instead of the drainable Wayland daemon.
#   2026-05-19  v1.8 — CRITICAL bug fix: `img_mime` was being computed
#               as `printf '%s' "$clip_types_x$clip_types_wl"` — that
#               smashes "TARGETS,image/png" + "TARGETS,image/png" into
#               "TARGETS,image/pngTARGETS,image/png" with no newline,
#               so the grep matched `image/pngTARGETS` as the
#               "image/*" mime. xclip then stored the bytes under
#               that frankenmime, and Claude Code's `wl-paste -t
#               image/png` couldn't find anything. Each press
#               concatenated again, producing `image/pngTARGETSTARGETS`
#               etc. Now joins with a newline AND validates with
#               `grep -E '^image/[a-z][a-z0-9.+-]*$'` so any
#               frankenmime is rejected.
#   2026-05-19  v1.9 — FAST PATH: when early-preload succeeds, skip
#               every probe and prestage, jump straight to send-text.
#               Cuts dispatch latency from ~410ms to ~120ms. Also
#               instrumented every checkpoint with the `t` helper so
#               future regressions surface as visible `Δ###ms` deltas
#               in tmux-paste.log. Reduced early-preload sleep from
#               120ms to 50ms (xclip claims selection in microseconds;
#               120 was excessive).
# ─────────────────────────────────────────────────────────────────────
# Tmux right-click "Paste" — terminal-aware paste dispatcher.
#
# Image short-circuit (checked first):
#   If the system clipboard advertises an image/* MIME type, no terminal's
#   native text-paste path can carry it. Instead synthesize a plain Ctrl+V
#   via ydotool so Claude Code's TUI catches it and reads the image off
#   the clipboard itself (wl-paste -t image/png).
#
# Otherwise, per-terminal text-paste strategy:
#   Kitty: kitty @ action paste_from_clipboard on focused window.
#          Reads clipboard via Kitty's own focused Wayland connection
#          (no wl-paste, no flash, no focus-gating hang).
#   Other (GNOME Terminal, etc.): simulate Ctrl+Shift+V via ydotool
#          so the host terminal does its own native paste.
#   Neither available: fall back to tmux send-keys C-v.
#
# Argument: target pane id (e.g. `%4`). Required.

set -u

# --- logging ---------------------------------------------------------
readonly LOG="${TMUX_PASTE_LOG:-$HOME/.local/state/tmux-paste.log}"
mkdir -p "$(dirname "$LOG")" 2>/dev/null
. /home/deadpool/.local/bin/clip-pipeline-log.sh 2>/dev/null || true
type clog >/dev/null 2>&1 || clog() { :; }
log() {
  printf '[%s] pid=%s pane=%s trigger=%s :: %s\n' \
    "$(date '+%Y-%m-%d %H:%M:%S.%3N')" "$$" "${pane:-?}" "${TMUX_PASTE_TRIGGER:-pane-menu}" "$*" >>"$LOG"
  clog "paste-dispatch" "pane='${pane:-?}'" "trigger='${TMUX_PASTE_TRIGGER:-pane-menu}'" "$*"
}

# Millisecond timing. Each `t <checkpoint>` logs ms-since-start AND
# delta-from-previous, so the log shows where time is being spent.
# Uses bash 5's $EPOCHREALTIME (seconds.microseconds) which avoids the
# fork cost of `date`.
_T_START_MS=
_T_PREV_MS=
t() {
  local now_ms epoch
  if [ -n "${EPOCHREALTIME:-}" ]; then
    epoch=${EPOCHREALTIME//./}
    now_ms=$((epoch / 1000))
  else
    now_ms=$(($(date +%s%N) / 1000000))
  fi
  if [ -z "$_T_START_MS" ]; then
    _T_START_MS=$now_ms
    _T_PREV_MS=$now_ms
  fi
  local total=$((now_ms - _T_START_MS))
  local delta=$((now_ms - _T_PREV_MS))
  _T_PREV_MS=$now_ms
  printf '[%s] T+%4dms (Δ%3dms) :: %s\n' "$(date '+%H:%M:%S.%3N')" "$total" "$delta" "$*" >>"$LOG"
  clog "paste-dispatch" "event=timing" "total_ms=$total" "delta_ms=$delta" "step='$*'"
}
# --------------------------------------------------------------------

pane="${1:-}"
t "script-start argv='$*'"
log "invoked argv='$*'"
if [ -z "$pane" ]; then
  log "ERROR no pane arg"
  tmux display-message -d 1500 'paste: no pane arg'
  exit 1
fi

# Recursion guard. The image branch sends \026 via `kitty @ send-text`,
# which travels kitty → tmux. Tmux has `bind -n C-v ... tmux-paste-
# dispatch.sh`, so that \026 re-fires the binding and re-invokes THIS
# script. Without the guard the second invocation would consume the
# paste-once clipboard owner before Claude Code's wl-paste shim does
# its real read, and the image attachment fails. A simple mtime-based
# lock with a 1.5s window covers a normal paste round-trip.
RECURSION_LOCK="${XDG_RUNTIME_DIR:-/tmp}/tmux-paste-dispatch.lock"
if [ -e "$RECURSION_LOCK" ]; then
  lock_age=$(($(date +%s) - $(stat -c %Y "$RECURSION_LOCK" 2>/dev/null || echo 0)))
  if [ "$lock_age" -lt 2 ]; then
    log "recursion guard tripped (lock age=${lock_age}s) — exiting"
    clog "paste-dispatch" "event=recursion-guard-trip" "pane='$pane'" "lock_age_s=$lock_age"
    exit 0
  fi
fi
: >"$RECURSION_LOCK" 2>/dev/null
# IMPORTANT: do NOT remove the lock on exit. Let it age out at the
# 2-second mark via mtime check above. If we removed it on EXIT, the
# recursive invocation that fires from kitty send-text would see no
# lock (because the parent already finished and removed it) and proceed
# to re-paste. Background-detached cleanup keeps the lock around long
# enough to suppress the immediate recursion.
( sleep 3; rm -f "$RECURSION_LOCK" 2>/dev/null ) &
disown 2>/dev/null || true
t "recursion-guard-passed"

tmux select-pane -t "$pane" 2>/dev/null || true
t "select-pane"

# ────────────────────────────────────────────────────────────────────
# EARLY SCREENSHOT PRE-LOAD — runs BEFORE any wl-paste probe so the
# clipboard's `wl-copy --paste-once` daemon (which exits after one
# receive) can't get drained by our own probes.
#
# Logic:
#   1. Check ~/Pictures/Screenshots/ for a file written in the last 30s.
#   2. If found AND xclip's text clipboard is empty (so we're not
#      clobbering a fresh browser text copy), load the screenshot into
#      xclip via `setsid -f xclip -i FILE`. xclip serves unlimited reads,
#      so the subsequent probes can hit it without draining.
#   3. This makes the FIRST press attach the image. Without this the
#      probes drain wl-copy --paste-once and the image-prestage block
#      below reads 0 bytes — the "needs 3 presses" bug.
# ────────────────────────────────────────────────────────────────────
_early_loaded=0
_early_mime=image/png
_early_ss_dir="$HOME/Pictures/Screenshots"
if [ -d "$_early_ss_dir" ]; then
  _early_latest=$(find "$_early_ss_dir" -maxdepth 1 -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' \) -mmin -1 -printf "%T@ %p\n" 2>/dev/null | sort -nr | sed -n '1s/^[^ ]* //p')
  if [ -n "$_early_latest" ]; then
    _early_mtime=$(stat -c %Y "$_early_latest" 2>/dev/null || echo 0)
    _early_age=$(($(date +%s) - _early_mtime))
    if [ "$_early_age" -le 30 ]; then
      _early_xt=$(timeout 0.15 xclip -selection clipboard -t text/plain -o 2>/dev/null | head -c 8)
      # Treat a "text" response that's actually PNG header bytes as
      # "no text" — xclip serves whatever's at text/plain target, and
      # if the prior owner was image-only, X may return image bytes
      # instead of nothing. The PNG magic is 0x89 P N G.
      case "$_early_xt" in
        $'\x89PNG'*) _early_xt= ;;
      esac
      if [ -z "$_early_xt" ]; then
        case "$_early_latest" in
          *.png|*.PNG) _early_mime=image/png ;;
          *.jpg|*.JPG|*.jpeg|*.JPEG) _early_mime=image/jpeg ;;
          *) _early_mime=image/png ;;
        esac
        t "early-preload before-xclip"
        setsid -f xclip -selection clipboard -t "$_early_mime" -i "$_early_latest" >/dev/null 2>&1 &
        sleep 0.05  # xclip claims selection on fork — 50ms is plenty
        _early_loaded=1
        clog "paste-dispatch" "event=early-preload" "path='$_early_latest'" "age_s=$_early_age" "mime='$_early_mime'"
        log "early-preload: $_early_latest (${_early_age}s old) -> xclip"
        t "early-preload after-sleep"
      else
        clog "paste-dispatch" "event=early-preload-skipped" "reason=text-on-clipboard" "preview='$_early_xt'"
      fi
    fi
  fi
fi

# ────────────────────────────────────────────────────────────────────
# FAST PATH — if early-preload succeeded, xclip is the durable image
# owner already. Skip every probe (clip_types_wl, wl_text, divergence,
# prestage) and jump straight to send-text. Cuts total dispatch
# latency from ~410ms to ~120ms (just the early-preload sleep + kitty
# send-text round-trip + tmux unbind).
# ────────────────────────────────────────────────────────────────────
if [ "$_early_loaded" = "1" ]; then
  _fp_client_term=$(tmux display-message -p -t "$pane" '#{client_termname}' 2>/dev/null)
  log "FAST PATH: early-preload succeeded, terminal='$_fp_client_term'"
  case "$_fp_client_term" in
    xterm-kitty*|kitty*)
      _fp_sock=""
      for _sp in /run/user/$(id -u)/kitty-main-*; do
        [ -S "$_sp" ] && _fp_sock="unix:$_sp" && break
      done
      if [ -n "$_fp_sock" ]; then
        t "fast-path before-unbind"
        tmux unbind -n C-v 2>>"$LOG"
        t "fast-path after-unbind"
        printf '\026' | kitty @ --to "$_fp_sock" send-text --match state:focused --stdin 2>>"$LOG"
        t "fast-path after-send-text"
        clog "paste-dispatch" "event=fast-path-done" "transport=kitty-send-text" "mime='$_early_mime'"
        log "FAST PATH: send-text done"
        setsid -f sh -c '
          sleep 0.1
          tmux bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v /home/deadpool/.local/bin/tmux-paste-dispatch.sh '\''#{pane_id}'\''"
        ' </dev/null >/dev/null 2>&1
        t "fast-path exit"
        exit 0
      fi
      ;;
    *)
      # GNOME Terminal / VTE — ydotool ctrl+v
      if command -v ydotool >/dev/null 2>&1; then
        export YDOTOOL_SOCKET="${YDOTOOL_SOCKET:-$XDG_RUNTIME_DIR/.ydotool_socket}"
        if [ -S "$YDOTOOL_SOCKET" ]; then
          tmux unbind -n C-v 2>>"$LOG"
          ydotool key ctrl+v 2>>"$LOG"
          clog "paste-dispatch" "event=fast-path-done" "transport=ydotool" "mime='$_early_mime'"
          log "FAST PATH: ydotool ctrl+v done"
          setsid -f sh -c '
            sleep 0.1
            tmux bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v /home/deadpool/.local/bin/tmux-paste-dispatch.sh '\''#{pane_id}'\''"
          ' </dev/null >/dev/null 2>&1
          exit 0
        fi
      fi
      ;;
  esac
fi

# --- clipboard snapshot for diagnostics (does NOT affect paste flow) ---
# NOTE: we deliberately do NOT call `wl-paste` here. Every wl-paste
# invocation opens a Wayland client which Ubuntu's dock briefly shows
# as "wl-clipboard ready" / Unknown gear icon — that's the flash the
# user was complaining about. xclip and the helper script are silent
# on the Wayland side.
{
  x_text=$(timeout 0.5 xclip -selection clipboard -o 2>/dev/null | head -c 200)
  helper_text=$(timeout 3 /home/deadpool/.local/bin/get-clipboard-text.sh 2>/dev/null | head -c 200)
  log "xclip ='$(printf '%s' "$x_text"  | tr '\n\r\t' '   ')'"
  log "helper='$(printf '%s' "$helper_text" | tr '\n\r\t' '   ')'"
} &
diag_pid=$!
# don't wait — diagnostics shouldn't slow paste

# Detect host terminal via the client's TERM. xterm-kitty → Kitty.
# Anything else → assume GNOME Terminal / VTE-style.
client_term=$(tmux display-message -p -t "$pane" '#{client_termname}' 2>/dev/null)
log "client_termname='$client_term'"

# Probe clipboard MIME types. If an image is present, no terminal's native
# text-paste path will deliver it — Claude Code's TUI must receive a plain
# Ctrl+V keystroke so its own image-clipboard reader (wl-paste -t image/png)
# fires. Skip kitty/text branches entirely in that case.
#
# Check BOTH wl-paste (Wayland clipboard) and xclip TARGETS (XWayland
# clipboard). On mutter, wl-paste is often blocked by the focus-gated
# clipboard policy and returns empty even when an image is on the
# clipboard; xclip via XWayland sees the X11 mirror and is more reliable
# here.
clip_types_wl=$(timeout 0.5 wl-paste --list-types 2>/dev/null)
wl_rc=$?
clip_types_x=$(timeout 0.5 xclip -selection clipboard -t TARGETS -o 2>/dev/null)
xc_rc=$?

# Synchronous Wayland text probe — used for both the Wayland-authoritative
# has_image decision and divergence logging.
#
# IMPORTANT: this is gated on wl_rc==0 AND clip_types_wl non-empty. When
# mutter's data-device is wedged, the first wl-paste already timed out at
# 500ms — probing again with --no-newline just adds a second "wl-clipboard
# ready" dock flash and another 500ms wait for no value (Wayland is silent;
# we fall back to X11 either way). Only when Wayland actually answered the
# types probe do we spend a second wl-paste invocation to fetch text.
if [ "$wl_rc" = "0" ] && [ -n "$clip_types_wl" ]; then
  wl_text=$(timeout 0.5 wl-paste --no-newline 2>/dev/null | head -c 120 | tr '\n\r\t' '   ')
else
  wl_text=
fi

wl_has_image=0
x_has_image=0
case "$clip_types_wl" in *image/*) wl_has_image=1 ;; esac
case "$clip_types_x"  in *image/*) x_has_image=1 ;; esac

# Wayland-authoritative decision policy.
#
# On GNOME-46/mutter the X11<->Wayland clipboard bridge is sticky: xclip
# TARGETS keeps advertising `image/png` long after a fresh text copy on
# the Wayland side, because mutter's wedged data-device never broadcasts
# the new selection back to the X11 mirror. The OLD policy (`has_image=1
# if EITHER side says image`) trusts that lie and clobbers fresh URLs
# with stale screenshots — see obs 6881 (GitHub URL → "[Image #9]").
#
# New policy:
#   - If Wayland answers at all (non-empty types OR non-empty text),
#     trust Wayland alone. Ignore X11 for image detection.
#   - Only when Wayland is fully silent (mutter wedge: types empty AND
#     text empty) fall back to X11 — that's the genuine "external client
#     can't read Wayland clipboard" case where xclip is the only source.
if [ -n "$clip_types_wl" ] || [ -n "$wl_text" ]; then
  has_image=$wl_has_image
  policy=wayland-authoritative
  if [ "$x_has_image" = "1" ] && [ "$wl_has_image" = "0" ]; then
    policy=wayland-authoritative-stale-x11-ignored
  fi
else
  has_image=$x_has_image
  policy=x11-fallback-wayland-silent
fi
log "clip_types_wl='$(printf '%s' "$clip_types_wl" | tr '\n' ',')' clip_types_x='$(printf '%s' "$clip_types_x" | tr '\n' ',')' has_image=$has_image policy=$policy wl_rc=$wl_rc xc_rc=$xc_rc"

# Auto-pickup fresh screenshot files. GNOME's default PrtScr only SAVES
# to ~/Pictures/Screenshots/ — it never copies to the clipboard. If the
# clipboard is GENUINELY empty (no image, no text) but a screenshot was
# written recently AND we haven't already attached it, pretend that
# screenshot is what the user meant to paste.
#
# Gating rules (regression-tested 2026-05-19):
#   1. has_image=0           — clipboard isn't already serving an image
#   2. No text on clipboard  — user didn't just copy text from a browser
#      (Wayland-side `wl_text` empty AND xclip text-target empty)
#   3. Screenshot ≤30s old   — fresh enough to be "the one the user
#      meant"
#   4. Different from the last auto-pickup we did — without this, every
#      right-click → Paste within 30s re-attaches the same screenshot,
#      flooding Claude with [Image #N] [Image #N+1] [Image #N+2]...
#
# NOTE: previously gated auto-pickup on a "last_attached" state file so
# the same screenshot couldn't be attached twice in a row. Removed in
# v1.5 because the user wants to be able to paste the same screenshot
# multiple times in a row — the 2-second recursion guard at the top of
# this script already prevents the rapid-fire flood case. The mtime
# gate also broke deliberate multi-paste of the same image.
AUTO_SS_STATE="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/clip-last-auto-ss"
if [ "$has_image" = "0" ]; then
  # Probe clipboard text presence (both sides). If anything's there,
  # the user intended a TEXT paste; skip the auto-pickup entirely.
  any_text=""
  if [ -n "${wl_text:-}" ]; then
    any_text="$wl_text"
  else
    xt=$(timeout 0.3 xclip -selection clipboard -t text/plain -o 2>/dev/null | head -c 40)
    [ -n "$xt" ] && any_text="$xt"
  fi
  if [ -n "$any_text" ]; then
    clog "paste-dispatch" "event=auto-pickup-skipped" "reason=text-on-clipboard" "preview='$(printf '%s' "$any_text" | tr '\n\r\t' '   ' | head -c 60)'"
  else
    ss_dir="$HOME/Pictures/Screenshots"
    if [ -d "$ss_dir" ]; then
      # NOTE: this box's `find` is bfs which doesn't accept fractional
      # -mmin, so we widen to <1 min and filter to <=30s in shell below.
      latest_ss=$(find "$ss_dir" -maxdepth 1 -type f \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' \) -mmin -1 -printf "%T@ %p\n" 2>/dev/null | sort -nr | sed -n '1s/^[^ ]* //p')
      if [ -n "$latest_ss" ]; then
        ss_mtime=$(stat -c %Y "$latest_ss" 2>/dev/null || echo 0)
        ss_age=$(($(date +%s) - ss_mtime))
      fi
      if [ -n "$latest_ss" ] && [ "$ss_age" -le 30 ]; then
        clog "paste-dispatch" "event=auto-pickup-screenshot" "path='$latest_ss'" "age_s=$ss_age" "mtime=$ss_mtime"
        log "auto-pickup: fresh screenshot $latest_ss (${ss_age}s old) — loading into xclip"
        case "$latest_ss" in
          *.png|*.PNG) ss_mime=image/png ;;
          *.jpg|*.JPG|*.jpeg|*.JPEG) ss_mime=image/jpeg ;;
          *) ss_mime=image/png ;;
        esac
        setsid -f xclip -selection clipboard -t "$ss_mime" -i "$latest_ss" >/dev/null 2>&1 &
        sleep 0.15  # let xclip claim selection
        has_image=1
        tmux display-message -d 1500 "📸 $(basename "$latest_ss")" 2>/dev/null
      fi
    fi
  fi
fi

# Divergence forensics (background, doesn't gate the paste).
{
  x_text2=$(timeout 0.5 xclip -selection clipboard -t text/plain -o 2>/dev/null | head -c 120 | tr '\n\r\t' '   ')
  x_ts=$(timeout 0.5 xclip -selection clipboard -t TIMESTAMP -o 2>/dev/null | head -c 40)
  divergence=0
  if [ "$x_has_image" = "1" ] && [ -n "$wl_text" ]; then divergence=1; fi
  if [ "$x_has_image" = "1" ] && [ -n "$x_text2" ]; then divergence=1; fi
  clog "paste-dispatch" "event=clip-divergence-probe" "wl_text='$wl_text'" "x_text='$x_text2'" "x_ts='$x_ts'" "stale_mirror_suspected=$divergence"
} &

if [ "$has_image" -eq 1 ]; then
  # Image-clipboard delivery on this box.
  #
  # Why `tmux send-keys` and not ydotool / `kitty @ send-text`:
  #   The user's tmux config binds C-v at the root keytable
  #   (`bind-key -T root C-v run-shell -b "... tmux-paste-dispatch.sh ..."`),
  #   so any Ctrl-V byte that arrives at tmux's input layer fires the
  #   binding and recursively re-invokes THIS script. Both alternative
  #   transports hit that layer:
  #     - ydotool injects at the kernel → kitty forwards to tmux → caught.
  #     - `kitty @ send-text \026` writes the byte to kitty's child
  #       (tmux) → caught.
  #   Each loop also blanks out the clipboard mid-flight (xclip selection
  #   gets stolen by the recursive read), which is why earlier attempts
  #   logged a successful exit but the TUI never received anything (see
  #   tmux-paste.log around 10:37:22).
  #
  # `tmux send-keys -t $pane C-v` writes \026 directly to the target
  # pane's pty AFTER tmux's input/keytable layer. The byte lands on the
  # foreground process (Claude Code / Codex TUI) without firing the root
  # binding, so no recursion. The TUI then calls `wl-paste -t image/png`;
  # the shim at ~/.local/bin/wl-paste falls back to xclip when mutter's
  # Wayland clipboard is silent.
  # Empirically `tmux send-keys -t $pane C-v` writes \026 to the pane
  # pty but Claude Code's TUI does NOT trigger its image-paste handler
  # when the byte arrives that way (verified 2026-05-19, log 11:20:17).
  # `kitty @ send-text` DOES trigger it (verified at 10:40:40 with the
  # compastor.hu screenshot that successfully attached). The downside
  # of kitty's path is that the \026 byte travels kitty → tmux → pane,
  # which fires tmux's `bind -n C-v` and re-invokes THIS script. The
  # recursion guard at the top of the file makes that second invocation
  # a no-op, so the only cost is one extra log line.
  case "$client_term" in
    xterm-kitty*|kitty*)
      sock=""
      for sock_path in /run/user/$(id -u)/kitty-main-*; do
        [ -S "$sock_path" ] && sock="unix:$sock_path" && break
      done
      log "image-paste branch (kitty send-text): sock='$sock'"
      if [ -n "$sock" ]; then
        # Pre-stage the image into xclip so Claude Code's eventual
        # `wl-paste -t image/png` (via the shim) reads from an xclip
        # owner instead of a possibly-exhausted wl-copy daemon.
        #
        # WHY: this user's screenshot tool publishes via
        # `wl-copy --paste-once` (clipboard-set.sh) which only serves
        # ONE clipboard read then exits. The dispatch script itself
        # invokes wl-paste/xclip several times above (clip_types_wl,
        # wl_text probe, diagnostic snapshot, etc.) — each one
        # consumes a receive slot. By the time the synthesized Ctrl-V
        # reaches Claude Code and it calls wl-paste, the wl-copy
        # daemon has already exited, and the read returns 0 bytes.
        # That's the bug behind "needs 3 paste presses to attach".
        #
        # Re-piping wl-paste into xclip -i makes xclip the durable
        # selection owner (it serves unlimited reads). The wl-paste
        # shim's xclip fallback then always returns the image bytes
        # when Claude reads. Verified by both diagnostic agents.
        # Extract a valid image MIME. Concat must use a newline separator —
        # `$clip_types_x$clip_types_wl` smashes the two lists together and
        # produces frankenmimes like "image/pngTARGETS" which xclip would
        # then use as the actual MIME, breaking every subsequent read.
        # Validate against `image/<lower-token>$` to reject any residue.
        img_mime=$(printf '%s\n%s\n' "$clip_types_x" "$clip_types_wl" \
                   | tr ',' '\n' \
                   | grep -m1 -E '^image/[a-z][a-z0-9.+-]*$' \
                   || true)
        [ -z "$img_mime" ] && img_mime=image/png
        clog "paste-dispatch" "event=image-prestage-start" "mime='$img_mime'"
        # Read SYNCHRONOUSLY to a temp file first, then hand the file
        # to xclip. The pipe version (`wl-paste | xclip`) was async on
        # wl-paste's side — when mutter is wedged the shim falls back
        # to xclip but the pipe doesn't complete before we send Ctrl-V,
        # so Claude's read returns 0 bytes. File-based load is
        # immediate (xclip claims selection at fork time).
        img_tmp=$(mktemp --tmpdir clip-prestage-XXXXXX.bin)
        timeout 1 wl-paste -t "$img_mime" >"$img_tmp" 2>/dev/null
        bytes=$(stat -c%s "$img_tmp" 2>/dev/null || echo 0)
        clog "paste-dispatch" "event=image-prestage-read" "bytes=$bytes"
        if [ "$bytes" -gt 0 ]; then
          setsid -f xclip -selection clipboard -t "$img_mime" -i "$img_tmp" >/dev/null 2>&1 &
          sleep 0.15  # xclip claims selection on fork
          log "image-prestage: $bytes bytes -> xclip ($img_mime)"
          clog "paste-dispatch" "event=image-prestage-loaded" "bytes=$bytes"
        else
          log "image-prestage: wl-paste returned 0 bytes — clipboard drained?"
          clog "paste-dispatch" "event=image-prestage-empty"
        fi
        # tmp file kept until next tmpreaper run; xclip is still reading.

        # Unbind tmux's root C-v keybinding before we synthesize the
        # keystroke so it doesn't intercept our \026 byte. Rebind 100ms
        # later via setsid -f (fully detached from this script's
        # process group so it survives script exit; SIGHUP from the
        # closing pty can't reach a setsid'd process).
        log "image-paste: unbinding tmux C-v temporarily"
        tmux unbind -n C-v 2>>"$LOG"
        printf '\026' | kitty @ --to "$sock" send-text --match state:focused --stdin 2>>"$LOG"
        rc=$?
        log "kitty send-text Ctrl-V exit=$rc"
        clog "paste-dispatch" "event=image-ctrlv-sent" "transport=kitty-send-text-unbound" "rc=$rc"
        setsid -f sh -c '
          sleep 0.1
          tmux bind -n C-v run-shell -b "TMUX_PASTE_TRIGGER=ctrl-v /home/deadpool/.local/bin/tmux-paste-dispatch.sh '\''#{pane_id}'\''"
        ' </dev/null >/dev/null 2>&1
        log "image-paste: rebind scheduled (setsid +100ms)"
        exit 0
      fi
      log "image-paste branch (kitty): no socket → falling through to ydotool"
      ;;
  esac
  # Non-kitty host: ydotool 0.1.8 ctrl+v. The kitty Ctrl+V keybind
  # interception doesn't apply outside kitty.
  if command -v ydotool >/dev/null 2>&1; then
    export YDOTOOL_SOCKET="${YDOTOOL_SOCKET:-$XDG_RUNTIME_DIR/.ydotool_socket}"
    log "image-paste branch (ydotool): socket='$YDOTOOL_SOCKET'"
    if [ -S "$YDOTOOL_SOCKET" ]; then
      ydotool key ctrl+v 2>>"$LOG"
      rc=$?
      log "ydotool key ctrl+v exit=$rc"
      clog "paste-dispatch" "event=image-ctrlv-sent" "transport=ydotool" "rc=$rc"
      exit 0
    fi
  fi
fi

case "$client_term" in
  xterm-kitty*|kitty*)
    sock=""
    for sock_path in /run/user/$(id -u)/kitty-main-*; do
      [ -S "$sock_path" ] && sock="unix:$sock_path" && break
    done
    log "kitty branch: sock='$sock'"
    if [ -n "$sock" ]; then
      unset KITTY_WINDOW_ID
      kitty @ --to "$sock" action --match state:focused paste_from_clipboard
      rc=$?
      log "kitty paste_from_clipboard exit=$rc"
      exit 0
    fi
    log "kitty branch: no socket → falling through"
    ;;
esac

# Non-kitty terminal: synthesize Ctrl+Shift+V via ydotool. The host
# terminal handles the paste natively, reading its own clipboard.
#
# ydotool 0.1.8 (Ubuntu 24.04) uses key-name syntax `ctrl+shift+v`,
# NOT the numeric `keycode:state` pairs from ydotool 1.x.
if command -v ydotool >/dev/null 2>&1; then
  export YDOTOOL_SOCKET="${YDOTOOL_SOCKET:-$XDG_RUNTIME_DIR/.ydotool_socket}"
  log "ydotool branch: socket='$YDOTOOL_SOCKET' exists=$([ -S "$YDOTOOL_SOCKET" ] && echo yes || echo no)"
  if [ -S "$YDOTOOL_SOCKET" ]; then
    ydotool key ctrl+shift+v 2>>"$LOG"
    rc=$?
    log "ydotool key ctrl+shift+v exit=$rc"
    exit 0
  fi
fi

# Last-resort fallback — just send Ctrl+V to the pane.
log "FALLBACK send-keys C-v (no kitty socket, no ydotool socket)"
tmux send-keys -t "$pane" C-v
