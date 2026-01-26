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
KERNEL="$BREENIX_ROOT/target/aarch64-unknown-none/release/kernel"
if [ ! -f "$KERNEL" ]; then
    echo "Building ARM64 kernel..."
    cd "$BREENIX_ROOT/kernel"
    cargo build --release --target aarch64-unknown-none
    cd "$BREENIX_ROOT"
fi

if [ ! -f "$KERNEL" ]; then
    echo "Error: ARM64 kernel not found at $KERNEL"
    echo "Try building with:"
    echo "  cd kernel && cargo build --release --target aarch64-unknown-none"
    exit 1
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
docker run --rm \
    -p 5901:5900 \
    -v "$KERNEL:/breenix/kernel:ro" \
    -v "$OUTPUT_DIR:/output" \
    "$IMAGE_NAME" \
    qemu-system-aarch64 \
        -M virt \
        -cpu cortex-a72 \
        -m 512M \
        -kernel /breenix/kernel \
        -device virtio-gpu-device \
        -vnc :0 \
        -device virtio-keyboard-device \
        -device virtio-blk-device,drive=hd0 \
        -drive if=none,id=hd0,format=raw,file=/dev/null \
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
