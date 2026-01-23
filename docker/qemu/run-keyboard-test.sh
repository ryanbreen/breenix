#!/bin/bash
# Test keyboard input in QEMU using QEMU monitor via TCP socket
#
# Usage: ./run-keyboard-test.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$PROJECT_ROOT"

# Build the kernel first - use testing feature (not interactive which requires test disk)
echo "Building kernel..."
cargo build --release --features testing --bin qemu-uefi 2>&1 | grep -E "Compiling|Finished|error" || true

# Get the UEFI image path
UEFI_IMG=$(find target/release/build -name "breenix-uefi.img" 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "ERROR: Could not find breenix-uefi.img"
    exit 1
fi
echo "Using UEFI image: $UEFI_IMG"

# Get OVMF path
OVMF_CODE=$(find target -name "OVMF_CODE.fd" 2>/dev/null | head -1)
if [ -z "$OVMF_CODE" ]; then
    OVMF_CODE="target/ovmf/x64/code.fd"
fi
echo "Using OVMF: $OVMF_CODE"

# Create output directory
mkdir -p target/keyboard-test
SERIAL1_OUTPUT="$PROJECT_ROOT/target/keyboard-test/serial1_output.txt"
SERIAL2_OUTPUT="$PROJECT_ROOT/target/keyboard-test/serial2_output.txt"
rm -f "$SERIAL1_OUTPUT" "$SERIAL2_OUTPUT"

echo "Starting QEMU with monitor on TCP port 4444..."
echo "Capturing both COM1 and COM2 serial ports..."

# Start QEMU in background with monitor on TCP socket
# Capture BOTH serial ports (COM1 = user output, COM2 = boot stages)
qemu-system-x86_64 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive format=raw,file="$UEFI_IMG" \
    -m 512M \
    -serial file:"$SERIAL1_OUTPUT" \
    -serial file:"$SERIAL2_OUTPUT" \
    -display none \
    -device VGA \
    -monitor tcp:127.0.0.1:4444,server,nowait \
    &
QEMU_PID=$!

echo "QEMU started with PID $QEMU_PID"

# Wait for kernel to boot and enable interrupts
# Need enough time for kernel init but before it panics from missing test disk
echo "Waiting 8 seconds for kernel to initialize and enable interrupts..."
sleep 8

# Check if serial output is being generated
echo "Serial output status:"
if [ -f "$SERIAL1_OUTPUT" ]; then
    SIZE1=$(wc -c < "$SERIAL1_OUTPUT")
    echo "  COM1 (user output): $SIZE1 bytes"
else
    echo "  COM1: No file yet"
fi
if [ -f "$SERIAL2_OUTPUT" ]; then
    SIZE2=$(wc -c < "$SERIAL2_OUTPUT")
    echo "  COM2 (boot stages): $SIZE2 bytes"
else
    echo "  COM2: No file yet"
fi

# Send keystrokes via QEMU monitor
echo "Sending keystrokes via monitor..."

# Function to send monitor command
send_key() {
    echo "sendkey $1" | nc -q 1 127.0.0.1 4444 2>/dev/null || echo "sendkey $1" | nc 127.0.0.1 4444 2>/dev/null || true
    sleep 0.5
}

# Send keys: a, b, c, 1, 2, 3
for key in a b c 1 2 3; do
    echo "  Sending key: $key"
    send_key "$key"
done

# Send Enter
echo "  Sending key: ret"
send_key "ret"
sleep 0.5
send_key "ret"
sleep 3

echo "Done sending keystrokes"

# Show final serial output sizes
echo "Final serial output status:"
if [ -f "$SERIAL1_OUTPUT" ]; then
    SIZE1=$(wc -c < "$SERIAL1_OUTPUT")
    echo "  COM1 (user output): $SIZE1 bytes"
fi
if [ -f "$SERIAL2_OUTPUT" ]; then
    SIZE2=$(wc -c < "$SERIAL2_OUTPUT")
    echo "  COM2 (boot stages): $SIZE2 bytes"
fi

# Quit QEMU
echo "quit" | nc -q 1 127.0.0.1 4444 2>/dev/null || echo "quit" | nc 127.0.0.1 4444 2>/dev/null || true
sleep 1
kill $QEMU_PID 2>/dev/null || true

# Check results - look in BOTH serial outputs
echo ""
echo "=== KEYBOARD TEST RESULTS ==="

# Combine both serial outputs for searching
cat "$SERIAL1_OUTPUT" "$SERIAL2_OUTPUT" 2>/dev/null > /tmp/combined_serial.txt

echo "Looking for KEY:XX patterns in both serial outputs..."
KEY_COUNT=$(grep -c "KEY:" /tmp/combined_serial.txt 2>/dev/null || echo "0")
if [ "$KEY_COUNT" -gt 0 ]; then
    echo "SUCCESS: Found $KEY_COUNT keyboard interrupt markers!"
    echo ""
    echo "Key patterns found:"
    grep -o "KEY:[0-9A-Fa-f][0-9A-Fa-f]" /tmp/combined_serial.txt | sort | uniq -c
else
    echo "FAILURE: No KEY:XX patterns found"
    echo ""
    echo "=== COM1 (user output) - checking for keyboard/PIC messages ==="
    if [ -f "$SERIAL1_OUTPUT" ]; then
        grep -i "PIC\|keyboard\|KEY" "$SERIAL1_OUTPUT" | head -10 || echo "No PIC/keyboard messages in COM1"
    fi
    echo ""
    echo "=== COM2 (boot stages) - checking for keyboard/PIC messages ==="
    if [ -f "$SERIAL2_OUTPUT" ]; then
        grep -i "PIC\|keyboard\|KEY" "$SERIAL2_OUTPUT" | head -10 || echo "No PIC/keyboard messages in COM2"
    fi
    echo ""
    echo "=== Last 20 lines of COM1 ==="
    tail -20 "$SERIAL1_OUTPUT" 2>/dev/null || echo "No COM1 output"
    echo ""
    echo "=== Last 20 lines of COM2 ==="
    tail -20 "$SERIAL2_OUTPUT" 2>/dev/null || echo "No COM2 output"
fi

rm -f /tmp/combined_serial.txt
