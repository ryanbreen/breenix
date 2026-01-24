#!/bin/bash
# Run QEMU natively on macOS with Cocoa display for best performance
# Usage: ./run-interactive-native.sh
#
# This gives much better frame rates than VNC through Docker.
# Use for graphics demos like bounce and demo.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Find the UEFI image
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: UEFI image not found. Build with:"
    echo "  cargo build --release --features interactive --bin qemu-uefi"
    exit 1
fi

# Check for test_binaries.img
if [ ! -f "$BREENIX_ROOT/target/test_binaries.img" ]; then
    echo "Error: test_binaries.img not found. Create with:"
    echo "  cargo run -p xtask -- create-test-disk"
    exit 1
fi

# Check for ext2.img
if [ ! -f "$BREENIX_ROOT/target/ext2.img" ]; then
    echo "Warning: ext2.img not found, filesystem features may not work"
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
echo "Starting QEMU with native Cocoa display"
echo "========================================="
echo "Output: $OUTPUT_DIR"
echo "UEFI image: $UEFI_IMG"
echo ""
echo "A QEMU window will open with the Breenix display."
echo "Press Ctrl+C here or close the window to stop."
echo ""

# Verify OVMF files were copied
if [ ! -f "$OUTPUT_DIR/OVMF_CODE.fd" ]; then
    echo "Error: OVMF_CODE.fd not found"
    exit 1
fi

# Build the QEMU command
QEMU_CMD=(
    qemu-system-x86_64
    -drive "if=pflash,format=raw,readonly=on,file=$OUTPUT_DIR/OVMF_CODE.fd"
    -drive "if=pflash,format=raw,file=$OUTPUT_DIR/OVMF_VARS.fd"
    -drive "if=none,id=hd,format=raw,media=disk,readonly=on,file=$UEFI_IMG"
    -device "virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off"
    -drive "if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img"
    -device "virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off"
    -machine "pc"
    -accel "tcg,thread=multi,tb-size=512"
    -cpu qemu64
    -smp 2
    -m 512
    -device virtio-vga
    -display cocoa,show-cursor=on
    -k en-us
    -no-reboot
    -device "isa-debug-exit,iobase=0xf4,iosize=0x04"
    -netdev "user,id=net0"
    -device "e1000,netdev=net0,mac=52:54:00:12:34:56"
    -serial "file:$OUTPUT_DIR/serial_user.txt"
    -serial "file:$OUTPUT_DIR/serial_kernel.txt"
)

# Add ext2 disk if it exists
if [ -f "$BREENIX_ROOT/target/ext2.img" ]; then
    QEMU_CMD+=(
        -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img"
        -device "virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off"
    )
fi

# Print the command for debugging
echo "Running: ${QEMU_CMD[*]}"
echo ""

# Run QEMU
"${QEMU_CMD[@]}"

echo ""
echo "========================================="
echo "QEMU stopped"
echo "========================================="
echo ""
echo "Serial output saved to:"
echo "  User (COM1):   $OUTPUT_DIR/serial_user.txt"
echo "  Kernel (COM2): $OUTPUT_DIR/serial_kernel.txt"
echo ""
echo "=== Last 30 lines of kernel log ==="
tail -30 "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null || echo "(no output)"
echo ""
echo "=== Last 20 lines of user output ==="
tail -20 "$OUTPUT_DIR/serial_user.txt" 2>/dev/null || echo "(no output)"
