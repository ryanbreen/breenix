#!/bin/bash
# Run kthread test in isolated Docker container
# Usage: ./run-kthread-test.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Build Docker image if needed
IMAGE_NAME="breenix-qemu"
if ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
    echo "Building Docker image..."
    docker build -t "$IMAGE_NAME" "$SCRIPT_DIR"
fi

# Ensure we have built artifacts
if [ ! -f "$BREENIX_ROOT/target/release/build/breenix-"*"/out/breenix-uefi.img" ]; then
    echo "Error: UEFI image not found. Run 'cargo build --release' first."
    exit 1
fi

# Find the UEFI image
UEFI_IMG=$(ls "$BREENIX_ROOT/target/release/build/breenix-"*"/out/breenix-uefi.img" 2>/dev/null | head -1)

# Create output directory for this run
OUTPUT_DIR=$(mktemp -d)
trap "rm -rf $OUTPUT_DIR" EXIT

# Create empty output files
touch "$OUTPUT_DIR/serial_kernel.txt"
touch "$OUTPUT_DIR/serial_user.txt"

echo "Running QEMU in Docker container..."
echo "  UEFI image: $UEFI_IMG"
echo "  Output dir: $OUTPUT_DIR"

# Copy OVMF files to writable location in output dir (pflash needs write access)
cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

# Run QEMU inside Docker with 30 second timeout
# Note: We use --rm to auto-cleanup the container
timeout 90 docker run --rm \
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

# Wait for kthread markers with timeout
echo "Waiting for kthread markers..."
TIMEOUT=60
ELAPSED=0
FOUND_LIFECYCLE=false
FOUND_JOIN=false

while [ $ELAPSED -lt $TIMEOUT ]; do
    sleep 1
    ELAPSED=$((ELAPSED + 1))

    if [ -f "$OUTPUT_DIR/serial_kernel.txt" ]; then
        # Check for lifecycle test markers
        if grep -q "KTHREAD_EXIT: kthread exited cleanly" "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
            FOUND_LIFECYCLE=true
        fi

        # Check for join test markers
        if grep -q "KTHREAD_JOIN_COMPLETE" "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
            FOUND_JOIN=true
        fi

        # Both tests passed
        if $FOUND_LIFECYCLE && $FOUND_JOIN; then
            echo "=== KTHREAD JOIN TEST: PASS ==="
            # Kill the container
            docker kill $(docker ps -q --filter ancestor="$IMAGE_NAME") 2>/dev/null || true
            exit 0
        fi
    fi
done

echo "=== KTHREAD JOIN TEST: TIMEOUT ==="
echo "Last 50 lines of output:"
tail -50 "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null || echo "(no output)"

# Kill any remaining container
docker kill $(docker ps -q --filter ancestor="$IMAGE_NAME") 2>/dev/null || true
exit 1
