#!/bin/bash
# Run Breenix ARM64 kernel with graphics in QEMU
#
# Usage: ./scripts/run-arm64-graphics.sh [release|debug]
#
# This version enables VirtIO GPU for graphical output.
# Use Cocoa display on macOS, or SDL on Linux.

set -e

BUILD_TYPE="${1:-release}"

if [ "$BUILD_TYPE" = "debug" ]; then
    KERNEL="target/aarch64-breenix/debug/kernel-aarch64"
else
    KERNEL="target/aarch64-breenix/release/kernel-aarch64"
fi

# Check if kernel exists
if [ ! -f "$KERNEL" ]; then
    echo "Building ARM64 kernel ($BUILD_TYPE)..."
    if [ "$BUILD_TYPE" = "debug" ]; then
        cargo build --target aarch64-breenix.json -Z build-std=core,alloc -p kernel --bin kernel-aarch64
    else
        cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -p kernel --bin kernel-aarch64
    fi
fi

echo "Starting Breenix ARM64 kernel with graphics in QEMU..."
echo "Serial output goes to terminal, display shows graphics."
echo "Press Ctrl-A X to exit QEMU"
echo ""

# Determine display backend based on OS
case "$(uname)" in
    Darwin)
        DISPLAY_OPT="-display cocoa,show-cursor=on"
        ;;
    *)
        DISPLAY_OPT="-display sdl"
        ;;
esac

# Run QEMU with:
# - VirtIO GPU device (MMIO) for framebuffer
# - Serial output to terminal (mon:stdio)
# - VirtIO block device (empty, MMIO)
# - VirtIO net device (MMIO)
# - VirtIO keyboard device (MMIO) for keyboard input
# NOTE: Use -device virtio-*-device (MMIO) not virtio-*-pci
exec qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a72 \
    -m 512M \
    -serial mon:stdio \
    -device virtio-gpu-device \
    $DISPLAY_OPT \
    -device virtio-blk-device,drive=hd0 \
    -drive if=none,id=hd0,format=raw,file=/dev/null \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -device virtio-keyboard-device \
    -kernel "$KERNEL"
