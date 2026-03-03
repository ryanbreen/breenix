#!/bin/bash
# Type a string into a Parallels VM console using keyboard scancodes.
# Usage: ./vm-type.sh <vm-name> <string>
# Special strings: ENTER, SPACE, TAB, BACKSPACE

VM="$1"
shift
TEXT="$*"

if [ -z "$VM" ] || [ -z "$TEXT" ]; then
    echo "Usage: $0 <vm-name> <text>"
    exit 1
fi

send_key() {
    local sc="$1"
    prlctl send-key-event "$VM" --scancode "$sc" --event press --delay 30 2>/dev/null
    prlctl send-key-event "$VM" --scancode "$sc" --event release 2>/dev/null
}

send_shift_key() {
    local sc="$1"
    # Press shift
    prlctl send-key-event "$VM" --scancode 42 --event press 2>/dev/null
    sleep 0.05
    prlctl send-key-event "$VM" --scancode "$sc" --event press --delay 30 2>/dev/null
    prlctl send-key-event "$VM" --scancode "$sc" --event release 2>/dev/null
    # Release shift
    prlctl send-key-event "$VM" --scancode 42 --event release 2>/dev/null
}

# Handle special words
if [ "$TEXT" = "ENTER" ]; then
    send_key 28
    exit 0
fi

# Type each character
for (( i=0; i<${#TEXT}; i++ )); do
    c="${TEXT:$i:1}"
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
        ' ') send_key 57 ;; # space
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
        *) echo "Warning: unmapped char '$c'" >&2 ;;
    esac
    sleep 0.02
done
