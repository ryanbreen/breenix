#!/bin/bash
# ARM64 Userspace Test Suite Runner
# 
# This script runs a batch of ARM64 userspace tests by modifying the kernel
# to load each test binary directly (instead of init_shell), then running QEMU.
#
# Usage: ./run-aarch64-test-suite.sh [test_name ...] | --all
#
# Examples:
#   ./run-aarch64-test-suite.sh clock_gettime_test fork_test
#   ./run-aarch64-test-suite.sh --all    # Run all tests

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
KERNEL_SRC="$BREENIX_ROOT/kernel/src/main_aarch64.rs"
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"

# Output directory for results
RESULTS_DIR="/tmp/breenix_arm64_test_results"
rm -rf "$RESULTS_DIR"
mkdir -p "$RESULTS_DIR"

# Create a writable copy of the ext2 disk for tests that need write access
# This prevents corruption of the master image while allowing write tests to pass
EXT2_DISK_WRITABLE="$RESULTS_DIR/ext2-writable.img"
echo "Creating writable copy of ext2 disk..."
cp "$EXT2_DISK" "$EXT2_DISK_WRITABLE"

# Get list of tests
if [ "$1" = "--all" ]; then
    # Get all test binaries from ext2 disk
    TESTS=$(docker run --rm --privileged \
        -v "$EXT2_DISK:/ext2.img:ro" \
        alpine:latest \
        sh -c '
            apk add --no-cache e2fsprogs >/dev/null 2>&1
            mkdir -p /mnt/ext2
            mount -o ro /ext2.img /mnt/ext2
            ls /mnt/ext2/bin/ | grep -E "_test$" | sort
        ' 2>/dev/null)
elif [ -n "$1" ]; then
    TESTS="$@"
else
    echo "Usage: $0 [test_name ...] | --all"
    echo ""
    echo "Examples:"
    echo "  $0 clock_gettime_test fork_test  # Run specific tests"
    echo "  $0 --all                          # Run all *_test binaries"
    exit 1
fi

# Save original kernel source
cp "$KERNEL_SRC" "$KERNEL_SRC.bak"

# Trap to restore on exit
restore_kernel() {
    if [ -f "$KERNEL_SRC.bak" ]; then
        mv "$KERNEL_SRC.bak" "$KERNEL_SRC"
    fi
}
trap restore_kernel EXIT

# Results tracking
PASSED=0
FAILED=0
SKIPPED=0
PASSED_TESTS=""
FAILED_TESTS=""

run_test() {
    local test_name="$1"
    local output_file="$RESULTS_DIR/${test_name}.txt"

    echo ""
    echo "=========================================="
    echo "Running: $test_name"
    echo "=========================================="

    # Restore original kernel source
    cp "$KERNEL_SRC.bak" "$KERNEL_SRC"

    # Reset writable ext2 disk to clean state for each test
    cp "$EXT2_DISK" "$EXT2_DISK_WRITABLE"
    
    # Modify kernel to load this test using Python for reliable replacement
    python3 - "$KERNEL_SRC" "$test_name" << 'PYTHON'
import sys
import re
kernel_src = sys.argv[1]
test_name = sys.argv[2]
with open(kernel_src, 'r') as f:
    content = f.read()
# Replace the path in run_userspace_from_ext2 call
content = re.sub(
    r'run_userspace_from_ext2\("/bin/init_shell"\)',
    f'run_userspace_from_ext2("/bin/{test_name}")',
    content
)
with open(kernel_src, 'w') as f:
    f.write(content)
print(f"Modified kernel to load: /bin/{test_name}")
PYTHON
    
    # Build (with testing feature to enable exec() and other test syscalls)
    echo "Building kernel..."
    if ! cargo build --release --features testing --target aarch64-breenix.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
        -p kernel --bin kernel-aarch64 2>&1 | tail -3; then
        echo "BUILD FAILED for $test_name"
        FAILED=$((FAILED + 1))
        FAILED_TESTS="$FAILED_TESTS $test_name"
        echo "RESULT: BUILD_FAILED" > "$output_file"
        return 1
    fi
    
    # Run QEMU with timeout (using writable ext2 copy to allow write tests)
    echo "Running test..."
    timeout 30 qemu-system-aarch64 \
        -M virt -cpu cortex-a72 -m 512 \
        -kernel "$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-blk-device,drive=ext2 \
        -drive if=none,id=ext2,format=raw,file="$EXT2_DISK_WRITABLE" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$output_file" 2>&1 &
    QEMU_PID=$!
    
    # Wait for test to complete (look for exit marker)
    for i in $(seq 1 25); do
        if grep -qE "exit\([0-9]+\)|Userspace Test Complete|Test Summary|RESULT:" "$output_file" 2>/dev/null; then
            sleep 2
            break
        fi
        sleep 1
    done
    
    kill $QEMU_PID 2>/dev/null || true
    wait $QEMU_PID 2>/dev/null || true
    
    # Check result
    local exit_code=""
    if grep -qE "exit\(0\)" "$output_file" 2>/dev/null; then
        exit_code="0"
    elif grep -qoE "exit\([0-9]+\)" "$output_file" 2>/dev/null; then
        exit_code=$(grep -oE "exit\([0-9]+\)" "$output_file" | head -1 | grep -oE "[0-9]+")
    fi
    
    if [ "$exit_code" = "0" ]; then
        echo "RESULT: PASS (exit 0)"
        PASSED=$((PASSED + 1))
        PASSED_TESTS="$PASSED_TESTS $test_name"
    elif grep -qE "FAIL|panic|exception|abort|Failed:" "$output_file" 2>/dev/null; then
        echo "RESULT: FAIL"
        FAILED=$((FAILED + 1))
        FAILED_TESTS="$FAILED_TESTS $test_name"
        # Show last 10 lines of output for debugging
        echo "--- Last 10 lines ---"
        tail -10 "$output_file"
    elif grep -qE "Failed to load|not found" "$output_file" 2>/dev/null; then
        echo "RESULT: SKIP (binary not found)"
        SKIPPED=$((SKIPPED + 1))
    else
        echo "RESULT: UNKNOWN"
        SKIPPED=$((SKIPPED + 1))
        # Show last 10 lines
        echo "--- Last 10 lines ---"
        tail -10 "$output_file"
    fi
}

# Change to breenix root for cargo
cd "$BREENIX_ROOT"

# Run each test
for test in $TESTS; do
    run_test "$test"
done

# Summary
echo ""
echo "=========================================="
echo "TEST SUITE SUMMARY"
echo "=========================================="
echo "Passed:  $PASSED"
echo "Failed:  $FAILED"
echo "Unknown: $SKIPPED"
echo "Total:   $((PASSED + FAILED + SKIPPED))"
echo ""
if [ -n "$PASSED_TESTS" ]; then
    echo "PASSED:$PASSED_TESTS"
fi
if [ -n "$FAILED_TESTS" ]; then
    echo "FAILED:$FAILED_TESTS"
fi
echo ""
echo "Results saved in: $RESULTS_DIR"

# Exit with failure if any tests failed
if [ $FAILED -gt 0 ]; then
    exit 1
fi
