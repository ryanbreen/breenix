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

# Find ext2 disk (required for userspace)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

# Check serial output for crash markers. Prints the crash type and returns 0
# if a crash is found, 1 if clean.
check_crash_markers() {
    local serial_file="$1"
    [ -f "$serial_file" ] || return 1
    if grep -qiE "(KERNEL PANIC|panic!)" "$serial_file" 2>/dev/null; then
        echo "Kernel panic"
        return 0
    fi
    if grep -qiE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception)" "$serial_file" 2>/dev/null; then
        echo "CPU exception"
        return 0
    fi
    if grep -qiE "soft lockup detected" "$serial_file" 2>/dev/null; then
        echo "Soft lockup"
        return 0
    fi
    return 1
}

run_single_test() {
    local OUTPUT_DIR="/tmp/breenix_aarch64_boot_native"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Create writable copy of ext2 disk to allow filesystem write tests
    local EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"

    # Run QEMU with 30s timeout
    # Always include GPU, keyboard, and network so kernel VirtIO enumeration finds them
    # Use writable disk copy (no readonly=on) to allow filesystem writes
    timeout 30 qemu-system-aarch64 \
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
    local QEMU_PID=$!

    # Wait for USERSPACE boot completion (20s timeout)
    # Accept any of:
    #   "breenix>" or "bsh " - shell prompt on serial (legacy/direct mode)
    #   "[bwm] Display:" - BWM window manager initialized (shell runs inside PTY)
    # DO NOT accept "Interactive Shell" - that's the KERNEL FALLBACK when userspace FAILS
    local BOOT_COMPLETE=false
    local CRASH_TYPE=""
    for i in $(seq 1 10); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if grep -qE "(breenix>|bsh |\[bwm\] Display:)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                BOOT_COMPLETE=true
                break
            fi
            if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
                break
            fi
        fi
        sleep 2
    done

    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true

    # Even if boot appeared successful, scan for crash markers that may have
    # appeared after the shell prompt (e.g., a child process crashed).
    if $BOOT_COMPLETE; then
        if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
            local LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
            echo "FAIL: $CRASH_TYPE after boot ($LINES lines)"
            return 1
        fi
        echo "SUCCESS"
        return 0
    else
        local LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
        if [ -n "$CRASH_TYPE" ]; then
            echo "FAIL: $CRASH_TYPE ($LINES lines)"
        else
            echo "FAIL: Userspace not detected ($LINES lines)"
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
