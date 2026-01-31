#!/bin/bash
# Native ARM64 boot test (runs QEMU directly on host)
# Much faster than Docker version but only works on macOS ARM64
#
# The retry mechanism provides robustness for local testing against
# transient host resource contention. If retries are frequently needed,
# investigate for potential regressions.
#
# Usage: ./run-aarch64-boot-test-native.sh

set -e

MAX_RETRIES=5
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

run_single_test() {
    local OUTPUT_DIR="/tmp/breenix_aarch64_boot_native"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Run QEMU with 30s timeout
    # Always include GPU and keyboard so kernel VirtIO enumeration finds them
    timeout 30 qemu-system-aarch64 \
        -M virt -cpu cortex-a72 -m 512 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-blk-device,drive=ext2 \
        -drive if=none,id=ext2,format=raw,readonly=on,file="$EXT2_DISK" \
        -serial file:"$OUTPUT_DIR/serial.txt" &
    local QEMU_PID=$!

    # Wait for USERSPACE shell prompt (20s timeout)
    # ONLY accept "breenix>" - the actual userspace shell prompt
    # DO NOT accept "Interactive Shell" - that's the KERNEL FALLBACK when userspace FAILS
    local BOOT_COMPLETE=false
    for i in $(seq 1 10); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if grep -q "breenix>" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                BOOT_COMPLETE=true
                break
            fi
            if grep -qiE "(KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                break
            fi
        fi
        sleep 2
    done

    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true

    if $BOOT_COMPLETE; then
        # Verify no excessive init_shell spawning
        local SHELL_COUNT=$(grep -o "init_shell" "$OUTPUT_DIR/serial.txt" 2>/dev/null | wc -l | tr -d ' ')
        SHELL_COUNT=${SHELL_COUNT:-0}
        if [ "$SHELL_COUNT" -le 5 ]; then
            echo "SUCCESS (${SHELL_COUNT} init_shell mentions)"
            return 0
        else
            echo "FAIL: Too many init_shell mentions: $SHELL_COUNT"
            return 1
        fi
    else
        local LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
        if grep -qiE "(KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "FAIL: Kernel panic ($LINES lines)"
        else
            echo "FAIL: Shell not detected ($LINES lines)"
        fi
        return 1
    fi
}

echo "========================================="
echo "ARM64 Boot Test (Native QEMU)"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo ""

for attempt in $(seq 1 $MAX_RETRIES); do
    echo "Attempt $attempt/$MAX_RETRIES..."
    if run_single_test; then
        echo ""
        echo "========================================="
        echo "ARM64 BOOT TEST: PASSED"
        echo "========================================="
        exit 0
    fi
    if [ $attempt -lt $MAX_RETRIES ]; then
        echo "Retrying..."
        sleep 1
    fi
done

echo ""
echo "========================================="
echo "ARM64 BOOT TEST: FAILED (after $MAX_RETRIES attempts)"
echo "========================================="
echo ""
echo "NOTE: If this test frequently requires retries or fails repeatedly,"
echo "there may be a regression. Check recent changes to boot code."
echo ""
echo "Last output:"
tail -10 /tmp/breenix_aarch64_boot_native/serial.txt 2>/dev/null || echo "(no output)"
exit 1
