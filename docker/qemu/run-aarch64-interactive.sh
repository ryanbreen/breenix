#!/bin/bash
# Run ARM64 kernel interactively in Docker with VNC display
# Usage: ./run-aarch64-interactive.sh
#
# Opens a VNC window where you can interact with the ARM64 shell.
# Use TigerVNC or any VNC client to connect to localhost:5901

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Build the kernel if needed
KERNEL="$BREENIX_ROOT/target/aarch64-unknown-none/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Building ARM64 kernel..."
    cargo build --release --target aarch64-unknown-none -p kernel --bin kernel-aarch64
fi

if [ ! -f "$KERNEL" ]; then
    echo "Error: ARM64 kernel not found at $KERNEL"
    echo "Try building with:"
    echo "  cargo build --release --target aarch64-unknown-none -p kernel --bin kernel-aarch64"
    exit 1
fi

# Check for ext2 disk image
EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ -f "$EXT2_DISK" ]; then
    echo "Found ext2 disk: $EXT2_DISK"
fi

# Build Docker image if needed
IMAGE_NAME="breenix-qemu-aarch64"
if ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
    echo "Building ARM64 Docker image..."
    docker build -t "$IMAGE_NAME" -f "$SCRIPT_DIR/Dockerfile.aarch64" "$SCRIPT_DIR"
fi

# Kill any existing containers (prevents port conflicts)
EXISTING=$(docker ps -q --filter ancestor="$IMAGE_NAME" 2>/dev/null)
if [ -n "$EXISTING" ]; then
    echo "Stopping existing ARM64 containers..."
    docker kill $EXISTING 2>/dev/null || true
fi

# Create output directory
OUTPUT_DIR=$(mktemp -d)

echo ""
echo "========================================="
echo "Breenix ARM64 Interactive Mode"
echo "========================================="
echo ""
echo "Kernel: $KERNEL"
echo "Output: $OUTPUT_DIR"
echo ""
echo "Connect to VNC at localhost:5901"
echo "Press Ctrl+C to stop"
echo ""

# Run QEMU with VNC display in Docker
# Port 5901 to avoid conflict with x86_64 on 5900

# Build disk options
# Create writable copy for the container to use
DISK_VOLUME=""
DISK_OPTS="-device virtio-blk-device,drive=hd0 -drive if=none,id=hd0,format=raw,file=/dev/null"
if [ -f "$EXT2_DISK" ]; then
    # Create a writable copy in /tmp for container use
    EXT2_WRITABLE="/tmp/breenix_aarch64_interactive_ext2.img"
    cp "$EXT2_DISK" "$EXT2_WRITABLE"
    DISK_VOLUME="-v $EXT2_WRITABLE:/breenix/ext2.img"
    DISK_OPTS="-device virtio-blk-device,drive=ext2disk -drive if=none,id=ext2disk,format=raw,file=/breenix/ext2.img"
fi

docker run --rm \
    -p 5901:5900 \
    -v "$KERNEL:/breenix/kernel:ro" \
    -v "$OUTPUT_DIR:/output" \
    $DISK_VOLUME \
    "$IMAGE_NAME" \
    qemu-system-aarch64 \
        -M virt \
        -cpu cortex-a72 \
        -m 512M \
        -kernel /breenix/kernel \
        -device virtio-gpu-device \
        -vnc :0 \
        -device virtio-keyboard-device \
        $DISK_OPTS \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:/output/serial.txt \
        -no-reboot \
    &

DOCKER_PID=$!

# Wait for VNC to be ready
echo "Waiting for QEMU to start..."
sleep 3

# Try to auto-open TigerVNC on macOS
if [ "$(uname)" = "Darwin" ]; then
    if [ -d "/Applications/TigerVNC Viewer 1.15.0.app" ]; then
        echo "Opening TigerVNC..."
        open "/Applications/TigerVNC Viewer 1.15.0.app" --args localhost:5901
    else
        echo ""
        echo "TigerVNC not found. Connect manually to localhost:5901"
        echo "Install from: https://github.com/TigerVNC/tigervnc/releases"
    fi
else
    echo ""
    echo "Connect your VNC client to localhost:5901"
fi

echo ""
echo "Serial output is being logged to: $OUTPUT_DIR/serial.txt"
echo "Use 'tail -f $OUTPUT_DIR/serial.txt' in another terminal to watch"
echo ""

# Wait for docker to finish
wait $DOCKER_PID 2>/dev/null

echo ""
echo "========================================="
echo "QEMU stopped"
echo "========================================="
echo ""
echo "Serial output saved to: $OUTPUT_DIR/serial.txt"
