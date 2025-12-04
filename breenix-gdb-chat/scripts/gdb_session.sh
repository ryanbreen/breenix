#!/bin/bash
# GDB Session Manager - Persistent interactive debugging
#
# This script manages a long-running gdb_chat.py session that can be
# interacted with command-by-command.
#
# Usage:
#   ./gdb_session.sh start   - Start a new GDB session (runs in background)
#   ./gdb_session.sh cmd "break kernel::kernel_main" - Send a command
#   ./gdb_session.sh serial  - Get all serial output
#   ./gdb_session.sh stop    - Stop the session
#   ./gdb_session.sh status  - Check if session is running

SESSION_DIR="/tmp/breenix_gdb_session"
INPUT_FIFO="$SESSION_DIR/input.fifo"
OUTPUT_FILE="$SESSION_DIR/output.jsonl"
PID_FILE="$SESSION_DIR/gdb_chat.pid"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

start_session() {
    # Clean up any existing session
    stop_session 2>/dev/null

    # Create session directory
    mkdir -p "$SESSION_DIR"

    # Create input FIFO
    rm -f "$INPUT_FIFO"
    mkfifo "$INPUT_FIFO"

    # Clear output file
    > "$OUTPUT_FILE"

    # Start gdb_chat.py in background
    # Use tail -f to keep FIFO open for multiple writes
    (tail -f "$INPUT_FIFO" | python3 "$SCRIPT_DIR/gdb_chat.py" >> "$OUTPUT_FILE" 2>&1) &
    echo $! > "$PID_FILE"

    echo "Session starting... waiting for GDB connection"

    # Wait for initial connection response (up to 60 seconds)
    for i in {1..60}; do
        if [ -s "$OUTPUT_FILE" ]; then
            # Check if we got a valid JSON response
            if head -1 "$OUTPUT_FILE" | python3 -c "import json,sys; json.load(sys.stdin)" 2>/dev/null; then
                echo "Session ready!"
                head -1 "$OUTPUT_FILE"
                return 0
            fi
        fi
        sleep 1
    done

    echo "ERROR: Session failed to start"
    cat "$OUTPUT_FILE"
    return 1
}

send_command() {
    local cmd="$1"

    if [ ! -p "$INPUT_FIFO" ]; then
        echo '{"success": false, "error": "No active session. Run: ./gdb_session.sh start"}'
        return 1
    fi

    # Count current lines in output
    local before_count=$(wc -l < "$OUTPUT_FILE" 2>/dev/null || echo 0)

    # Send command
    echo "$cmd" > "$INPUT_FIFO"

    # Wait for response (up to 120 seconds for continue commands)
    local timeout=120
    for i in $(seq 1 $timeout); do
        local after_count=$(wc -l < "$OUTPUT_FILE" 2>/dev/null || echo 0)
        if [ "$after_count" -gt "$before_count" ]; then
            # Got new output - return the last line
            tail -1 "$OUTPUT_FILE"
            return 0
        fi
        sleep 1
    done

    echo '{"success": false, "error": "timeout waiting for response"}'
    return 1
}

get_serial() {
    send_command "serial"
}

stop_session() {
    if [ -f "$PID_FILE" ]; then
        local pid=$(cat "$PID_FILE")
        # Kill the tail -f | python pipeline
        pkill -P $pid 2>/dev/null
        kill $pid 2>/dev/null
        rm -f "$PID_FILE"
    fi

    # Clean up QEMU and GDB
    pkill -9 qemu-system-x86_64 2>/dev/null
    pkill -9 gdb 2>/dev/null

    # Remove FIFO
    rm -f "$INPUT_FIFO"

    echo "Session stopped"
}

session_status() {
    if [ -f "$PID_FILE" ] && kill -0 $(cat "$PID_FILE") 2>/dev/null; then
        echo "Session is running (PID: $(cat "$PID_FILE"))"
        echo "Output file: $OUTPUT_FILE"
        echo "Lines in output: $(wc -l < "$OUTPUT_FILE")"
        return 0
    else
        echo "No active session"
        return 1
    fi
}

# Main command dispatch
case "${1:-}" in
    start)
        start_session
        ;;
    cmd)
        if [ -z "${2:-}" ]; then
            echo "Usage: $0 cmd \"gdb command\""
            exit 1
        fi
        send_command "$2"
        ;;
    serial)
        get_serial
        ;;
    stop)
        stop_session
        ;;
    status)
        session_status
        ;;
    *)
        echo "Usage: $0 {start|cmd|serial|stop|status}"
        echo ""
        echo "Commands:"
        echo "  start              Start a new GDB debugging session"
        echo "  cmd \"command\"      Send a GDB command (e.g., cmd \"break main\")"
        echo "  serial             Get all kernel serial output"
        echo "  stop               Stop the current session"
        echo "  status             Check session status"
        exit 1
        ;;
esac
