#!/bin/bash
# Run QEMU interactively in Docker with VNC display
# Usage: ./run-interactive.sh
#
# Automatically opens TigerVNC connected to the QEMU display.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Build Docker image if needed
IMAGE_NAME="breenix-qemu"
if ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
    echo "Building Docker image..."
    docker build -t "$IMAGE_NAME" "$SCRIPT_DIR"
fi

# Find the UEFI image
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: UEFI image not found. Build with:"
    echo "  cargo build --release --features interactive --bin qemu-uefi"
    exit 1
fi

# Create output directory
OUTPUT_DIR=$(mktemp -d)

# Copy OVMF files
cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

# Create empty serial output files
touch "$OUTPUT_DIR/serial_user.txt"
touch "$OUTPUT_DIR/serial_kernel.txt"

echo ""
echo "========================================="
echo "Starting QEMU with VNC display"
echo "========================================="
echo "Output: $OUTPUT_DIR"
echo ""
echo "Press Ctrl+C to stop"
echo ""

# Run QEMU with VNC in background
docker run --rm \
    -p 5900:5900 \
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
        -device virtio-vga \
        -vnc :0 \
        -k en-us \
        -no-reboot \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -netdev user,id=net0 \
        -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
        -serial file:/output/serial_user.txt \
        -serial file:/output/serial_kernel.txt \
    &

DOCKER_PID=$!

# Wait for VNC to be ready
echo "Waiting for VNC server..."
sleep 3

# Auto-open TigerVNC
echo "Opening TigerVNC..."
open "/Applications/TigerVNC Viewer 1.15.0.app" --args localhost:5900

# Wait for docker to finish
wait $DOCKER_PID 2>/dev/null

echo ""
echo "========================================="
echo "QEMU stopped"
echo "========================================="
echo ""
echo "Serial output saved to:"
echo "  User (COM1):   $OUTPUT_DIR/serial_user.txt"
echo "  Kernel (COM2): $OUTPUT_DIR/serial_kernel.txt"
