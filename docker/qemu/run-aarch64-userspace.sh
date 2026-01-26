#!/bin/bash
# Run ARM64 kernel with userspace binaries in Docker
# Usage: ./run-aarch64-userspace.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the ARM64 kernel
KERNEL="$BREENIX_ROOT/target/aarch64-unknown-none/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found. Build with:"
    echo "  cargo build --release --target aarch64-unknown-none -p kernel --features aarch64-qemu --bin kernel-aarch64"
    exit 1
fi

# Find or create ARM64 test disk
TEST_DISK="$BREENIX_ROOT/target/aarch64_test_binaries.img"
if [ ! -f "$TEST_DISK" ]; then
    echo "Creating ARM64 test disk image..."

    # Create a disk image with userspace binaries
    # Using a simple raw format - kernel will need to parse this
    TEMP_DIR=$(mktemp -d)

    # Copy ARM64 binaries to temp dir
    if [ -d "$BREENIX_ROOT/userspace/tests/aarch64" ]; then
        cp "$BREENIX_ROOT/userspace/tests/aarch64/"*.elf "$TEMP_DIR/" 2>/dev/null || true
    fi

    # Create a simple FAT disk image
    # 4MB should be plenty for test binaries
    dd if=/dev/zero of="$TEST_DISK" bs=1M count=4

    # Format as FAT16
    if command -v mkfs.fat &>/dev/null; then
        mkfs.fat -F 16 "$TEST_DISK"
        # Mount and copy files
        MOUNT_DIR=$(mktemp -d)
        if mount -o loop "$TEST_DISK" "$MOUNT_DIR" 2>/dev/null; then
            cp "$TEMP_DIR"/*.elf "$MOUNT_DIR/" 2>/dev/null || true
            umount "$MOUNT_DIR"
        else
            echo "Note: Could not mount disk image to copy files"
            echo "      (This is expected on macOS - using mtools instead)"
        fi
        rmdir "$MOUNT_DIR"
    fi

    # On macOS, use mtools if available
    if command -v mtools &>/dev/null || [ -f /opt/homebrew/bin/mformat ]; then
        # Try to use mtools
        mformat -i "$TEST_DISK" -F :: 2>/dev/null || true
        for f in "$TEMP_DIR"/*.elf; do
            [ -f "$f" ] && mcopy -i "$TEST_DISK" "$f" :: 2>/dev/null || true
        done
    fi

    rm -rf "$TEMP_DIR"

    echo "Created: $TEST_DISK"
fi

echo "Running ARM64 kernel with userspace..."
echo "Kernel: $KERNEL"
echo "Test disk: $TEST_DISK"

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

# Run QEMU with ARM64 virt machine and VirtIO devices
# QEMU virt machine VirtIO MMIO addresses:
#   0x0a000000 - 0x0a003fff: virtio@a000000 (device 0)
#   0x0a004000 - 0x0a007fff: virtio@a004000 (device 1)
#   etc.
docker run --rm \
    -v "$KERNEL:/breenix/kernel:ro" \
    -v "$TEST_DISK:/breenix/test_disk.img:ro" \
    -v "$OUTPUT_DIR:/output" \
    breenix-qemu-aarch64 \
    qemu-system-aarch64 \
        -M virt \
        -cpu cortex-a72 \
        -m 512 \
        -kernel /breenix/kernel \
        -drive if=none,id=hd0,format=raw,readonly=on,file=/breenix/test_disk.img \
        -device virtio-blk-device,drive=hd0 \
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
