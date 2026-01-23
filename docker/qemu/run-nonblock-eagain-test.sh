#!/bin/bash
# Run nonblock EAGAIN test in isolated Docker container
# Usage: ./run-nonblock-eagain-test.sh
#
# This script runs the nonblock_eagain_test kernel build in Docker.
# The test verifies that a nonblocking socket returns EAGAIN immediately
# when no data is available (no external packet needed).

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Build Docker image if needed
IMAGE_NAME="breenix-qemu"
if ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
    echo "Building Docker image..."
    docker build -t "$IMAGE_NAME" "$SCRIPT_DIR"
fi

echo "Building nonblock_eagain_test kernel..."
cargo build --release --features nonblock_eagain_test --bin qemu-uefi

# Check for nonblock_eagain_test kernel build
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: UEFI image not found."
    exit 1
fi

if [ ! -f "$BREENIX_ROOT/target/test_binaries.img" ]; then
    echo "Error: test_binaries.img not found. Build the test disk first."
    exit 1
fi

# Create output directory
OUTPUT_DIR=$(mktemp -d)
trap "rm -rf $OUTPUT_DIR" EXIT

# Create empty output files
touch "$OUTPUT_DIR/serial_kernel.txt"
touch "$OUTPUT_DIR/serial_user.txt"

echo "Running nonblock EAGAIN test in Docker container..."
echo "  UEFI image: $UEFI_IMG"
echo "  Output dir: $OUTPUT_DIR"
echo ""

# Copy OVMF files to writable location
cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

# Run QEMU inside Docker
# No external packet needed - test verifies EAGAIN return immediately
timeout 60 docker run --rm \
    -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
    -v "$BREENIX_ROOT/target/test_binaries.img:/breenix/test_binaries.img:ro" \
    -v "$BREENIX_ROOT/target/ext2.img:/breenix/ext2.img:ro" \
    -v "$OUTPUT_DIR:/output" \
    "$IMAGE_NAME" \
    qemu-system-x86_64 \
        -pflash /output/OVMF_CODE.fd \
        -pflash /output/OVMF_VARS.fd \
        -drive if=none,id=hd,format=raw,media=disk,readonly=on,file=/breenix/breenix-uefi.img \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive if=none,id=testdisk,format=raw,readonly=on,file=/breenix/test_binaries.img \
        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
        -drive if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img \
        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
        -machine pc,accel=tcg \
        -cpu qemu64 \
        -smp 1 \
        -m 512 \
        -display none \
        -boot strict=on \
        -no-reboot \
        -no-shutdown \
        -monitor none \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -netdev user,id=net0 \
        -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
        -serial file:/output/serial_user.txt \
        -serial file:/output/serial_kernel.txt \
    &

QEMU_PID=$!

echo "Waiting for nonblock EAGAIN test..."
TIMEOUT=45
ELAPSED=0

while [ $ELAPSED -lt $TIMEOUT ]; do
    sleep 1
    ELAPSED=$((ELAPSED + 1))

    if [ -f "$OUTPUT_DIR/serial_user.txt" ]; then
        USER_OUTPUT=$(cat "$OUTPUT_DIR/serial_user.txt" 2>/dev/null)

        if echo "$USER_OUTPUT" | grep -q "NONBLOCK_EAGAIN_TEST: PASS"; then
            echo ""
            echo "========================================="
            echo "NONBLOCK EAGAIN TEST: PASS"
            echo "========================================="
            docker kill $(docker ps -q --filter ancestor="$IMAGE_NAME") 2>/dev/null || true
            exit 0
        fi

        if echo "$USER_OUTPUT" | grep -q "NONBLOCK_EAGAIN_TEST:.*errno="; then
            echo ""
            echo "========================================="
            echo "NONBLOCK EAGAIN TEST: FAIL (wrong errno)"
            echo "========================================="
            echo ""
            echo "User output (COM1):"
            cat "$OUTPUT_DIR/serial_user.txt" 2>/dev/null | grep -E "NONBLOCK_EAGAIN_TEST" || echo "(no output)"
            docker kill $(docker ps -q --filter ancestor="$IMAGE_NAME") 2>/dev/null || true
            exit 1
        fi
    fi
done

echo ""
echo "========================================="
echo "NONBLOCK EAGAIN TEST: TIMEOUT"
echo "========================================="
echo ""
echo "User output (COM1):"
cat "$OUTPUT_DIR/serial_user.txt" 2>/dev/null | grep -E "NONBLOCK_EAGAIN_TEST" || echo "(no nonblock eagain output)"
echo ""
echo "Last 20 lines of kernel output (COM2):"
tail -20 "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null || echo "(no output)"

docker kill $(docker ps -q --filter ancestor="$IMAGE_NAME") 2>/dev/null || true
exit 1
