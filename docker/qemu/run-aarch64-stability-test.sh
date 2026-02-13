#!/bin/bash
# ARM64 stability test - ensures kernel stays stable after shell prompt.
#
# This test boots to the userspace shell, then continues monitoring serial
# output for aborts/exceptions for a short window (post-boot stability).
#
# Usage: ./run-aarch64-stability-test.sh

set -e

WAIT_FOR_PROMPT_SECS=20
POST_PROMPT_WAIT_SECS=8
CHECK_INTERVAL_SECS=1
QEMU_TIMEOUT_SECS=40

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Find ext2 disk (required for init_shell)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

OUTPUT_DIR="/tmp/breenix_aarch64_stability"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Create writable copy of ext2 disk to allow filesystem write tests
EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$EXT2_WRITABLE"

cleanup() {
    if [ -n "${QEMU_PID:-}" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "========================================="
echo "ARM64 Stability Test"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo "Wait for prompt: ${WAIT_FOR_PROMPT_SECS}s"
echo "Post-prompt window: ${POST_PROMPT_WAIT_SECS}s"
echo ""

# Run QEMU with timeout
# Always include GPU, keyboard, and network so kernel VirtIO enumeration finds them
# Use writable disk copy (no readonly=on) to allow filesystem writes
timeout "$QEMU_TIMEOUT_SECS" qemu-system-aarch64 \
    -M virt -cpu cortex-a72 -m 512 -smp 4 \
    -kernel "$KERNEL" \
    -display none -no-reboot \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -device virtio-tablet-device \
    -device virtio-blk-device,drive=ext2 \
    -drive if=none,id=ext2,format=raw,file="$EXT2_WRITABLE" \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -serial file:"$OUTPUT_DIR/serial.txt" &
QEMU_PID=$!

# Wait for USERSPACE shell prompt (init_shell or bsh)
# Accept "breenix>" (init_shell) or "bsh " (bsh shell) as valid userspace prompts
# DO NOT accept "Interactive Shell" - that's the KERNEL FALLBACK when userspace FAILS
BOOT_COMPLETE=false
PROMPT_LINE=0
for _ in $(seq 1 $((WAIT_FOR_PROMPT_SECS / CHECK_INTERVAL_SECS))); do
    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        if grep -qE "(breenix>|bsh )" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BOOT_COMPLETE=true
            PROMPT_LINE=$(grep -nE "(breenix>|bsh )" "$OUTPUT_DIR/serial.txt" | tail -1 | cut -d: -f1)
            break
        fi
        if grep -qiE "(KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            break
        fi
    fi
    sleep "$CHECK_INTERVAL_SECS"
done

if ! $BOOT_COMPLETE; then
    LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
    if grep -qiE "(KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        echo "FAIL: Kernel panic before shell prompt ($LINES lines)"
    else
        echo "FAIL: Shell prompt not detected ($LINES lines)"
    fi
    tail -10 "$OUTPUT_DIR/serial.txt" 2>/dev/null || true
    exit 1
fi

# Verify shell (init_shell or bsh) appears at least once
SHELL_COUNT=$(grep -oE "(init_shell|bsh)" "$OUTPUT_DIR/serial.txt" 2>/dev/null | wc -l | tr -d ' ')
SHELL_COUNT=${SHELL_COUNT:-0}
if [ "$SHELL_COUNT" -lt 1 ]; then
    echo "FAIL: shell marker (init_shell or bsh) not found after prompt"
    tail -10 "$OUTPUT_DIR/serial.txt" 2>/dev/null || true
    exit 1
fi

echo "Boot complete. Monitoring post-prompt output..."
sleep "$POST_PROMPT_WAIT_SECS"

POST_PROMPT_FILE="$OUTPUT_DIR/post_prompt.txt"
if [ "$PROMPT_LINE" -gt 0 ]; then
    tail -n +"$((PROMPT_LINE + 1))" "$OUTPUT_DIR/serial.txt" > "$POST_PROMPT_FILE"
else
    cp "$OUTPUT_DIR/serial.txt" "$POST_PROMPT_FILE"
fi

if grep -qiE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception)" "$POST_PROMPT_FILE"; then
    echo "FAIL: Exception detected after shell prompt"
    grep -inE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception)" "$POST_PROMPT_FILE" | head -5
    exit 1
fi

if grep -qiE "(KERNEL PANIC|panic!)" "$POST_PROMPT_FILE"; then
    echo "FAIL: Kernel panic detected after shell prompt"
    grep -inE "(KERNEL PANIC|panic!)" "$POST_PROMPT_FILE" | head -5
    exit 1
fi

echo "SUCCESS: No aborts/exceptions detected after shell prompt"
exit 0
