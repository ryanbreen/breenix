#!/usr/bin/env bash
#
# Type a string into a Parallels VM via send-key-event.
# Converts ASCII characters to Parallels key codes.
#
# Usage: ./type-in-vm.sh "command to type"
#        ./type-in-vm.sh --enter            # Just press Enter
#        ./type-in-vm.sh "command" --enter   # Type + press Enter
#
set -euo pipefail

VM_NAME="${VM_NAME:-breenix-hwdump}"
DELAY="${KEY_DELAY:-50}"

send_key() {
    local code="$1"
    prlctl send-key-event "$VM_NAME" --scancode "$code" --event press >/dev/null 2>&1
    sleep 0.02
    prlctl send-key-event "$VM_NAME" --scancode "$code" --event release >/dev/null 2>&1
    sleep 0.02
}

send_shift_key() {
    local code="$1"
    # Hold shift
    prlctl send-key-event "$VM_NAME" --scancode 42 --event press >/dev/null 2>&1
    sleep 0.01
    prlctl send-key-event "$VM_NAME" --scancode "$code" --event press >/dev/null 2>&1
    sleep 0.02
    prlctl send-key-event "$VM_NAME" --scancode "$code" --event release >/dev/null 2>&1
    sleep 0.01
    # Release shift
    prlctl send-key-event "$VM_NAME" --scancode 42 --event release >/dev/null 2>&1
    sleep 0.02
}

send_enter() {
    send_key 28
}

# Map ASCII to PS/2 scan codes
char_to_scancode() {
    local c="$1"
    case "$c" in
        a) send_key 30 ;; b) send_key 48 ;; c) send_key 46 ;; d) send_key 32 ;;
        e) send_key 18 ;; f) send_key 33 ;; g) send_key 34 ;; h) send_key 35 ;;
        i) send_key 23 ;; j) send_key 36 ;; k) send_key 37 ;; l) send_key 38 ;;
        m) send_key 50 ;; n) send_key 49 ;; o) send_key 24 ;; p) send_key 25 ;;
        q) send_key 16 ;; r) send_key 19 ;; s) send_key 31 ;; t) send_key 20 ;;
        u) send_key 22 ;; v) send_key 47 ;; w) send_key 17 ;; x) send_key 45 ;;
        y) send_key 21 ;; z) send_key 44 ;;
        A) send_shift_key 30 ;; B) send_shift_key 48 ;; C) send_shift_key 46 ;;
        D) send_shift_key 32 ;; E) send_shift_key 18 ;; F) send_shift_key 33 ;;
        G) send_shift_key 34 ;; H) send_shift_key 35 ;; I) send_shift_key 23 ;;
        J) send_shift_key 36 ;; K) send_shift_key 37 ;; L) send_shift_key 38 ;;
        M) send_shift_key 50 ;; N) send_shift_key 49 ;; O) send_shift_key 24 ;;
        P) send_shift_key 25 ;; Q) send_shift_key 16 ;; R) send_shift_key 19 ;;
        S) send_shift_key 31 ;; T) send_shift_key 20 ;; U) send_shift_key 22 ;;
        V) send_shift_key 47 ;; W) send_shift_key 17 ;; X) send_shift_key 45 ;;
        Y) send_shift_key 21 ;; Z) send_shift_key 44 ;;
        0) send_key 11 ;; 1) send_key 2 ;; 2) send_key 3 ;; 3) send_key 4 ;;
        4) send_key 5 ;; 5) send_key 6 ;; 6) send_key 7 ;; 7) send_key 8 ;;
        8) send_key 9 ;; 9) send_key 10 ;;
        ' ') send_key 57 ;;
        '-') send_key 12 ;;
        '=') send_key 13 ;;
        '[') send_key 26 ;;
        ']') send_key 27 ;;
        '\\') send_key 43 ;;
        ';') send_key 39 ;;
        "'") send_key 40 ;;
        '`') send_key 41 ;;
        ',') send_key 51 ;;
        '.') send_key 52 ;;
        '/') send_key 53 ;;
        '!') send_shift_key 2 ;;
        '@') send_shift_key 3 ;;
        '#') send_shift_key 4 ;;
        '$') send_shift_key 5 ;;
        '%') send_shift_key 6 ;;
        '^') send_shift_key 7 ;;
        '&') send_shift_key 8 ;;
        '*') send_shift_key 9 ;;
        '(') send_shift_key 10 ;;
        ')') send_shift_key 11 ;;
        '_') send_shift_key 12 ;;
        '+') send_shift_key 13 ;;
        '{') send_shift_key 26 ;;
        '}') send_shift_key 27 ;;
        '|') send_shift_key 43 ;;
        ':') send_shift_key 39 ;;
        '"') send_shift_key 40 ;;
        '~') send_shift_key 41 ;;
        '<') send_shift_key 51 ;;
        '>') send_shift_key 52 ;;
        '?') send_shift_key 53 ;;
        $'\t') send_key 15 ;;
        *) echo "WARNING: unmapped char '$c'" >&2 ;;
    esac
}

PRESS_ENTER=false
TEXT=""

for arg in "$@"; do
    if [ "$arg" = "--enter" ]; then
        PRESS_ENTER=true
    else
        TEXT="$arg"
    fi
done

# Type the text character by character
if [ -n "$TEXT" ]; then
    for (( i=0; i<${#TEXT}; i++ )); do
        char="${TEXT:$i:1}"
        char_to_scancode "$char"
    done
fi

# Press enter if requested
if [ "$PRESS_ENTER" = true ]; then
    send_enter
fi
