#!/bin/bash
#
# Test Tracing Framework via GDB Memory Dump
#
# This script:
# 1. Starts QEMU with the kernel (GDB enabled)
# 2. Connects GDB and sets a breakpoint after tracing initialization
# 3. Continues to the breakpoint
# 4. Dumps the TRACE_BUFFERS memory region
# 5. Parses and validates the trace data
#

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Find kernel binary to get symbol offsets
KERNEL_BIN=$(find "$BREENIX_ROOT/target" -path "*/x86_64-unknown-none/release/deps/artifact/*/bin/kernel-*" -type f ! -name "*aarch64*" ! -name "*.d" 2>/dev/null | head -1)

if [ -z "$KERNEL_BIN" ]; then
    echo "Error: Kernel binary not found. Build with:"
    echo "  cargo build --release --features testing,external_test_bins --bin qemu-uefi"
    exit 1
fi

echo "Kernel binary: $KERNEL_BIN"

# Get symbol offsets
TRACE_BUFFERS_OFFSET=$(nm "$KERNEL_BIN" | grep " B TRACE_BUFFERS$" | awk '{print $1}')
TRACE_ENABLED_OFFSET=$(nm "$KERNEL_BIN" | grep " B TRACE_ENABLED$" | awk '{print $1}')
TRACE_CPU0_IDX_OFFSET=$(nm "$KERNEL_BIN" | grep " B TRACE_CPU0_WRITE_IDX$" | awk '{print $1}')

if [ -z "$TRACE_BUFFERS_OFFSET" ]; then
    echo "Error: TRACE_BUFFERS symbol not found"
    exit 1
fi

# Calculate runtime addresses (kernel base = 0x10000000000)
KERNEL_BASE=0x10000000000
TRACE_BUFFERS_ADDR=$(printf "0x%x" $((KERNEL_BASE + 0x$TRACE_BUFFERS_OFFSET)))
TRACE_ENABLED_ADDR=$(printf "0x%x" $((KERNEL_BASE + 0x$TRACE_ENABLED_OFFSET)))
TRACE_CPU0_IDX_ADDR=$(printf "0x%x" $((KERNEL_BASE + 0x$TRACE_CPU0_IDX_OFFSET)))

echo ""
echo "Symbol Addresses:"
echo "  TRACE_BUFFERS:      $TRACE_BUFFERS_ADDR (offset: 0x$TRACE_BUFFERS_OFFSET)"
echo "  TRACE_ENABLED:      $TRACE_ENABLED_ADDR (offset: 0x$TRACE_ENABLED_OFFSET)"
echo "  TRACE_CPU0_WRITE_IDX: $TRACE_CPU0_IDX_ADDR (offset: 0x$TRACE_CPU0_IDX_OFFSET)"
echo ""

# Buffer size calculation
# TraceCpuBuffer: 1024 events * 16 bytes = 16384 + ~48 bytes metadata, aligned to 64
EVENTS_SIZE=$((1024 * 16))
BUFFER_SIZE=$(( ((EVENTS_SIZE + 64 + 63) / 64) * 64 ))  # ~16448, aligned
TOTAL_SIZE=$((BUFFER_SIZE * 8))  # 8 CPUs

echo "Buffer sizes:"
echo "  Per-CPU buffer: $BUFFER_SIZE bytes"
echo "  Total (8 CPUs): $TOTAL_SIZE bytes"
echo ""

# Create output directory
OUTPUT_DIR="/tmp/breenix_trace_test"
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Create GDB commands file
cat > "$OUTPUT_DIR/gdb_commands.txt" << EOF
set pagination off
set confirm off

# Connect to QEMU GDB server
target remote localhost:1234

# Wait for kernel to initialize tracing
# Set breakpoint after tracing init (look for the checkpoint log)
break *($KERNEL_BASE + 0x18b090)

# Continue to let kernel boot
continue

# Wait a moment for tracing to record some events
shell sleep 2

# Check if tracing is enabled
print/x *(unsigned long long*)$TRACE_ENABLED_ADDR

# Check write index
print/x *(unsigned long long*)$TRACE_CPU0_IDX_ADDR

# Dump trace buffers to file
dump binary memory $OUTPUT_DIR/trace_buffers.bin $TRACE_BUFFERS_ADDR ($TRACE_BUFFERS_ADDR + $TOTAL_SIZE)

# Dump enabled flag
dump binary memory $OUTPUT_DIR/trace_enabled.bin $TRACE_ENABLED_ADDR ($TRACE_ENABLED_ADDR + 8)

echo \n=== Trace Memory Dump Complete ===\n

quit
EOF

echo "Starting QEMU with GDB..."

# Start QEMU with GDB server
UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
if [ -z "$UEFI_IMG" ]; then
    echo "Error: UEFI image not found"
    exit 1
fi

# Copy OVMF files
cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"

# Start QEMU in background with GDB enabled
qemu-system-x86_64 \
    -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
    -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
    -drive if=none,id=hd,format=raw,readonly=on,file="$UEFI_IMG" \
    -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
    -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
    -display none -no-reboot -no-shutdown \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -serial file:"$OUTPUT_DIR/serial.txt" \
    -gdb tcp::1234 -S \
    &>/dev/null &
QEMU_PID=$!

echo "QEMU started (PID: $QEMU_PID)"
sleep 2

# Run GDB
echo "Running GDB commands..."
timeout 60 gdb -batch -x "$OUTPUT_DIR/gdb_commands.txt" "$KERNEL_BIN" 2>&1 || true

# Clean up
kill $QEMU_PID 2>/dev/null || true

# Check results
echo ""
echo "=== Results ==="
echo ""

if [ -f "$OUTPUT_DIR/trace_buffers.bin" ]; then
    SIZE=$(stat -f%z "$OUTPUT_DIR/trace_buffers.bin" 2>/dev/null || stat -c%s "$OUTPUT_DIR/trace_buffers.bin" 2>/dev/null)
    echo "Trace buffer dump: $SIZE bytes"

    # Parse with Python script
    if [ -f "$BREENIX_ROOT/scripts/trace_memory_dump.py" ]; then
        echo ""
        echo "Parsing trace data..."
        python3 "$BREENIX_ROOT/scripts/trace_memory_dump.py" --parse "$OUTPUT_DIR/trace_buffers.bin" --validate
    fi
else
    echo "Error: Trace buffer dump not created"
    echo ""
    echo "GDB output:"
    cat "$OUTPUT_DIR/gdb_commands.txt" 2>/dev/null || true
    exit 1
fi

echo ""
echo "Serial output:"
head -50 "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no serial output)"

echo ""
echo "Done. Files saved to: $OUTPUT_DIR"
