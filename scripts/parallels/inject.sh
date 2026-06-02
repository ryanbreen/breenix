#!/usr/bin/env bash
#
# inject.sh — canonical Parallels key-injection helper for Breenix host-side tests.
#
# All scancodes are PS/2 set-1 codes; Parallels translates them to USB-HID and
# delivers them to the guest. Extended keys (cursor keys, GUI/Super, etc.) use a
# 0xE0 (224) prefix byte that is sent as its own press/release around the code.
#
# Each command is delivered as ONE `prlctl send-key-event -j` batch (events read
# from stdin), so inter-event delays are applied precisely by the Parallels
# dispatcher — essential for the timing-sensitive double-tap on a loaded host,
# where 4 separate prlctl spawns would otherwise blow bwm's 400ms window.
#
# The VM name is read from $VM (env) or, if unset, the first positional arg
# *only* for the rare case where a caller wants `inject.sh <vm> tap ...`. The
# normal form is `VM=breenix-123 inject.sh <command> ...`.
#
# Commands:
#   tap <code> [hold_ms]            single press+release of a basic key
#   key <code> [hold_ms]            alias for tap
#   doubletap <code> <gap_ms> [prefix]
#                                   two clean taps separated by gap_ms; if a
#                                   prefix is given (e.g. 224 for 0xE0) each tap
#                                   is wrapped with that extended prefix
#   hold <code> <hold_ms> [prefix]  press, wait hold_ms, release (extended-aware)
#   type <string>                   type a lowercase-ascii string (a-z, space,
#                                   digits 0-9)
#   enter                           tap Enter (scancode 28)
#
# Examples:
#   VM=breenix-123 scripts/parallels/inject.sh doubletap 91 150 224   # double-Super
#   VM=breenix-123 scripts/parallels/inject.sh type term
#   VM=breenix-123 scripts/parallels/inject.sh enter
#
# Default timings (override per-call via the hold_ms / gap_ms args):
#   HOLD_MS     key press-to-release dwell      (default 40)
#   PREFIX_MS   gap around an extended prefix    (default 5)
#   TYPE_GAP_MS inter-character gap for `type`   (default 40)
#
set -euo pipefail

# ---- defaults (tunable via env) --------------------------------------------
HOLD_MS="${HOLD_MS:-40}"
PREFIX_MS="${PREFIX_MS:-5}"
TYPE_GAP_MS="${TYPE_GAP_MS:-40}"

# ---- VM resolution ----------------------------------------------------------
# Prefer $VM. If $VM is unset/empty, allow the legacy `inject.sh <vm> <cmd> ...`
# form by peeking at $1 only when it does not look like a known command.
if [[ -z "${VM:-}" ]]; then
    case "${1:-}" in
        tap|key|doubletap|hold|type|enter) : ;;  # $1 is a command, VM truly missing
        "" ) : ;;
        * )
            VM="$1"
            shift
            ;;
    esac
fi
if [[ -z "${VM:-}" ]]; then
    echo "inject.sh: error: VM name is empty/unset." >&2
    echo "inject.sh: set it with 'export VM=breenix-<epoch>' (preferred) or pass the VM name as the first argument." >&2
    exit 2
fi

# ---- low-level primitives (single batched -j call) --------------------------
# Every command's key events are sent as ONE `prlctl send-key-event -j` batch
# read from stdin. This is the critical design point: a double-tap is 4 events
# that must land inside bwm's 400ms window, and 4 SEPARATE prlctl spawns take
# ~1.9s on a loaded host (window blown). As one batch, the inter-event DELAYS are
# applied by the Parallels dispatcher with precise timing, independent of host
# load — so the double-tap always lands in-window regardless of prlctl's
# process-spawn latency.

# Send a JSON event array (built by the helpers below) as one -j batch via stdin.
send_json() { printf '%s' "$1" | prlctl send-key-event "$VM" -j >/dev/null 2>&1; }

# Emit the JSON event objects for one (possibly extended) tap: press, hold, release.
#   $1 code, $2 hold_ms, $3 extended-prefix (optional, e.g. 224 for 0xE0)
tap_events() {
    local code="$1" hold="$2" ext="${3:-}" pre="" post=""
    if [[ -n "$ext" ]]; then
        pre="{\"scancode\":$ext,\"event\":\"press\"},{\"delay\":$PREFIX_MS},"
        post=",{\"delay\":$PREFIX_MS},{\"scancode\":$ext,\"event\":\"release\"}"
    fi
    printf '%s{"scancode":%s,"event":"press"},{"delay":%s},{"scancode":%s,"event":"release"}%s' \
        "$pre" "$code" "$hold" "$code" "$post"
}

# Single tap.  $1 code, $2 hold_ms (optional), $3 ext-prefix (optional)
tap() { send_json "[$(tap_events "$1" "${2:-$HOLD_MS}" "${3:-}")]"; }

# Two clean taps separated by gap_ms, sent atomically in ONE batch (the dispatcher
# spaces them by gap_ms). $1 code, $2 gap_ms, $3 ext-prefix (optional)
doubletap() {
    local code="$1" gap="${2:-150}" ext="${3:-}"
    send_json "[$(tap_events "$code" "$HOLD_MS" "$ext"),{\"delay\":$gap},$(tap_events "$code" "$HOLD_MS" "$ext")]"
}

# Press, hold for hold_ms, release.  $1 code, $2 hold_ms, $3 ext-prefix (optional)
hold() { send_json "[$(tap_events "$1" "${2:-100}" "${3:-}")]"; }

# PS/2 set-1 scancodes for printable characters we support in `type`.
declare -A SC=(
  [a]=30 [b]=48 [c]=46 [d]=32 [e]=18 [f]=33 [g]=34 [h]=35 [i]=23 [j]=36
  [k]=37 [l]=38 [m]=50 [n]=49 [o]=24 [p]=25 [q]=16 [r]=19 [s]=31 [t]=20
  [u]=22 [v]=47 [w]=17 [x]=45 [y]=21 [z]=44
  [1]=2 [2]=3 [3]=4 [4]=5 [5]=6 [6]=7 [7]=8 [8]=9 [9]=10 [0]=11
  [' ']=57
)

# Type a string as ONE -j batch: press+release each char, spaced by TYPE_GAP_MS.
type_str() {
    local s="$1" i ch code parts=""
    for (( i=0; i<${#s}; i++ )); do
        ch="${s:$i:1}"
        code="${SC[$ch]:-}"
        if [[ -n "$code" ]]; then
            [[ -n "$parts" ]] && parts+=","
            parts+="$(tap_events "$code" "$HOLD_MS"),{\"delay\":$TYPE_GAP_MS}"
        else
            echo "inject.sh: skipping unsupported character '$ch'" >&2
        fi
    done
    [[ -z "$parts" ]] && return 0
    send_json "[$parts]"
}

# ---- dispatch ---------------------------------------------------------------
cmd="${1:?command required (tap|key|doubletap|hold|type|enter)}"; shift || true
case "$cmd" in
    tap|key)   tap "$@" ;;
    doubletap) doubletap "$@" ;;
    hold)      hold "$@" ;;
    enter)     tap 28 ;;
    type)      type_str "$@" ;;
    *) echo "inject.sh: unknown command: $cmd" >&2; exit 2 ;;
esac
