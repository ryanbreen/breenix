#!/bin/bash
# Paste text into the Breenix QEMU guest via the monitor sendkey interface.
#
# Usage:
#   echo 'console.log("hello");' | ./scripts/paste.sh
#   ./scripts/paste.sh <<< 'let x = 42;'
#   ./scripts/paste.sh < script.js
#   ./scripts/paste.sh -f script.js
#
# Prerequisites:
#   Run Breenix with: ./run.sh --clean  (monitor port is auto-enabled)
#
# Options:
#   -p PORT   Monitor port (default: 4444)
#   -d DELAY  Delay between keys in seconds (default: 0.01)
#   -f FILE   Read input from file instead of stdin
#   -n        Send Enter at the end
#   -h        Show help

set -e

PORT=4444
DELAY=0.01
SEND_ENTER=false
INPUT_FILE=""

while getopts "p:d:f:nh" opt; do
    case $opt in
        p) PORT="$OPTARG" ;;
        d) DELAY="$OPTARG" ;;
        f) INPUT_FILE="$OPTARG" ;;
        n) SEND_ENTER=true ;;
        h)
            echo "Usage: echo 'code' | $0 [-p port] [-d delay] [-n]"
            echo "       $0 -f script.js [-p port] [-d delay] [-n]"
            echo ""
            echo "Paste text into Breenix QEMU guest via monitor sendkey."
            echo ""
            echo "Options:"
            echo "  -p PORT   Monitor port (default: 4444)"
            echo "  -d DELAY  Delay between keys in seconds (default: 0.01)"
            echo "  -f FILE   Read from file"
            echo "  -n        Send Enter at the end"
            exit 0
            ;;
        *) exit 1 ;;
    esac
done

# Check monitor is reachable
if ! nc -z 127.0.0.1 "$PORT" 2>/dev/null; then
    echo "Error: Cannot connect to QEMU monitor on port $PORT" >&2
    echo "Make sure Breenix is running (./run.sh --clean)" >&2
    exit 1
fi

# Set up persistent connection to QEMU monitor
FIFO=$(mktemp -u /tmp/breenix_paste.XXXXXX)
mkfifo "$FIFO"
nc 127.0.0.1 "$PORT" < "$FIFO" > /dev/null 2>&1 &
NC_PID=$!
exec 3>"$FIFO"

cleanup() {
    exec 3>&- 2>/dev/null
    kill $NC_PID 2>/dev/null
    wait $NC_PID 2>/dev/null
    rm -f "$FIFO"
}
trap cleanup EXIT

send_key() {
    echo "sendkey $1" >&3
    sleep "$DELAY"
}

send_char() {
    local c="$1"
    case "$c" in
        " ")  send_key "spc" ;;
        "	") send_key "tab" ;;  # literal tab
        "")   send_key "ret" ;;   # newline handled by caller
        "a")  send_key "a" ;;
        "b")  send_key "b" ;;
        "c")  send_key "c" ;;
        "d")  send_key "d" ;;
        "e")  send_key "e" ;;
        "f")  send_key "f" ;;
        "g")  send_key "g" ;;
        "h")  send_key "h" ;;
        "i")  send_key "i" ;;
        "j")  send_key "j" ;;
        "k")  send_key "k" ;;
        "l")  send_key "l" ;;
        "m")  send_key "m" ;;
        "n")  send_key "n" ;;
        "o")  send_key "o" ;;
        "p")  send_key "p" ;;
        "q")  send_key "q" ;;
        "r")  send_key "r" ;;
        "s")  send_key "s" ;;
        "t")  send_key "t" ;;
        "u")  send_key "u" ;;
        "v")  send_key "v" ;;
        "w")  send_key "w" ;;
        "x")  send_key "x" ;;
        "y")  send_key "y" ;;
        "z")  send_key "z" ;;
        "A")  send_key "shift-a" ;;
        "B")  send_key "shift-b" ;;
        "C")  send_key "shift-c" ;;
        "D")  send_key "shift-d" ;;
        "E")  send_key "shift-e" ;;
        "F")  send_key "shift-f" ;;
        "G")  send_key "shift-g" ;;
        "H")  send_key "shift-h" ;;
        "I")  send_key "shift-i" ;;
        "J")  send_key "shift-j" ;;
        "K")  send_key "shift-k" ;;
        "L")  send_key "shift-l" ;;
        "M")  send_key "shift-m" ;;
        "N")  send_key "shift-n" ;;
        "O")  send_key "shift-o" ;;
        "P")  send_key "shift-p" ;;
        "Q")  send_key "shift-q" ;;
        "R")  send_key "shift-r" ;;
        "S")  send_key "shift-s" ;;
        "T")  send_key "shift-t" ;;
        "U")  send_key "shift-u" ;;
        "V")  send_key "shift-v" ;;
        "W")  send_key "shift-w" ;;
        "X")  send_key "shift-x" ;;
        "Y")  send_key "shift-y" ;;
        "Z")  send_key "shift-z" ;;
        "0")  send_key "0" ;;
        "1")  send_key "1" ;;
        "2")  send_key "2" ;;
        "3")  send_key "3" ;;
        "4")  send_key "4" ;;
        "5")  send_key "5" ;;
        "6")  send_key "6" ;;
        "7")  send_key "7" ;;
        "8")  send_key "8" ;;
        "9")  send_key "9" ;;
        "!")  send_key "shift-1" ;;
        "@")  send_key "shift-2" ;;
        "#")  send_key "shift-3" ;;
        '$')  send_key "shift-4" ;;
        "%")  send_key "shift-5" ;;
        "^")  send_key "shift-6" ;;
        "&")  send_key "shift-7" ;;
        "*")  send_key "shift-8" ;;
        "(")  send_key "shift-9" ;;
        ")")  send_key "shift-0" ;;
        "-")  send_key "minus" ;;
        "_")  send_key "shift-minus" ;;
        "=")  send_key "equal" ;;
        "+")  send_key "shift-equal" ;;
        "[")  send_key "bracket_left" ;;
        "{")  send_key "shift-bracket_left" ;;
        "]")  send_key "bracket_right" ;;
        "}")  send_key "shift-bracket_right" ;;
        "\\") send_key "backslash" ;;
        "|")  send_key "shift-backslash" ;;
        ";")  send_key "semicolon" ;;
        ":")  send_key "shift-semicolon" ;;
        "'")  send_key "apostrophe" ;;
        '"')  send_key "shift-apostrophe" ;;
        ",")  send_key "comma" ;;
        "<")  send_key "shift-comma" ;;
        ".")  send_key "dot" ;;
        ">")  send_key "shift-dot" ;;
        "/")  send_key "slash" ;;
        "?")  send_key "shift-slash" ;;
        '`')  send_key "grave_accent" ;;
        "~")  send_key "shift-grave_accent" ;;
        *)    echo "Warning: unsupported character '$c'" >&2 ;;
    esac
}

# Read input
if [ -n "$INPUT_FILE" ]; then
    TEXT=$(cat "$INPUT_FILE")
else
    TEXT=$(cat)
fi

TOTAL=${#TEXT}
COUNT=0

echo "Pasting $TOTAL characters into Breenix (port $PORT, ${DELAY}s delay)..." >&2

# Send character by character
for (( i=0; i<${#TEXT}; i++ )); do
    char="${TEXT:$i:1}"
    if [ "$char" = $'\n' ]; then
        send_key "ret"
    else
        send_char "$char"
    fi
    COUNT=$((COUNT + 1))
    # Progress every 50 chars
    if [ $((COUNT % 50)) -eq 0 ]; then
        echo "  $COUNT / $TOTAL chars..." >&2
    fi
done

if [ "$SEND_ENTER" = true ]; then
    send_key "ret"
fi

echo "Done. Pasted $TOTAL characters." >&2
