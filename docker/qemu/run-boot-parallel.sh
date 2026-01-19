#!/bin/bash
# Run N parallel Docker full boot tests
# Usage: ./run-boot-parallel.sh [count]

set -e

COUNT=${1:-5}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the full boot image
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: No UEFI image found. Build with:"
    echo "  cargo build --release --features testing,external_test_bins --bin qemu-uefi"
    exit 1
fi

echo "Running $COUNT parallel Docker full boot tests..."
echo "Image: $UEFI_IMG"

# Create output directories and launch containers
for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="/tmp/breenix_boot_$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

    docker run --rm \
        -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
        -v "$BREENIX_ROOT/target/test_binaries.img:/breenix/test_binaries.img:ro" \
        -v "$BREENIX_ROOT/target/ext2.img:/breenix/ext2.img:ro" \
        -v "$OUTPUT_DIR:/output" \
        breenix-qemu \
        qemu-system-x86_64 \
            -pflash /output/OVMF_CODE.fd \
            -pflash /output/OVMF_VARS.fd \
            -drive if=none,id=hd,format=raw,readonly=on,file=/breenix/breenix-uefi.img \
            -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
            -drive if=none,id=testdisk,format=raw,readonly=on,file=/breenix/test_binaries.img \
            -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
            -drive if=none,id=ext2disk,format=raw,readonly=on,file=/breenix/ext2.img \
            -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
            -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
            -display none -no-reboot -no-shutdown \
            -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
            -serial file:/output/serial_user.txt \
            -serial file:/output/serial_kernel.txt \
        &>/dev/null &
    echo "  Started test $i"
done

# Wait for all to complete - look for kthread markers (they run early in boot)
echo "Waiting for kthread tests to complete (120s timeout)..."
PASSED=0
FAILED=0

for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="/tmp/breenix_boot_$i"
    FOUND=false

    # Wait up to 120 seconds for kthread markers
    for j in $(seq 1 120); do
        if grep -q "KTHREAD JOIN TEST: Completed" "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
            FOUND=true
            break
        fi
        sleep 1
    done

    if $FOUND; then
        # Check if kthread tests actually passed
        if grep -q "KTHREAD_EXIT: kthread exited cleanly" "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
            echo "  Test $i: PASS"
            PASSED=$((PASSED + 1))
        else
            echo "  Test $i: FAIL (kthread didn't exit cleanly)"
            FAILED=$((FAILED + 1))
        fi
    else
        echo "  Test $i: TIMEOUT"
        tail -10 "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null || echo "    (no output)"
        FAILED=$((FAILED + 1))
    fi
done

# Cleanup
docker kill $(docker ps -q --filter ancestor=breenix-qemu) 2>/dev/null || true

echo ""
echo "========================================="
echo "Results: $PASSED passed, $FAILED failed out of $COUNT"
echo "========================================="

if [ $FAILED -eq 0 ]; then
    exit 0
else
    exit 1
fi
