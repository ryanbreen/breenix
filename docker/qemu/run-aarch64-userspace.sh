#!/bin/bash
# Run ARM64 kernel with userspace binaries in Docker
# Usage: ./run-aarch64-userspace.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-breenix/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-breenix.json -p kernel --features aarch64-qemu --bin kernel-aarch64"
    exit 1
fi

# Find or create ARM64 ext2 disk
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
EXT2_SIZE_BYTES=$((8 * 1024 * 1024))
EXT2_SIZE_ACTUAL=0
if [ -f "$EXT2_DISK" ]; then
    if stat -f%z "$EXT2_DISK" >/dev/null 2>&1; then
        EXT2_SIZE_ACTUAL=$(stat -f%z "$EXT2_DISK")
    else
        EXT2_SIZE_ACTUAL=$(stat -c %s "$EXT2_DISK")
    fi
fi

if [ ! -f "$EXT2_DISK" ] || [ "$EXT2_SIZE_ACTUAL" -ne "$EXT2_SIZE_BYTES" ]; then
    if [ -f "$EXT2_DISK" ]; then
        echo "Recreating ARM64 ext2 disk (size mismatch: $EXT2_SIZE_ACTUAL bytes)"
        rm -f "$EXT2_DISK"
    else
        echo "Creating ARM64 ext2 disk image..."
    fi

    "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64 --size 8

    if [ ! -f "$EXT2_DISK" ]; then
        echo "Error: Failed to create ext2 disk image at $EXT2_DISK"
        exit 1
    fi
fi

echo "Running ARM64 kernel with userspace..."
echo "Kernel: $KERNEL"
echo "Ext2 disk: $EXT2_DISK"

# Create output directory
OUTPUT_DIR="/tmp/breenix_aarch64_1"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Build the ARM64 Docker image if not exists
if ! docker images breenix-qemu-aarch64 --format "{{.Repository}}" | grep -q breenix-qemu-aarch64; then
    echo "Building ARM64 Docker image..."
    docker build -t breenix-qemu-aarch64 -f "$SCRIPT_DIR/Dockerfile.aarch64" "$SCRIPT_DIR"
fi

echo "Starting QEMU ARM64 with VirtIO devices..."

# Create writable copy of ext2 disk to allow filesystem write tests
EXT2_WRITABLE="$OUTPUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$EXT2_WRITABLE"

# Run QEMU with ARM64 virt machine and VirtIO devices
# QEMU virt machine provides 32 VirtIO MMIO slots at:
#   0x0a000000 + n*0x200  for n=0..31
# Devices are assigned from slot 31 downward.
# Use writable disk copy (no readonly=on) to allow filesystem writes
docker run --rm \
    -v "$KERNEL:/breenix/kernel:ro" \
    -v "$EXT2_WRITABLE:/breenix/ext2.img" \
    -v "$OUTPUT_DIR:/output" \
    breenix-qemu-aarch64 \
    qemu-system-aarch64 \
        -M virt \
        -cpu cortex-a72 \
        -m 512 \
        -kernel /breenix/kernel \
        -drive if=none,id=ext2disk,format=raw,file=/breenix/ext2.img \
        -device virtio-blk-device,drive=ext2disk \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -display none \
        -no-reboot \
        -serial file:/output/serial.txt \
        &

QEMU_PID=$!

# Wait for output (60 second timeout)
echo "Waiting for kernel output (60s timeout)..."
FOUND=false
for i in $(seq 1 60); do
    if [ -f "$OUTPUT_DIR/serial.txt" ] && [ -s "$OUTPUT_DIR/serial.txt" ]; then
        # Check for boot complete or userspace output
        if grep -qE "(Boot Complete|Hello|userspace|fork)" "$OUTPUT_DIR/serial.txt" 2>/dev/null; then
            FOUND=true
            break
        fi
    fi
    sleep 1
done

# Wait a bit more for any additional output
sleep 2

# Show output
echo ""
echo "========================================="
echo "Serial Output:"
echo "========================================="
if [ -f "$OUTPUT_DIR/serial.txt" ]; then
    cat "$OUTPUT_DIR/serial.txt"
else
    echo "(no output)"
fi
echo "========================================="

# Cleanup
docker kill $(docker ps -q --filter ancestor=breenix-qemu-aarch64) 2>/dev/null || true

if $FOUND; then
    echo "ARM64 kernel produced output!"
    exit 0
else
    echo "Timeout or no meaningful output"
    exit 1
fi
