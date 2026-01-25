#!/bin/bash
# Run Breenix ARM64 kernel in QEMU
#
# Usage: ./scripts/run-arm64-qemu.sh [release|debug]

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

echo "Starting Breenix ARM64 kernel in QEMU..."
echo "Press Ctrl-A X to exit QEMU"
echo ""

exec qemu-system-aarch64 \
    -M virt \
    -cpu cortex-a72 \
    -m 512M \
    -nographic \
    -kernel "$KERNEL" \
    -drive if=none,id=none
