#!/bin/bash
# Strict ARM64 boot test - runs multiple iterations and requires ALL to pass
# Used for CI to catch regressions. Does NOT retry failed boots.
#
# Unlike run-aarch64-boot-test-native.sh which uses retries (masking failures),
# this test counts every boot attempt. A single failure means the test fails.
#
# Usage: ./run-aarch64-boot-test-strict.sh [iterations]
#        Default: 20 iterations
#
# Exit codes:
#   0 - All iterations passed
#   1 - One or more iterations failed

set -e

ITERATIONS=${1:-20}
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

# Track results
SUCCESSES=0
FAILURES=0
FAILED_ITERATIONS=""

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
    local iteration=$1
    local OUTPUT_DIR="/tmp/breenix_aarch64_strict_$iteration"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Create writable copy of ext2 disk to allow filesystem write tests
    local EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"

    # Run QEMU with 20s timeout
    # Always include GPU, keyboard, and network so kernel VirtIO enumeration finds them
    # Use writable disk copy (no readonly=on) to allow filesystem writes
    timeout 20 qemu-system-aarch64 \
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

    # Wait for USERSPACE boot completion (18s max, checking every 1.5s)
    # Accept any of:
    #   "breenix>" or "bsh " - shell prompt on serial (legacy/direct mode)
    #   "[bwm] Display:" - BWM window manager initialized (shell runs inside PTY)
    # DO NOT accept "Interactive Shell" - that's the KERNEL FALLBACK when userspace FAILS
    local BOOT_COMPLETE=false
    local CRASH_TYPE=""
    for i in $(seq 1 12); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if grep -qE "(breenix>|bsh |\[bwm\] Display:)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                BOOT_COMPLETE=true
                break
            fi
            if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
                break
            fi
        fi
        sleep 1.5
    done

    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true

    if $BOOT_COMPLETE; then
        # Even if boot appeared successful, scan for crash markers
        if CRASH_TYPE=$(check_crash_markers "$OUTPUT_DIR/serial.txt"); then
            local LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
            echo "  [FAIL] Boot $iteration: $CRASH_TYPE after boot ($LINES lines)"
            return 1
        fi
        echo "  [OK] Boot $iteration: SUCCESS"
        return 0
    else
        local LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
        if [ -n "$CRASH_TYPE" ]; then
            echo "  [FAIL] Boot $iteration: $CRASH_TYPE ($LINES lines)"
        else
            echo "  [FAIL] Boot $iteration: Userspace not detected ($LINES lines)"
        fi
        return 1
    fi
}

echo "========================================="
echo "ARM64 Strict Boot Test"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo "Iterations: $ITERATIONS"
echo "Requirement: 100% success rate (all $ITERATIONS must pass)"
echo ""
echo "Running tests..."
echo ""

START_TIME=$(date +%s)

for i in $(seq 1 $ITERATIONS); do
    if run_single_test $i; then
        SUCCESSES=$((SUCCESSES + 1))
    else
        FAILURES=$((FAILURES + 1))
        FAILED_ITERATIONS="$FAILED_ITERATIONS $i"
    fi
done

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

echo ""
echo "========================================="
echo "RESULTS"
echo "========================================="
echo "Total iterations: $ITERATIONS"
echo "Successes: $SUCCESSES"
echo "Failures: $FAILURES"
echo "Success rate: $(( (SUCCESSES * 100) / ITERATIONS ))%"
echo "Duration: ${DURATION}s"

if [ $FAILURES -eq 0 ]; then
    echo ""
    echo "========================================="
    echo "PASS: $SUCCESSES/$ITERATIONS boots succeeded"
    echo "========================================="
    exit 0
else
    echo ""
    echo "Failed iterations:$FAILED_ITERATIONS"
    echo ""
    echo "========================================="
    echo "FAIL: Only $SUCCESSES/$ITERATIONS boots succeeded"
    echo "========================================="
    echo ""
    echo "This indicates a regression or timing bug that needs investigation."
    echo "Serial output from failed boots can be found in /tmp/breenix_aarch64_strict_N/"
    exit 1
fi
