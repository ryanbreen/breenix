#!/bin/bash
# GDB Session Manager - Persistent interactive debugging
#
# This script manages a long-running gdb_chat.py session that can be
# interacted with command-by-command. Supports both x86_64 and ARM64.
#
# Usage:
#   ./gdb_session.sh start                     - Start x86_64 GDB session (default)
#   ./gdb_session.sh start --arch aarch64      - Start ARM64 GDB session
#   ./gdb_session.sh cmd "break kernel_main"   - Send a command
#   ./gdb_session.sh serial                    - Get all serial output
#   ./gdb_session.sh stop                      - Stop the session
#   ./gdb_session.sh status                    - Check if session is running

SESSION_DIR="/tmp/breenix_gdb_session"
INPUT_FIFO="$SESSION_DIR/input.fifo"
OUTPUT_FILE="$SESSION_DIR/output.jsonl"
PID_FILE="$SESSION_DIR/gdb_chat.pid"
ARCH_FILE="$SESSION_DIR/arch"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Parse --arch flag from arguments after the subcommand
parse_arch() {
    local arch="x86_64"
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --arch)
                arch="$2"
                shift 2
                ;;
            --arch=*)
                arch="${1#--arch=}"
                shift
                ;;
            *)
                shift
                ;;
        esac
    done
    echo "$arch"
}

# Read the architecture of the current session
get_session_arch() {
    if [ -f "$ARCH_FILE" ]; then
        cat "$ARCH_FILE"
    else
        echo "x86_64"
    fi
}

start_session() {
    local arch
    arch=$(parse_arch "$@")

    # Validate arch
    if [ "$arch" != "x86_64" ] && [ "$arch" != "aarch64" ]; then
        echo "ERROR: Invalid architecture '$arch'. Use 'x86_64' or 'aarch64'."
        exit 1
    fi

    # Clean up any existing session
    stop_session 2>/dev/null

    # Create session directory
    mkdir -p "$SESSION_DIR"

    # Save architecture for stop/status commands
    echo "$arch" > "$ARCH_FILE"

    # Create input FIFO
    rm -f "$INPUT_FIFO"
    mkfifo "$INPUT_FIFO"

    # Clear output file
    > "$OUTPUT_FILE"

    # Start gdb_chat.py in background with architecture flag
    # Use tail -f to keep FIFO open for multiple writes
    (tail -f "$INPUT_FIFO" | python3 "$SCRIPT_DIR/gdb_chat.py" --arch "$arch" >> "$OUTPUT_FILE" 2>&1) &
    echo $! > "$PID_FILE"

    echo "Session starting (arch=$arch)... waiting for GDB connection"

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
    local before_count
    before_count=$(wc -l < "$OUTPUT_FILE" 2>/dev/null || echo 0)

    # Send command
    echo "$cmd" > "$INPUT_FIFO"

    # Wait for response (up to 120 seconds for continue commands)
    local timeout=120
    for i in $(seq 1 $timeout); do
        local after_count
        after_count=$(wc -l < "$OUTPUT_FILE" 2>/dev/null || echo 0)
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
    local arch
    arch=$(get_session_arch)

    if [ -f "$PID_FILE" ]; then
        local pid
        pid=$(cat "$PID_FILE")
        # Kill the tail -f | python pipeline
        pkill -P "$pid" 2>/dev/null
        kill "$pid" 2>/dev/null
        rm -f "$PID_FILE"
    fi

    # Clean up QEMU (architecture-aware) and GDB
    if [ "$arch" = "aarch64" ]; then
        pkill -9 -f "qemu-system-aarch64.*-s.*-S" 2>/dev/null || true
    else
        pkill -9 qemu-system-x86_64 2>/dev/null || true
    fi
    pkill -9 gdb 2>/dev/null || true

    # Remove FIFO and arch file
    rm -f "$INPUT_FIFO"
    rm -f "$ARCH_FILE"

    echo "Session stopped"
}

session_status() {
    if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
        local arch
        arch=$(get_session_arch)
        echo "Session is running (PID: $(cat "$PID_FILE"), arch: $arch)"
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
        shift
        start_session "$@"
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
        echo "Usage: $0 {start|cmd|serial|stop|status} [options]"
        echo ""
        echo "Commands:"
        echo "  start [--arch ARCH]  Start a new GDB debugging session"
        echo "                       ARCH: x86_64 (default) or aarch64"
        echo "  cmd \"command\"        Send a GDB command (e.g., cmd \"break main\")"
        echo "  serial               Get all kernel serial output"
        echo "  stop                 Stop the current session"
        echo "  status               Check session status"
        echo ""
        echo "Examples:"
        echo "  $0 start                          # x86_64 session"
        echo "  $0 start --arch aarch64           # ARM64 session"
        echo "  $0 cmd \"break kernel_main\""
        echo "  $0 cmd \"continue\""
        echo "  $0 cmd \"info registers\""
        echo "  $0 serial"
        echo "  $0 stop"
        exit 1
        ;;
esac
