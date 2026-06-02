#!/usr/bin/env bash
#
# inject.sh — canonical Parallels key-injection helper for Breenix host-side tests.
#
# All scancodes are PS/2 set-1 codes; Parallels translates them to USB-HID and
# delivers them to the guest. Extended keys (cursor keys, GUI/Super, etc.) use a
# 0xE0 (224) prefix byte that is sent as its own press/release around the code.
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

# ---- low-level primitives ---------------------------------------------------
ms_to_s() { awk "BEGIN{printf \"%.3f\", ${1}/1000}"; }

press()   { prlctl send-key-event "$VM" --scancode "$1" --event press   >/dev/null 2>&1; }
release() { prlctl send-key-event "$VM" --scancode "$1" --event release >/dev/null 2>&1; }

# Tap a (possibly extended) key.
#   $1 code, $2 hold_ms (optional), $3 extended-prefix (optional, e.g. 224)
tap() {
    local code="$1"
    local hold_ms="${2:-$HOLD_MS}"
    local ext="${3:-}"
    if [[ -n "$ext" ]]; then press "$ext"; sleep "$(ms_to_s "$PREFIX_MS")"; fi
    press "$code"
    sleep "$(ms_to_s "$hold_ms")"
    release "$code"
    if [[ -n "$ext" ]]; then sleep "$(ms_to_s "$PREFIX_MS")"; release "$ext"; fi
}

# Two clean taps separated by gap_ms.
#   $1 code, $2 gap_ms, $3 extended-prefix (optional)
doubletap() {
    local code="$1"
    local gap_ms="${2:-150}"
    local ext="${3:-}"
    tap "$code" "$HOLD_MS" "$ext"
    sleep "$(ms_to_s "$gap_ms")"
    tap "$code" "$HOLD_MS" "$ext"
}

# Press, hold for hold_ms, release (extended-aware).
#   $1 code, $2 hold_ms, $3 extended-prefix (optional)
hold() {
    local code="$1"
    local hold_ms="${2:-100}"
    local ext="${3:-}"
    if [[ -n "$ext" ]]; then press "$ext"; sleep "$(ms_to_s "$PREFIX_MS")"; fi
    press "$code"
    sleep "$(ms_to_s "$hold_ms")"
    release "$code"
    if [[ -n "$ext" ]]; then sleep "$(ms_to_s "$PREFIX_MS")"; release "$ext"; fi
}

# PS/2 set-1 scancodes for printable characters we support in `type`.
declare -A SC=(
  [a]=30 [b]=48 [c]=46 [d]=32 [e]=18 [f]=33 [g]=34 [h]=35 [i]=23 [j]=36
  [k]=37 [l]=38 [m]=50 [n]=49 [o]=24 [p]=25 [q]=16 [r]=19 [s]=31 [t]=20
  [u]=22 [v]=47 [w]=17 [x]=45 [y]=21 [z]=44
  [1]=2 [2]=3 [3]=4 [4]=5 [5]=6 [6]=7 [7]=8 [8]=9 [9]=10 [0]=11
  [' ']=57
)

type_str() {
    local s="$1" i ch code
    for (( i=0; i<${#s}; i++ )); do
        ch="${s:$i:1}"
        code="${SC[$ch]:-}"
        if [[ -n "$code" ]]; then
            tap "$code"
            sleep "$(ms_to_s "$TYPE_GAP_MS")"
        else
            echo "inject.sh: skipping unsupported character '$ch'" >&2
        fi
    done
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
