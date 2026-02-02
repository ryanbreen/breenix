#!/bin/bash
# Run a single ARM64 userspace test
# Usage: ./run-aarch64-userspace-test.sh <test_name> [timeout]
#
# Example: ./run-aarch64-userspace-test.sh clock_gettime_test 30
#
# The script boots the ARM64 kernel, waits for the shell prompt,
# sends the test command via QEMU monitor, and captures the output.

set -e

if [ -z "$1" ]; then
    echo "Usage: $0 <test_name> [timeout_seconds]"
    echo "Example: $0 clock_gettime_test 30"
    exit 1
fi

TEST_NAME="$1"
TIMEOUT="${2:-45}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found."
    exit 1
fi

# Find ext2 disk
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found."
    exit 1
fi

# Create output directory
OUTPUT_DIR="/tmp/breenix_aarch64_test_$$"
mkdir -p "$OUTPUT_DIR"

# Create writable copy of ext2 disk to allow filesystem write tests
EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$EXT2_WRITABLE"

# Create FIFOs for monitor control
MONITOR_IN="$OUTPUT_DIR/monitor.in"
MONITOR_OUT="$OUTPUT_DIR/monitor.out"
mkfifo "$MONITOR_IN"
mkfifo "$MONITOR_OUT"

# Start QEMU in background
# Use -serial file for output capture
# Use -monitor pipe for sending commands
# Use writable disk copy (no readonly=on) to allow filesystem writes
timeout "$TIMEOUT" qemu-system-aarch64 \
    -M virt -cpu cortex-a72 -m 512 \
    -kernel "$KERNEL" \
    -display none -no-reboot \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -device virtio-blk-device,drive=ext2 \
    -drive if=none,id=ext2,format=raw,file="$EXT2_WRITABLE" \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -serial file:"$OUTPUT_DIR/serial.txt" \
    -monitor pipe:"$OUTPUT_DIR/monitor" \
    &
QEMU_PID=$!

# Wait for shell prompt
echo "Waiting for shell prompt..."
FOUND_PROMPT=false
for i in $(seq 1 30); do
    if [ -f "$OUTPUT_DIR/serial.txt" ] && grep -q "breenix>" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        FOUND_PROMPT=true
        break
    fi
    sleep 1
done

if ! $FOUND_PROMPT; then
    echo "Timeout waiting for shell prompt"
    kill $QEMU_PID 2>/dev/null || true
    cat "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no output)"
    rm -rf "$OUTPUT_DIR"
    exit 1
fi

echo "Shell prompt found, sending test command: $TEST_NAME"

# Send keystrokes via QEMU monitor
# Format: sendkey <key> sends individual keystrokes
{
    # Read monitor output in background (must consume it)
    cat "$MONITOR_OUT" >/dev/null &
    
    # Send each character of the test name
    for (( i=0; i<${#TEST_NAME}; i++ )); do
        char="${TEST_NAME:$i:1}"
        case "$char" in
            [a-z]) key="$char" ;;
            [A-Z]) key="shift-$(echo $char | tr 'A-Z' 'a-z')" ;;
            [0-9]) key="$char" ;;
            '_') key="shift-minus" ;;
            '/') key="slash" ;;
            '.') key="dot" ;;
            '-') key="minus" ;;
            *) key="" ;;
        esac
        if [ -n "$key" ]; then
            echo "sendkey $key" > "$MONITOR_IN"
            sleep 0.05
        fi
    done
    
    # Send Enter
    sleep 0.1
    echo "sendkey ret" > "$MONITOR_IN"
} &
SEND_PID=$!

# Wait for test to complete (look for test-specific markers)
echo "Waiting for test to complete..."
TEST_COMPLETE=false
for i in $(seq 1 20); do
    if grep -qE "(PASS|FAIL|OK|ERROR|Test Summary|panic)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        TEST_COMPLETE=true
        sleep 2  # Give time for full output
        break
    fi
    sleep 1
done

# Cleanup
kill $QEMU_PID 2>/dev/null || true
kill $SEND_PID 2>/dev/null || true
wait $QEMU_PID 2>/dev/null || true

# Show output
echo ""
echo "=== Test Output ==="
cat "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no output)"
echo "==================="

# Determine result
if grep -q "PASS\|: OK" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    RESULT="PASS"
    EXIT_CODE=0
elif grep -qiE "FAIL|ERROR|panic" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
    RESULT="FAIL"
    EXIT_CODE=1
else
    RESULT="UNKNOWN"
    EXIT_CODE=2
fi

echo ""
echo "TEST RESULT: $RESULT"

rm -rf "$OUTPUT_DIR"
exit $EXIT_CODE
