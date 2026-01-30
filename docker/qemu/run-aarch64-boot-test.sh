#!/bin/bash
# Run ARM64 boot test with verification
# Checks for:
# 1. Successful boot (shell startup)
# 2. No multiple init_shell processes (regression test)
# 3. No crashes/panics/exceptions
#
# Usage: ./run-aarch64-boot-test.sh

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

# Find ext2 disk (required for init_shell)
# NOTE: The test_binaries disk is NOT included because it triggers a VirtIO
# block driver bug on ARM64 that causes crashes during sector reads.
# This boot test only needs the ext2 disk to verify init_shell starts.
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"

if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    echo "Build it with: ./scripts/build_ext2_arm64.sh"
    exit 1
fi

echo "========================================="
echo "ARM64 Boot Test"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo ""

# Create output directory
OUTPUT_DIR="/tmp/breenix_aarch64_boot"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Build the ARM64 Docker image if not exists
if ! docker images breenix-qemu-aarch64 --format "{{.Repository}}" | grep -q breenix-qemu-aarch64; then
    echo "Building ARM64 Docker image..."
    docker build -t breenix-qemu-aarch64 -f "$SCRIPT_DIR/Dockerfile.aarch64" "$SCRIPT_DIR"
fi

echo "Starting QEMU ARM64..."

# Build QEMU command
QEMU_CMD="qemu-system-aarch64 -M virt -cpu cortex-a72 -m 512 -kernel /breenix/kernel -display none -no-reboot -serial file:/output/serial.txt"

# Add ext2 disk volume (only one VirtIO block device to avoid driver bug)
DOCKER_VOLUMES="-v $KERNEL:/breenix/kernel:ro -v $OUTPUT_DIR:/output -v $EXT2_DISK:/breenix/ext2.img:ro"
QEMU_CMD="$QEMU_CMD -device virtio-blk-device,drive=ext2 -drive if=none,id=ext2,format=raw,readonly=on,file=/breenix/ext2.img"

# Run QEMU in background
# Docker ARM64 emulation is slower than native
# Allow up to 120 seconds for boot
docker run --rm $DOCKER_VOLUMES breenix-qemu-aarch64 timeout 120 $QEMU_CMD &
DOCKER_PID=$!

# Wait for kernel output (120 second timeout, polling every 2 seconds)
echo "Waiting for kernel boot (120s timeout)..."
BOOT_COMPLETE=false

for i in $(seq 1 60); do
    if [ -f "$OUTPUT_DIR/serial.txt" ]; then
        # Check for shell startup markers
        if grep -qE "(breenix>|Welcome to Breenix|Interactive Shell)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            BOOT_COMPLETE=true
            break
        fi
        # Also check for early crash
        if grep -qiE "(exception.*Data abort|KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            echo "ERROR: Early crash detected!"
            break
        fi
    fi
    sleep 2
done

# Give it a bit more time to settle
sleep 3

# Stop Docker
docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64) 2>/dev/null || true
wait $DOCKER_PID 2>/dev/null || true

echo ""
echo "========================================="
echo "Test Results"
echo "========================================="

# Verify results
PASSED=true
DETAILS=""

# 1. Check boot completion
if $BOOT_COMPLETE; then
    DETAILS+="[PASS] Shell startup detected\n"
else
    DETAILS+="[FAIL] Shell startup NOT detected\n"
    PASSED=false
fi

# 2. Check for multiple init_shell (regression test)
if [ -f "$OUTPUT_DIR/serial.txt" ]; then
    SHELL_COUNT=$(grep -o "init_shell" "$OUTPUT_DIR/serial.txt" 2>/dev/null | wc -l | tr -d ' ')
    SHELL_COUNT=${SHELL_COUNT:-0}
    # Expected: 3-4 mentions due to verbose logging (loading, create_process, thread add)
    if [ "$SHELL_COUNT" -le 5 ]; then
        DETAILS+="[PASS] init_shell mentions: $SHELL_COUNT (expected <=5)\n"
    else
        DETAILS+="[FAIL] Too many init_shell mentions: $SHELL_COUNT (regression!)\n"
        PASSED=false
    fi
else
    DETAILS+="[SKIP] No output file to check\n"
fi

# 3. Check for crashes
if [ -f "$OUTPUT_DIR/serial.txt" ]; then
    if grep -qiE "(\[exception\]|KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
        DETAILS+="[FAIL] Crash detected:\n"
        grep -iE "(\[exception\]|KERNEL PANIC|panic!)" "$OUTPUT_DIR/serial.txt" 2>/dev/null | head -3
        PASSED=false
    else
        DETAILS+="[PASS] No crashes detected\n"
    fi
else
    DETAILS+="[SKIP] No output file to check\n"
fi

echo -e "$DETAILS"

# Show last 20 lines of output for debugging
echo ""
echo "Last 20 lines of serial output:"
echo "----------------------------------------"
tail -20 "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no output)"
echo "----------------------------------------"

# Full output location
echo ""
echo "Full output: $OUTPUT_DIR/serial.txt"

if $PASSED; then
    echo ""
    echo "========================================="
    echo "ARM64 BOOT TEST: PASSED"
    echo "========================================="
    exit 0
else
    echo ""
    echo "========================================="
    echo "ARM64 BOOT TEST: FAILED"
    echo "========================================="
    exit 1
fi
