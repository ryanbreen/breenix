#!/bin/bash
# Run N native ARM64 kthread boots, one host aarch64 QEMU at a time.
#
# This script stress tests the ARM64 threading subsystem by running multiple
# QEMU instances back to back. It validates scheduler, context switching, and
# locks under load.
#
# #826/R181: this script used to launch its N qemu-system-aarch64 processes
# CONCURRENTLY (a launch loop backgrounding them together, then a separate
# wait/verdict loop polling each one's output for up to 60s) -- exactly the
# shape #826 measured driving the guest clock down to 37-53% of wall-clock
# on this host. The two loops are merged into one: each boot is launched,
# polled, killed and verified before the next one starts, serialized through
# qemu_host_lock_acquire/qemu_host_lock_release the same way the other
# aarch64 gate scripts under docker/qemu/ do. This is a real behavior change, not just a wording one: with
# N processes down to 1 at a time, total wall-clock for COUNT boots is now
# roughly COUNT times one boot's duration rather than bounded by the
# slowest of N run together, and each boot's 60-poll search window now
# starts at THAT boot's own launch instant rather than at a fixed offset
# from when the (formerly parallel) batch began -- the previous two-loop
# shape would have falsely TIMEOUT'd boots that simply had not been
# launched yet once launches were serialized behind a lock, since a boot's
# 60-sample window used to run concurrently with the other boots' windows in
# the same batch, not starting from its own launch.
#
# Note: ARM64 kthread_test_only feature is not yet implemented in main_aarch64.rs.
# This script tests the boot+userspace path which exercises kthreads, scheduler,
# and context switching. When kthread_test_only is added to ARM64, update the
# kernel build command and success marker accordingly.
#
# Usage: ./run-aarch64-kthread-parallel.sh [count]
#
# Examples:
#   ./run-aarch64-kthread-parallel.sh      # Run 10 sequential tests (default)
#   ./run-aarch64-kthread-parallel.sh 5    # Run 5 sequential tests

set -e

COUNT=${1:-10}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"
# #825: two concurrent invocations of this script on the same host each
# hardcoded the identical /tmp/breenix_aarch64_kthread_$i paths (reconstructed
# independently in the launch loop and the wait/verdict loop below, the same
# duplication PR #801 found in its x86 twin run-kthread-parallel.sh for
# #797), so one invocation's rm -rf/mkdir could delete and rewrite the serial
# another invocation's poll loop was mid-boot scoring. Defaulting to /tmp
# keeps a caller that leaves it unset byte-identical; a concurrent-lane launcher sets
# this to a per-worktree directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: a relative value would resolve against whatever
# directory happens to be current when each loop below runs (the same F6
# guard PR #801 gave the x86 gate scripts for #797).
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP"; exit 1 ;;
esac

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Find ext2 disk (required for init_shell which exercises threading)
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

echo "Running $COUNT sequential ARM64 kthread tests (one host aarch64 QEMU at a time)..."
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo ""

# Array to track QEMU PIDs (kept for the closing "Output logs" message's
# shape parity with the rest of this file's history; each entry is set and
# consumed within the same iteration below, not read back in a later one).
declare -a QEMU_PIDS
PASSED=0
FAILED=0

for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_aarch64_kthread_$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"

    # Create writable copy of ext2 disk for each instance
    EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"

    # Run QEMU natively (ARM64 runs natively on macOS ARM64)
    # No Docker needed - much faster than x86-64 emulation
    qemu_host_lock_acquire
    timeout 60 qemu-system-aarch64 \
        -M virt -cpu cortex-a72 -m 512 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-blk-device,drive=ext2 \
        -drive if=none,id=ext2,format=raw,file="$EXT2_WRITABLE" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$OUTPUT_DIR/serial.txt" &>/dev/null &
    QEMU_PIDS[$i]=$!
    echo "  Started test $i (PID ${QEMU_PIDS[$i]})"

    # Wait up to 60 seconds for this test, starting from ITS OWN launch above.
    # Look for userspace shell prompt ("breenix>" or "bsh ") which indicates:
    # - Scheduler initialized successfully
    # - Context switching works (idle thread -> shell)
    # - Timer interrupts firing correctly
    # - Per-CPU data working
    #
    # When ARM64 kthread_test_only is implemented, change this to:
    # grep -q "KTHREAD_TEST_ONLY_COMPLETE"
    FOUND=false
    for j in $(seq 1 30); do
        if [ -f "$OUTPUT_DIR/serial.txt" ]; then
            if grep -qE "(breenix>|bsh )" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
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
    qemu_host_lock_release

    if $FOUND; then
        # Verify no excessive shell spawning (would indicate scheduler bugs)
        SHELL_COUNT=$(grep -oE "(init_shell|/bin/bsh)" "$OUTPUT_DIR/serial.txt" 2>/dev/null | wc -l | tr -d ' ')
        SHELL_COUNT=${SHELL_COUNT:-0}
        if [ "$SHELL_COUNT" -le 5 ]; then
            echo "  Test $i: PASS (${SHELL_COUNT} shell mentions)"
            PASSED=$((PASSED + 1))
        else
            echo "  Test $i: FAIL (too many shell spawns: $SHELL_COUNT)"
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
echo "Output logs in: $BREENIX_GATE_TMP/breenix_aarch64_kthread_*/"
echo "========================================="

if [ $FAILED -eq 0 ]; then
    exit 0
else
    exit 1
fi
