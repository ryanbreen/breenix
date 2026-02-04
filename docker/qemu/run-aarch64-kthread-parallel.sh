#!/bin/bash
# Run N parallel native ARM64 kthread tests
#
# This script stress tests the ARM64 threading subsystem by running multiple
# QEMU instances in parallel. It validates scheduler, context switching, and
# locks under load.
#
# Note: ARM64 kthread_test_only feature is not yet implemented in main_aarch64.rs.
# This script tests the boot+userspace path which exercises kthreads, scheduler,
# and context switching. When kthread_test_only is added to ARM64, update the
# kernel build command and success marker accordingly.
#
# Usage: ./run-aarch64-kthread-parallel.sh [count]
#
# Examples:
#   ./run-aarch64-kthread-parallel.sh      # Run 10 parallel tests (default)
#   ./run-aarch64-kthread-parallel.sh 5    # Run 5 parallel tests

set -e

COUNT=${1:-10}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Find ext2 disk (required for init_shell which exercises threading)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

echo "Running $COUNT parallel ARM64 kthread tests..."
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo ""

# Array to track QEMU PIDs
declare -a QEMU_PIDS

# Create output directories and launch QEMU instances
for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="/tmp/breenix_aarch64_kthread_$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Create writable copy of ext2 disk for each instance
    EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"

    # Run QEMU natively (ARM64 runs natively on macOS ARM64)
    # No Docker needed - much faster than x86-64 emulation
    timeout 60 qemu-system-aarch64 \
        -M virt -cpu cortex-a72 -m 512 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-blk-device,drive=ext2 \
        -drive if=none,id=ext2,format=raw,file="$EXT2_WRITABLE" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$OUTPUT_DIR/serial.txt" &>/dev/null &
    QEMU_PIDS[$i]=$!
    echo "  Started test $i (PID ${QEMU_PIDS[$i]})"
done

# Wait for all to complete (with timeout)
echo ""
echo "Waiting for tests to complete (60s timeout)..."
PASSED=0
FAILED=0

for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="/tmp/breenix_aarch64_kthread_$i"

    # Wait up to 60 seconds for this test
    # Look for userspace shell prompt "breenix>" which indicates:
    # - Scheduler initialized successfully
    # - Context switching works (idle thread -> init_shell)
    # - Timer interrupts firing correctly
    # - Per-CPU data working
    #
    # When ARM64 kthread_test_only is implemented, change this to:
    # grep -q "KTHREAD_TEST_ONLY_COMPLETE"
    FOUND=false
    for j in $(seq 1 30); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if grep -q "breenix>" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                FOUND=true
                break
            fi
            # Also check for kernel panic
            if grep -qiE "(KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
                break
            fi
        fi
        sleep 2
    done

    # Kill the QEMU instance if still running
    kill ${QEMU_PIDS[$i]} 2>/dev/null || true
    wait ${QEMU_PIDS[$i]} 2>/dev/null || true

    if $FOUND; then
        # Verify no excessive init_shell spawning (would indicate scheduler bugs)
        SHELL_COUNT=$(grep -o "init_shell" "$OUTPUT_DIR/serial.txt" 2>/dev/null | wc -l | tr -d ' ')
        SHELL_COUNT=${SHELL_COUNT:-0}
        if [ "$SHELL_COUNT" -le 5 ]; then
            echo "  Test $i: PASS (${SHELL_COUNT} init_shell mentions)"
            PASSED=$((PASSED + 1))
        else
            echo "  Test $i: FAIL (too many init_shell: $SHELL_COUNT)"
            FAILED=$((FAILED + 1))
        fi
    else
        if [ -f "$OUTPUT_DIR/serial.txt" ] && grep -qiE "(KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "  Test $i: FAIL (kernel panic)"
        else
            echo "  Test $i: TIMEOUT"
        fi
        FAILED=$((FAILED + 1))
    fi
done

echo ""
echo "========================================="
echo "ARM64 Kthread Parallel Test Results"
echo "========================================="
echo "Passed: $PASSED"
echo "Failed: $FAILED"
echo "Total:  $COUNT"
echo ""
echo "Output logs in: /tmp/breenix_aarch64_kthread_*/"
echo "========================================="

if [ $FAILED -eq 0 ]; then
    exit 0
else
    exit 1
fi
