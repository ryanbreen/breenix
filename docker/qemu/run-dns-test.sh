#!/bin/bash
# Run DNS test in isolated Docker container
# Usage: ./run-dns-test.sh
#
# This script runs the dns_test_only kernel build in Docker,
# isolating QEMU from the host system to prevent crashes.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Build Docker image if needed
IMAGE_NAME="breenix-qemu"
if ! docker image inspect "$IMAGE_NAME" &>/dev/null; then
    echo "Building Docker image..."
    docker build -t "$IMAGE_NAME" "$SCRIPT_DIR"
fi

# Check for dns_test_only kernel build
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: UEFI image not found. Build with:"
    echo "  cargo build --release --features dns_test_only --bin qemu-uefi"
    exit 1
fi

# Create output directory
OUTPUT_DIR=$(mktemp -d)
trap "rm -rf $OUTPUT_DIR" EXIT

# Create empty output files
touch "$OUTPUT_DIR/serial_kernel.txt"
touch "$OUTPUT_DIR/serial_user.txt"

echo "Running DNS test in Docker container..."
echo "  UEFI image: $UEFI_IMG"
echo "  Output dir: $OUTPUT_DIR"
echo ""

# Copy OVMF files to writable location
cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

# Run QEMU inside Docker
# - Uses SLIRP networking (user mode) which provides DNS at 10.0.2.3
# - TCG acceleration (software emulation) - slower but isolated
timeout 120 docker run --rm \
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
        -display none \
        -boot strict=on \
        -no-reboot \
        -no-shutdown \
        -monitor none \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -netdev user,id=net0 \
        -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
        -serial file:/output/serial_user.txt \
        -serial file:/output/serial_kernel.txt \
    &

QEMU_PID=$!

# Wait for DNS test markers
echo "Waiting for DNS test completion..."
TIMEOUT=90
ELAPSED=0

# DNS test stages to check
STAGES_PASSED=0
TOTAL_STAGES=6

while [ $ELAPSED -lt $TIMEOUT ]; do
    sleep 1
    ELAPSED=$((ELAPSED + 1))

    if [ -f "$OUTPUT_DIR/serial_user.txt" ]; then
        USER_OUTPUT=$(cat "$OUTPUT_DIR/serial_user.txt" 2>/dev/null)

        # Check each stage
        CURRENT_STAGES=0

        if echo "$USER_OUTPUT" | grep -q "DNS Test: Starting"; then
            CURRENT_STAGES=$((CURRENT_STAGES + 1))
        fi

        if echo "$USER_OUTPUT" | grep -q "DNS_TEST: google_resolve OK"; then
            CURRENT_STAGES=$((CURRENT_STAGES + 1))
        fi

        if echo "$USER_OUTPUT" | grep -q "DNS_TEST: example_resolve OK"; then
            CURRENT_STAGES=$((CURRENT_STAGES + 1))
        fi

        if echo "$USER_OUTPUT" | grep -q "DNS_TEST: nxdomain OK"; then
            CURRENT_STAGES=$((CURRENT_STAGES + 1))
        fi

        if echo "$USER_OUTPUT" | grep -q "DNS_TEST: empty_hostname OK"; then
            CURRENT_STAGES=$((CURRENT_STAGES + 1))
        fi

        if echo "$USER_OUTPUT" | grep -q "DNS Test: All tests passed"; then
            CURRENT_STAGES=$((CURRENT_STAGES + 1))
        fi

        # Report progress if changed
        if [ $CURRENT_STAGES -gt $STAGES_PASSED ]; then
            STAGES_PASSED=$CURRENT_STAGES
            echo "  Progress: $STAGES_PASSED/$TOTAL_STAGES stages passed"
        fi

        # All tests passed
        if [ $STAGES_PASSED -eq $TOTAL_STAGES ]; then
            echo ""
            echo "========================================="
            echo "DNS TEST: ALL $TOTAL_STAGES STAGES PASSED"
            echo "========================================="
            docker kill $(docker ps -q --filter ancestor="$IMAGE_NAME") 2>/dev/null || true
            exit 0
        fi
    fi
done

echo ""
echo "========================================="
echo "DNS TEST: TIMEOUT ($STAGES_PASSED/$TOTAL_STAGES passed)"
echo "========================================="
echo ""
echo "User output (COM1):"
cat "$OUTPUT_DIR/serial_user.txt" 2>/dev/null | grep -E "(DNS|resolve)" || echo "(no DNS output)"
echo ""
echo "Last 20 lines of kernel output (COM2):"
tail -20 "$OUTPUT_DIR/serial_kernel.txt" 2>/dev/null || echo "(no output)"

# Kill any remaining container
docker kill $(docker ps -q --filter ancestor="$IMAGE_NAME") 2>/dev/null || true
exit 1
