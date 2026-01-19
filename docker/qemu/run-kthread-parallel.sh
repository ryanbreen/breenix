#!/bin/bash
# Run N parallel Docker kthread tests
# Usage: ./run-kthread-parallel.sh [count]

set -e

COUNT=${1:-10}
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Find the kthread_test_only image (build with: cargo build --release --features kthread_test_only --bin qemu-uefi)
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: No UEFI image found. Build with:"
    echo "  cargo build --release --features kthread_test_only --bin qemu-uefi"
    exit 1
fi

echo "Running $COUNT parallel Docker kthread tests..."
echo "Image: $UEFI_IMG"

# Create output directories and launch containers
for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="/tmp/breenix_kthread_$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

    docker run --rm \
        -v "$UEFI_IMG:/breenix/breenix-uefi.img:ro" \
        -v "$OUTPUT_DIR:/output" \
        breenix-qemu \
        qemu-system-x86_64 \
            -pflash /output/OVMF_CODE.fd \
            -pflash /output/OVMF_VARS.fd \
            -drive if=none,id=hd,format=raw,readonly=on,file=/breenix/breenix-uefi.img \
            -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
            -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
            -display none -no-reboot -no-shutdown \
            -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
            -serial file:/output/serial_user.txt \
            -serial file:/output/serial_kernel.txt \
        &>/dev/null &
    echo "  Started test $i"
done

# Wait for all to complete (with timeout)
echo "Waiting for tests to complete (60s timeout)..."
PASSED=0
FAILED=0

for i in $(seq 1 $COUNT); do
    OUTPUT_DIR="/tmp/breenix_kthread_$i"

    # Wait up to 60 seconds for this test
    for j in $(seq 1 60); do
        if grep -q "KTHREAD_TEST_ONLY_COMPLETE" "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null; then
            echo "  Test $i: PASS"
            PASSED=$((PASSED + 1))
            break
        fi
        sleep 1
    done

    if [ $j -eq 60 ]; then
        echo "  Test $i: TIMEOUT"
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
