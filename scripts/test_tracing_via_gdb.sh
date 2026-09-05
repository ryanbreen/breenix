#!/bin/bash
#
# Tracing-framework evidence harness (arch-portable: x86_64 and aarch64).
#
# Boots a Breenix kernel under QEMU with the gdbstub enabled, lets it run long
# enough to record real trace events, attaches GDB (which halts the guest),
# dumps the whole TRACE_BUFFERS region plus TRACE_ENABLED, and hands the dump to
# scripts/trace_memory_dump.py --validate. The parser's verdict is this script's
# exit status: a dump that parses to zero events is a FAILURE, not a pass.
#
# Three properties this harness deliberately does NOT have:
#   * no hardcoded breakpoint address. The previous version broke at a literal
#     `KERNEL_BASE + 0x18b090`, which silently stopped meaning anything the
#     moment the kernel was rebuilt. Instead the guest free-runs for a settle
#     window and GDB's attach is what halts it.
#   * no hardcoded CPU count. MAX_CPUS and TRACE_BUFFER_SIZE are read out of the
#     kernel sources they are defined in, so a change there cannot leave this
#     harness dumping (and validating) half the buffers.
#   * no hardcoded x86_64 kernel load base. The bootloader crate places the
#     kernel PIE at a runtime-chosen free virtual address slot and prints the
#     choice exactly once, as "virtual_address_offset: 0x..." on serial, early
#     in boot. The same binary has been observed to land at 0x8000000000 on
#     some boots and 0x10000000000 on others -- a base baked into this script
#     would silently make the symbol addresses derived below wrong on
#     whichever boots picked the other slot. The base is
#     instead read off each boot's own serial capture after the settle
#     window, and the script fails loudly -- not silently -- if that line
#     is missing. breenix-gdb-chat/scripts/gdb_chat.py's
#     resync_symbols() (landed for #739) and the #702 RCA loop harness both
#     derive the same value the same way; this port does not reinvent it.
#
# Usage:
#   scripts/test_tracing_via_gdb.sh [--arch aarch64|x86_64] [--settle SECONDS]
#                                   [--port PORT] [--out DIR]
#
# Default --arch is derived from the host (`uname -m`); pass it explicitly in a
# gate. Both arches are runnable on an ARM Mac (QEMU TCG for x86_64) and on the
# beast x86 VM.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# #826/#834/R181: sourced unconditionally since this harness's --arch is
# runtime-selected, but only the aarch64 branch below actually calls
# qemu_host_lock_acquire -- the host-wide lock in
# docker/qemu/lib/qemu-host-lock.sh serializes qemu-system-aarch64 boots
# specifically, and this script's x86_64 leg (qemu-system-x86_64) is outside
# that lock's scope, same as run.sh's x86_64 leg. #834 extends this lock's
# coverage from docker/qemu/*.sh (its original #826/R181 scope) to scripts/
# as well.
# shellcheck source=../docker/qemu/lib/qemu-host-lock.sh
source "$BREENIX_ROOT/docker/qemu/lib/qemu-host-lock.sh"

# #825: without --out, two concurrent invocations of this harness for the
# same --arch hardcoded the identical /tmp/breenix_trace_test_$ARCH path, so
# one invocation's rm -rf/mkdir could delete and rewrite another's in-flight
# capture. --out remains the per-invocation override; BREENIX_GATE_TMP is
# now the base its default is built under (default /tmp, so an unset caller
# is byte-identical to before).
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP" >&2; exit 2 ;;
esac

case "$(uname -m)" in
    arm64 | aarch64) DEFAULT_ARCH=aarch64 ;;
    *) DEFAULT_ARCH=x86_64 ;;
esac

ARCH="$DEFAULT_ARCH"
SETTLE_SECONDS=15
GDB_PORT=1234
OUTPUT_DIR=""

while [ $# -gt 0 ]; do
    case "$1" in
        --arch) ARCH="$2"; shift 2 ;;
        --settle) SETTLE_SECONDS="$2"; shift 2 ;;
        --port) GDB_PORT="$2"; shift 2 ;;
        --out) OUTPUT_DIR="$2"; shift 2 ;;
        -h | --help) sed -n '2,39p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done

case "$ARCH" in
    aarch64 | x86_64) ;;
    *) echo "Unsupported --arch '$ARCH' (expected aarch64 or x86_64)" >&2; exit 2 ;;
esac

if [ -z "$OUTPUT_DIR" ]; then
    OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_trace_test_$ARCH"
fi

# ---------------------------------------------------------------------------
# Layout constants, read from the kernel sources that define them.
#
# These are derivations, not pins: if MAX_CPUS or TRACE_BUFFER_SIZE moves, this
# harness follows it. A failed derivation is fatal — silently falling back to a
# guessed value is exactly how the old `* 8` dumped half of a 16-CPU array.
# ---------------------------------------------------------------------------
read_kernel_const() {
    local file="$1" name="$2" value
    value=$(sed -n "s/^pub const ${name}: usize = \([0-9_]*\);.*/\1/p" "$file" | head -1 | tr -d '_')
    if [ -z "$value" ]; then
        echo "Error: could not read ${name} from ${file}" >&2
        exit 1
    fi
    printf '%s' "$value"
}

MAX_CPUS=$(read_kernel_const "$BREENIX_ROOT/kernel/src/tracing/core.rs" MAX_CPUS)
TRACE_BUFFER_SIZE=$(read_kernel_const "$BREENIX_ROOT/kernel/src/tracing/buffer.rs" TRACE_BUFFER_SIZE)

# TraceEvent is #[repr(C, align(16))]: u64 + u16 + u8 + u8 + u32 = 16 bytes.
TRACE_EVENT_SIZE=16
# TraceCpuBuffer metadata after the entries array: write_idx + read_idx +
# dropped + explicit padding = 8 + 8 + 8 + 24.
TRACE_BUFFER_METADATA=48
ENTRIES_SIZE=$((TRACE_BUFFER_SIZE * TRACE_EVENT_SIZE))
# #[repr(C, align(64))] rounds the struct size up to a 64-byte multiple.
BUFFER_SIZE=$(((ENTRIES_SIZE + TRACE_BUFFER_METADATA + 63) / 64 * 64))
TOTAL_SIZE=$((BUFFER_SIZE * MAX_CPUS))

# ---------------------------------------------------------------------------
# Per-arch kernel image, load base, and QEMU command line.
# ---------------------------------------------------------------------------
if [ "$ARCH" = "aarch64" ]; then
    KERNEL_BIN="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
    if [ ! -f "$KERNEL_BIN" ]; then
        echo "Error: ARM64 kernel not found at $KERNEL_BIN. Build with:" >&2
        echo "  cargo build --release --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64" >&2
        exit 1
    fi
    # Durable #528 guard: the aarch64 kernel must be soft-float.
    "$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL_BIN"
    # The aarch64 kernel is linked at its runtime virtual address, so nm already
    # reports the address the MMU-enabled kernel uses. No relocation offset.
    KERNEL_BASE=0
    QEMU_BIN=qemu-system-aarch64
else
    # Cargo keeps one artifact directory per build hash, so `target` holds every
    # x86 kernel this checkout has ever produced. Taking the first `find` hit
    # picks an arbitrary one, and a stale binary makes every symbol address
    # wrong without anything looking wrong: the first x86 run of this harness
    # read TRACE_ENABLED as 0x89485024448b48d0 (instruction bytes) because the
    # chosen binary's segments sat 0x3a000 away from the booted kernel's. Take
    # the most recently built one, which is the one the UEFI image embeds.
    KERNEL_BIN=$(find "$BREENIX_ROOT/target" \
        -path "*/x86_64-unknown-none/release/deps/artifact/*/bin/kernel-*" \
        -type f ! -name "*aarch64*" ! -name "*.d" -print0 2>/dev/null |
        xargs -0 ls -t 2>/dev/null | head -1)
    if [ -z "$KERNEL_BIN" ]; then
        echo "Error: x86_64 kernel binary not found. Build with:" >&2
        echo "  cargo build --release --features testing,external_test_bins --bin qemu-uefi" >&2
        exit 1
    fi
    # The x86_64 kernel is a PIE; the bootloader crate chooses its runtime
    # load base per boot (see the header comment above) and only reveals it
    # on serial once the guest has actually booted. KERNEL_BASE is derived
    # from that serial line further down, after QEMU has started and settled
    # -- any value assigned here would just be a guess.
    QEMU_BIN=qemu-system-x86_64
fi

command -v "$QEMU_BIN" >/dev/null || { echo "Error: $QEMU_BIN not on PATH" >&2; exit 1; }
GDB_BIN="${BREENIX_GDB_BIN:-gdb}"
command -v "$GDB_BIN" >/dev/null || { echo "Error: $GDB_BIN not on PATH (set BREENIX_GDB_BIN)" >&2; exit 1; }

echo "Architecture:  $ARCH"
echo "Kernel binary: $KERNEL_BIN"

symbol_addr() {
    local sym="$1" off
    off=$(nm "$KERNEL_BIN" | awk -v s="$sym" '$3 == s { print $1; exit }')
    if [ -z "$off" ]; then
        echo "Error: symbol $sym not found in $KERNEL_BIN" >&2
        exit 1
    fi
    printf '0x%x' $((KERNEL_BASE + 0x$off))
}

echo ""
echo "Derived layout (from kernel sources, not hardcoded):"
echo "  MAX_CPUS:            $MAX_CPUS"
echo "  TRACE_BUFFER_SIZE:   $TRACE_BUFFER_SIZE events/CPU"
echo "  Per-CPU buffer:      $BUFFER_SIZE bytes"
echo "  TRACE_BUFFERS total: $TOTAL_SIZE bytes"
echo ""
# Symbol addresses depend on KERNEL_BASE, which for x86_64 is not known until
# the guest has booted and printed its load offset on serial -- see the
# derivation after the settle window below.

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

QEMU_PID=""
cleanup() {
    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

echo "Starting $QEMU_BIN with gdbstub on port $GDB_PORT..."

if [ "$ARCH" = "aarch64" ]; then
    EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
    if [ ! -f "$EXT2_DISK" ]; then
        echo "Error: ext2 disk not found at $EXT2_DISK" >&2
        exit 1
    fi
    cp "$EXT2_DISK" "$OUTPUT_DIR/ext2-writable.img"
    qemu_host_lock_acquire
    "$QEMU_BIN" \
        -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
        -kernel "$KERNEL_BIN" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-blk-device,drive=ext2 \
        -drive "if=none,id=ext2,format=raw,file=$OUTPUT_DIR/ext2-writable.img" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial "file:$OUTPUT_DIR/serial.txt" \
        -gdb "tcp::$GDB_PORT" \
        >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
    # F2: registers QEMU with the lock's own EXIT trap (see
    # docker/qemu/lib/qemu-host-lock.sh) so a SIGTERM/SIGINT delivered to
    # just this script's own PID during the settle window still kills QEMU
    # instead of orphaning it with the lock free.
    qemu_host_lock_track_pid "$QEMU_PID"
else
    UEFI_IMG=$(ls -t "$BREENIX_ROOT/target/release/build/breenix-"*/out/breenix-uefi.img 2>/dev/null | head -1)
    if [ -z "$UEFI_IMG" ]; then
        echo "Error: UEFI image not found (run the qemu-uefi build first)" >&2
        exit 1
    fi
    # Pre-existing defect found by round 1 of the KERNEL_BASE-fix review, present
    # on main and unchanged by that fix: a kernel built with the feature set this
    # script itself instructs (testing,external_test_bins) gates get_test_binary()
    # on `feature = "testing"` alone (kernel/src/userspace_test.rs) and requires a
    # second VirtIO block device (index 1) unconditionally -- without it a boot
    # panics with FATAL: DISK LOADING FAILED before the settle window completes
    # (0 of 2 unmodified-script attempts, one per branch, reached
    # TRACE_VALIDATION:PASS -- see the KERNEL_BASE-fix round's evidence README).
    # This device is that second device, wired identically to
    # docker/qemu/run-boot-parallel.sh's testdisk pair.
    TEST_DISK_IMG="$BREENIX_ROOT/target/test_binaries.img"
    if [ ! -f "$TEST_DISK_IMG" ]; then
        echo "Error: test disk image not found at $TEST_DISK_IMG. Repack with:" >&2
        echo "  cargo run -p xtask -- create-test-disk" >&2
        exit 1
    fi
    cp "$BREENIX_ROOT/target/ovmf/x64/code.fd" "$OUTPUT_DIR/OVMF_CODE.fd"
    cp "$BREENIX_ROOT/target/ovmf/x64/vars.fd" "$OUTPUT_DIR/OVMF_VARS.fd"
    "$QEMU_BIN" \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive "if=none,id=hd,format=raw,readonly=on,file=$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=testdisk,format=raw,readonly=on,file=$TEST_DISK_IMG" \
        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
        -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
        -display none -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -serial "file:$OUTPUT_DIR/serial.txt" \
        -gdb "tcp::$GDB_PORT" \
        >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
fi

echo "QEMU started (PID: $QEMU_PID); letting the kernel run ${SETTLE_SECONDS}s to record events..."
sleep "$SETTLE_SECONDS"

if ! kill -0 "$QEMU_PID" 2>/dev/null; then
    echo "Error: QEMU exited during the settle window" >&2
    tail -40 "$OUTPUT_DIR/qemu.log" 2>/dev/null || true
    exit 1
fi

# ---------------------------------------------------------------------------
# Derive the kernel's runtime load base now that the guest has booted, then
# the symbol addresses that depend on it (see the header comment's third
# bullet for why this cannot be done before boot on x86_64).
# ---------------------------------------------------------------------------
if [ "$ARCH" = "x86_64" ]; then
    KERNEL_BASE=$(grep -oE 'virtual_address_offset:[[:space:]]*0x[0-9a-fA-F]+' \
        "$OUTPUT_DIR/serial.txt" 2>/dev/null | head -1 | grep -oE '0x[0-9a-fA-F]+' || true)
    if [ -z "$KERNEL_BASE" ]; then
        echo "Error: no 'virtual_address_offset: 0x...' line found on serial -- cannot" >&2
        echo "  derive the kernel's runtime load base, so no symbol address below can be" >&2
        echo "  trusted (see the header comment's third bullet). The bootloader prints" >&2
        echo "  this once, early in boot; either the settle window (${SETTLE_SECONDS}s)" >&2
        echo "  ended before boot reached that point, or serial capture is broken." >&2
        echo "  Refusing to fall back to a guessed base. Serial tail:" >&2
        tail -40 "$OUTPUT_DIR/serial.txt" 2>/dev/null >&2 || true
        cleanup
        QEMU_PID=""
        exit 1
    fi
fi
echo "Kernel load base: $KERNEL_BASE" | tee "$OUTPUT_DIR/kernel_base.txt"

TRACE_BUFFERS_ADDR=$(symbol_addr TRACE_BUFFERS)
TRACE_ENABLED_ADDR=$(symbol_addr TRACE_ENABLED)
TRACE_CPU0_IDX_ADDR=$(symbol_addr TRACE_CPU0_WRITE_IDX)

echo ""
echo "Symbol addresses:"
echo "  TRACE_BUFFERS:        $TRACE_BUFFERS_ADDR"
echo "  TRACE_ENABLED:        $TRACE_ENABLED_ADDR"
echo "  TRACE_CPU0_WRITE_IDX: $TRACE_CPU0_IDX_ADDR"
echo ""

cat > "$OUTPUT_DIR/gdb_commands.txt" <<EOF
set pagination off
set confirm off
set architecture auto
# Attaching to the QEMU gdbstub is what halts the guest; there is no breakpoint
# address to go stale.
target remote localhost:$GDB_PORT
printf "TRACE_ENABLED = 0x%llx\n", *(unsigned long long*)$TRACE_ENABLED_ADDR
printf "TRACE_CPU0_WRITE_IDX = %llu\n", *(unsigned long long*)$TRACE_CPU0_IDX_ADDR
dump binary memory $OUTPUT_DIR/trace_buffers.bin $TRACE_BUFFERS_ADDR ($TRACE_BUFFERS_ADDR + $TOTAL_SIZE)
dump binary memory $OUTPUT_DIR/trace_enabled.bin $TRACE_ENABLED_ADDR ($TRACE_ENABLED_ADDR + 8)
echo \n=== Trace Memory Dump Complete ===\n
quit
EOF

echo "Attaching GDB and dumping the trace region..."
set +e
timeout 120 "$GDB_BIN" -nx -batch -x "$OUTPUT_DIR/gdb_commands.txt" "$KERNEL_BIN" \
    2>&1 | tee "$OUTPUT_DIR/gdb_output.txt"
GDB_STATUS=${PIPESTATUS[0]}
set -e

cleanup
QEMU_PID=""

echo ""
echo "=== Results ==="
echo ""

if [ ! -f "$OUTPUT_DIR/trace_buffers.bin" ]; then
    echo "FAIL: trace buffer dump was not created (gdb exit $GDB_STATUS)"
    echo ""
    echo "Serial tail:"
    tail -40 "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no serial output)"
    exit 1
fi

DUMP_SIZE=$(wc -c < "$OUTPUT_DIR/trace_buffers.bin" | tr -d ' ')
echo "Trace buffer dump: $DUMP_SIZE bytes (expected $TOTAL_SIZE)"
if [ "$DUMP_SIZE" -ne "$TOTAL_SIZE" ]; then
    echo "FAIL: short dump — the guest memory read did not cover TRACE_BUFFERS"
    exit 1
fi

grep -E '^TRACE_(ENABLED|CPU0_WRITE_IDX)' "$OUTPUT_DIR/gdb_output.txt" || true

TRACE_ENABLED_VALUE=$(sed -n 's/^TRACE_ENABLED = 0x\([0-9a-fA-F]*\)$/\1/p' \
    "$OUTPUT_DIR/gdb_output.txt" | head -1)
if [ -z "$TRACE_ENABLED_VALUE" ] || [ "$((16#$TRACE_ENABLED_VALUE))" -eq 0 ]; then
    echo "FAIL: TRACE_ENABLED is not set — the kernel never enabled tracing"
    exit 1
fi
echo "OK: TRACE_ENABLED is set"

echo ""
echo "Parsing and validating trace data..."
set +e
python3 "$BREENIX_ROOT/scripts/trace_memory_dump.py" \
    --parse "$OUTPUT_DIR/trace_buffers.bin" --max-cpus "$MAX_CPUS" --validate \
    2>&1 | tee "$OUTPUT_DIR/validation.txt"
VALIDATE_STATUS=${PIPESTATUS[0]}
set -e

echo ""
echo "Serial output (first 50 lines):"
head -50 "$OUTPUT_DIR/serial.txt" 2>/dev/null || echo "(no serial output)"

echo ""
if [ "$VALIDATE_STATUS" -eq 0 ]; then
    echo "TRACING_EVIDENCE:$ARCH:PASS"
else
    echo "TRACING_EVIDENCE:$ARCH:FAIL"
fi
echo "Artifacts: $OUTPUT_DIR"
exit "$VALIDATE_STATUS"
