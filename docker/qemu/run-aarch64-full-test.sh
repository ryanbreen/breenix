#!/bin/bash
# ARM64 Full System Test (Native QEMU)
#
# This test matches the manual workflow of running ./run.sh:
#   Phase 1: Boot and run all 84 subsystem tests (wait for [BOOT_TESTS:PASS])
#   Phase 2: Verify BWM shell is up and services launched
#   Phase 3: Wait for bounce demo under GPU load (10+ seconds)
#   Phase 4: Verify kernel is still alive — no crashes during sustained operation
#
# This is the REAL test. Unlike boot-test-native.sh which exits at the shell
# prompt, this test waits for the full 84-test suite to complete and then
# monitors sustained operation under GPU load.
#
# Usage: ./run-aarch64-full-test.sh [--rebuild]
#
# Options:
#   --rebuild   Force rebuild of the kernel before testing

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Parse args
REBUILD=false
for arg in "$@"; do
    case "$arg" in
        --rebuild) REBUILD=true ;;
    esac
done

# Optionally rebuild
if $REBUILD; then
    echo "Building ARM64 kernel with boot_tests feature..."
    (cd "$BREENIX_ROOT" && cargo build --release --features boot_tests \
        --target aarch64-breenix.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
        -p kernel --bin kernel-aarch64 2>&1)
    echo "Build complete."
    echo ""
fi

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found at $KERNEL"
    echo "Build with: cargo build --release --features boot_tests --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

# Find ext2 disk
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

OUTPUT_DIR="/tmp/breenix_aarch64_full_test"
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
echo "ARM64 Full System Test"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo ""

# Start QEMU in background (120s total timeout — 84 tests need time)
timeout 120 qemu-system-aarch64 \
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
TESTS_PASSED=0
TESTS_TOTAL=0
TEST_FAILURES=""

# Helper: check for fatal markers
check_fatal() {
    local serial="$OUTPUT_DIR/serial.txt"
    if grep -qiE "soft lockup detected" "$serial" 2>/dev/null; then
        echo "Soft lockup detected"
        return 0
    fi
    if grep -qiE "(KERNEL PANIC|panic!)" "$serial" 2>/dev/null; then
        echo "Kernel panic"
        return 0
    fi
    if grep -qiE "DATA_ABORT.*FAR=" "$serial" 2>/dev/null; then
        echo "DATA_ABORT"
        return 0
    fi
    if grep -qiE "INSTRUCTION_ABORT" "$serial" 2>/dev/null; then
        echo "INSTRUCTION_ABORT"
        return 0
    fi
    if grep -qiE "Unhandled sync exception" "$serial" 2>/dev/null; then
        echo "Unhandled exception"
        return 0
    fi
    return 1
}

# --- Phase 1: Run all 84 subsystem tests (up to 90s) ---
echo "Phase 1: Running 84 subsystem tests..."
echo "  (Waiting for [BOOT_TESTS:PASS] or [BOOT_TESTS:FAIL])"
PHASE1_OK=false
for i in $(seq 1 45); do  # 45 * 2s = 90s timeout
    if ! kill -0 $QEMU_PID 2>/dev/null; then
        FAIL_REASON="Phase 1: QEMU exited before tests completed"
        break
    fi

    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        # Check for test suite completion
        if grep -q "\[BOOT_TESTS:PASS\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            PHASE1_OK=true
            TESTS_PASSED=$(grep '\[TESTS_COMPLETE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | sed 's/.*\[TESTS_COMPLETE:\([0-9]*\).*/\1/' | tail -1)
            TESTS_TOTAL=$(grep '\[TESTS_COMPLETE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | sed 's/.*\[TESTS_COMPLETE:[0-9]*\/\([0-9]*\).*/\1/' | tail -1)
            echo "  All tests passed: ${TESTS_PASSED:-?}/${TESTS_TOTAL:-?}"
            break
        fi

        # Check for test suite failure (tests ran but some failed)
        if grep -q "\[BOOT_TESTS:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            TEST_FAILURES=$(grep "\[TEST:.*:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null || true)
            TESTS_PASSED=$(grep '\[TESTS_COMPLETE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | sed 's/.*\[TESTS_COMPLETE:\([0-9]*\).*/\1/' | tail -1)
            TESTS_TOTAL=$(grep '\[TESTS_COMPLETE:' "$OUTPUT_DIR/serial.txt" 2>/dev/null | sed 's/.*\[TESTS_COMPLETE:[0-9]*\/\([0-9]*\).*/\1/' | tail -1)
            FAIL_REASON="Phase 1: Test suite completed with failures (${TESTS_PASSED:-?}/${TESTS_TOTAL:-?})"
            break
        fi

        # Check for crash during tests
        if FATAL=$(check_fatal); then
            # Report which test was running when crash happened
            LAST_TEST=$(grep "\[TEST:.*:START\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null | tail -1)
            FAIL_REASON="Phase 1: $FATAL during test execution. Last test: $LAST_TEST"
            break
        fi

        # Progress indicator every 10 seconds
        if (( i % 5 == 0 )); then
            COMPLETED=$(grep -c "\[TEST:.*:PASS\]" "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
            FAILED=$(grep -c "\[TEST:.*:FAIL" "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo 0)
            echo "  Progress: ${COMPLETED} passed, ${FAILED} failed (${i}*2s elapsed)"
        fi
    fi
    sleep 2
done

if ! $PHASE1_OK && [ -z "$FAIL_REASON" ]; then
    FAIL_REASON="Phase 1 timeout: tests did not complete within 90s"
fi

# --- Phase 2: Verify services (10s) ---
if [ -z "$FAIL_REASON" ]; then
    echo "Phase 1: PASS (${TESTS_PASSED}/${TESTS_TOTAL} tests)"
    echo ""
    echo "Phase 2: Checking services..."
    SHELL_OK=false
    BWM_OK=false
    for i in $(seq 1 5); do
        if grep -qE "(breenix>|bsh )" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            SHELL_OK=true
        fi
        if grep -qE "\[pty\] Unlocked PTY" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            SHELL_OK=true
        fi
        if grep -qE "\[bwm\] Display:" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BWM_OK=true
        fi
        if $SHELL_OK; then break; fi
        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 2: $FATAL during service startup"
            break
        fi
        sleep 2
    done

    if [ -z "$FAIL_REASON" ]; then
        if $BWM_OK; then echo "  BWM: running"; fi
        if $SHELL_OK; then
            echo "Phase 2: PASS (shell spawned)"
        else
            FAIL_REASON="Phase 2 timeout: shell not detected"
        fi
    fi
fi

# --- Phase 3: Sustained operation under GPU load (15s) ---
# This catches the crashes that only manifest under sustained GPU rendering
# (e.g., bounce demo, btop updating, BWM rendering test progress).
if [ -z "$FAIL_REASON" ]; then
    echo ""
    echo "Phase 3: Sustained operation soak (15 seconds)..."
    for check in $(seq 1 5); do
        sleep 3

        if FATAL=$(check_fatal); then
            FAIL_REASON="Phase 3: $FATAL during sustained operation (check $check/5)"
            break
        fi

        if ! kill -0 $QEMU_PID 2>/dev/null; then
            FAIL_REASON="Phase 3: QEMU exited unexpectedly (check $check/5)"
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
unset QEMU_PID

# --- Report ---
echo ""
TOTAL_LINES=$(wc -l < "$OUTPUT_DIR/serial.txt" 2>/dev/null | tr -d ' ')
TOTAL_LINES=${TOTAL_LINES:-0}

if [ -z "$FAIL_REASON" ]; then
    echo "========================================="
    echo "ARM64 FULL SYSTEM TEST: PASSED"
    echo "========================================="
    echo "Tests: ${TESTS_PASSED}/${TESTS_TOTAL} passed"
    echo "Stability: 15s soak clean"
    echo "Serial: ${TOTAL_LINES} lines"
    echo "Log: $OUTPUT_DIR/serial.txt"
    exit 0
else
    echo "========================================="
    echo "ARM64 FULL SYSTEM TEST: FAILED"
    echo "========================================="
    echo "Reason: $FAIL_REASON"
    if [ -n "$TEST_FAILURES" ]; then
        echo ""
        echo "Failed tests:"
        echo "$TEST_FAILURES"
    fi
    echo ""
    echo "Serial: ${TOTAL_LINES} lines"
    echo "Log: $OUTPUT_DIR/serial.txt"
    echo ""
    echo "Last 20 lines of serial output:"
    tail -20 "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no output)"
    exit 1
fi
