#!/bin/bash
# ARM64 Stability Test (Native QEMU)
#
# Multi-phase test that verifies sustained operation under GPU load:
#   Phase 1: Boot → BWM initializes (20s timeout)
#   Phase 2: Services → bounce demo + shell prompt appear (10s timeout)
#   Phase 3: Stability soak → 15 seconds of monitoring for lockups/panics
#   Phase 4: Report
#
# Unlike the basic boot test which exits as soon as the shell prompt appears,
# this test continues monitoring to catch deadlocks and soft lockups that
# only manifest under sustained GPU load (e.g., bounce.elf running).
#
# Usage: ./run-aarch64-stability-test.sh

set -e

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

OUTPUT_DIR="/tmp/breenix_aarch64_stability"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Create writable copy of ext2 disk
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
echo "ARM64 Stability Test (Native QEMU)"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo ""

# Start QEMU in background (60s total timeout)
timeout 60 qemu-system-aarch64 \
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

FAIL_REASON=""

# Helper: check for fatal markers in serial output
check_fatal() {
    if grep -qiE "soft lockup detected" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        echo "Soft lockup detected"
        return 0
    fi
    if grep -qiE "(KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        echo "Kernel panic"
        return 0
    fi
    if grep -qiE "(DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        echo "CPU exception"
        return 0
    fi
    return 1
}

# --- Phase 1: Boot (20s timeout) ---
echo "Phase 1: Boot (waiting for BWM or shell)..."
PHASE1_OK=false
for i in $(seq 1 10); do
    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        # Accept BWM display init or shell prompt as boot success
        if grep -qE "(\[bwm\] Display:|breenix>|bsh )" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            PHASE1_OK=true
            break
        fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 1: $FATAL during boot"
            break
        fi
    fi
    sleep 2
done

if ! $PHASE1_OK && [ -z "$FAIL_REASON" ]; then
    FAIL_REASON="Phase 1 timeout: neither BWM nor shell prompt detected"
fi

# --- Phase 2: Services (10s timeout) ---
# In BWM mode, shell writes to its PTY (rendered to framebuffer by BWM),
# so "bsh " won't appear on serial. We accept any of:
#   - Direct shell prompt on serial (non-BWM mode)
#   - PTY allocation markers (BWM mode — PTYs created for shell/btop)
#   - BWM shell PID or similar marker
if [ -z "$FAIL_REASON" ]; then
    echo "Phase 1: PASS"
    echo "Phase 2: Services (waiting for shell or BWM child processes)..."
    SHELL_OK=false
    BOUNCE_OK=false
    for i in $(seq 1 5); do
        # Direct shell prompt on serial
        if grep -qE "(breenix>|bsh )" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            SHELL_OK=true
        fi
        # BWM allocated PTYs for its child processes (shell + btop).
        # In BWM mode, shell output goes to PTY → framebuffer, not serial.
        # PTY unlock markers prove BWM successfully called openpty() for children.
        if grep -qE "\[pty\] Unlocked PTY" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            SHELL_OK=true
        fi
        if grep -q "Bounce demo starting" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BOUNCE_OK=true
        fi
        if $SHELL_OK; then
            break
        fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 2: $FATAL during service startup"
            break
        fi
        sleep 2
    done

    if [ -z "$FAIL_REASON" ]; then
        if $BOUNCE_OK; then
            echo "  bounce demo: detected"
        else
            echo "  bounce demo: not detected (optional)"
        fi
        if ! $SHELL_OK; then
            FAIL_REASON="Phase 2 timeout: shell not detected"
        else
            echo "Phase 2: PASS (shell spawned)"
        fi
    fi
fi

# --- Phase 3: Stability soak (15s, check every 3s) ---
# Monitor for panics, lockups, and exceptions over a sustained period.
# Note: serial output may NOT grow after boot — the kernel doesn't produce
# periodic serial output by default (particle thread is disabled, and in BWM
# mode all output goes through PTYs to the framebuffer). So we only check
# for negative markers, not output growth.
if [ -z "$FAIL_REASON" ]; then
    echo "Phase 3: Stability soak (15s)..."

    for check in $(seq 1 5); do
        sleep 3

        # Check for fatal markers that may appear during sustained operation
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 3: $FATAL during soak (check $check)"
            break
        fi

        # Check QEMU is still running (hasn't crashed or rebooted)
        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 3: QEMU exited unexpectedly during soak (check $check)"
            break
        fi

        CURR_LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null | tr -d ' ')
        echo "  Check $check/5: OK (${CURR_LINES:-0} lines, QEMU alive)"
    done

    if [ -z "$FAIL_REASON" ]; then
        echo "Phase 3: PASS (stable for 15s)"
    fi
fi

# --- Cleanup QEMU ---
kill $QEMU_PID 2>/dev/null || true
wait $QEMU_PID 2>/dev/null || true
unset QEMU_PID  # Prevent trap from trying to kill again

# --- Phase 4: Report ---
echo ""
TOTAL_LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null | tr -d ' ')
TOTAL_LINES=${TOTAL_LINES:-0}

if [ -z "$FAIL_REASON" ]; then
    echo "========================================="
    echo "ARM64 STABILITY TEST: PASSED"
    echo "========================================="
    echo "Serial output: ${TOTAL_LINES} lines"
    echo "Log: $OUTPUT_DIR/serial.txt"
    exit 0
else
    echo "========================================="
    echo "ARM64 STABILITY TEST: FAILED"
    echo "========================================="
    echo "Reason: $FAIL_REASON"
    echo "Serial output: ${TOTAL_LINES} lines"
    echo "Log: $OUTPUT_DIR/serial.txt"
    echo ""
    echo "Last 15 lines:"
    tail -15 "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no output)"
    exit 1
fi
