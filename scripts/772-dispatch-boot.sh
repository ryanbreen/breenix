#!/usr/bin/env bash
# One full-profile x86 boot for the #772 dispatch census.
#
# Free-runs the gate kernel with the QEMU gdbstub open, polls for the gate's own
# completion markers, reads the `DISPATCH_*` trace counters out-of-band over GDB
# (after the markers, i.e. after the recv episode this boot measures has
# resolved), stops the guest, scores the verdict with the same scripts
# run-x86-gate.sh uses, and emits the census JSON.
#
# It replaces the fix round's ad-hoc driver on two points:
#
#   * cleanup is BY PID -- only the QEMU this script started is killed. The
#     earlier driver matched process NAMES, which reaps other slots' guests on a
#     shared host.
#   * the counter list is not hard-coded. Each `DISPATCH_*` symbol in the
#     kernel ELF is read, so counters added later are picked up with no edit.
#
# Usage: 772-dispatch-boot.sh <outdir> <tag> [<repo-root>]
# The kernel must already be built for the profile under test.

set -uo pipefail

OUTDIR="${1:?usage: 772-dispatch-boot.sh <outdir> <tag> [<repo-root>]}"
TAG="${2:?usage: 772-dispatch-boot.sh <outdir> <tag> [<repo-root>]}"
REPO="${3:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

cd "$REPO" || exit 1
# shellcheck disable=SC1091
source "$HOME/.cargo/env" 2>/dev/null || export PATH="$HOME/.cargo/bin:$PATH"

rm -rf "$OUTDIR"
mkdir -p "$OUTDIR"

KERNEL_BIN=$(find "$REPO/target" \
    -path "*/x86_64-unknown-none/release/deps/artifact/*/bin/kernel-*" \
    -type f ! -name "*aarch64*" ! -name "*.d" -print0 2>/dev/null |
    xargs -0 ls -t 2>/dev/null | head -1)
if [ -z "$KERNEL_BIN" ]; then
    echo "RESULT tag=$TAG outdir=$OUTDIR error=no_kernel_elf"
    exit 2
fi

# The DISPATCH_* counter symbols in this build, name and file-relative address.
nm "$KERNEL_BIN" | awk '$3 ~ /^DISPATCH_/ { print $3, "0x"$1 }' | sort > "$OUTDIR/counter_symbols.txt"

if [ -w /dev/kvm ]; then ACCEL=kvm; else ACCEL=tcg; fi
if [ "$ACCEL" = kvm ]; then CPU=host; else CPU=qemu64; fi

BREENIX_NET_MODE=none BREENIX_QEMU_ACCEL=$ACCEL BREENIX_QEMU_CPU=$CPU \
  timeout 150 ./target/release/qemu-uefi \
  -serial file:"$OUTDIR/serial_user.log" \
  -serial file:"$OUTDIR/serial_kernel.log" \
  -s \
  > "$OUTDIR/stdout.log" 2>&1 &
QPID=$!

FOUND=0
for _ in $(seq 1 280); do
    sleep 0.5
    if grep -q 'USERSPACE TEST COMPLETE' "$OUTDIR/serial_kernel.log" "$OUTDIR/serial_user.log" 2>/dev/null &&
       grep -q 'TEST_TALLY:' "$OUTDIR/serial_kernel.log" "$OUTDIR/serial_user.log" 2>/dev/null; then
        FOUND=1
        break
    fi
    kill -0 "$QPID" 2>/dev/null || break
done
sleep 1

GDB_OK=0
if [ "$FOUND" = 1 ] && kill -0 "$QPID" 2>/dev/null; then
    KERNEL_BASE=$(grep -ohE 'virtual_address_offset:[[:space:]]*0x[0-9a-fA-F]+' \
        "$OUTDIR/serial_kernel.log" "$OUTDIR/serial_user.log" 2>/dev/null |
        head -1 | grep -oE '0x[0-9a-fA-F]+')
    if [ -n "$KERNEL_BASE" ]; then
        {
            echo "set pagination off"
            echo "set confirm off"
            echo "target remote localhost:1234"
            while read -r name addr; do
                # +64 is per_cpu[0].value: the TraceCounter header (name and
                # description, 32 bytes) padded to the 64-byte per-CPU slot.
                printf 'printf "%s_CPU0=%%lu\\n", *(unsigned long long*)(0x%x + 64)\n' \
                    "$name" "$((KERNEL_BASE + addr))"
            done < "$OUTDIR/counter_symbols.txt"
            echo "detach"
            echo "quit"
        } > "$OUTDIR/gdbcmd.txt"
        timeout 30 gdb -nx -batch -x "$OUTDIR/gdbcmd.txt" "$KERNEL_BIN" > "$OUTDIR/gdb_output.txt" 2>&1
        if grep -q 'DISPATCH_KERNEL_RESTORE_TOTAL_CPU0=' "$OUTDIR/gdb_output.txt"; then
            GDB_OK=1
        fi
    fi
fi

# Cleanup: only the guest this script started, by PID (R84).
if kill -0 "$QPID" 2>/dev/null; then
    kill -9 "$QPID" 2>/dev/null
fi
wait "$QPID" 2>/dev/null

VERDICT_RC=1
if EXPECTED_EXITS=10 ./scripts/x86-gate-verdict.sh \
        "$OUTDIR/serial_user.log" "$OUTDIR/serial_kernel.log" \
        > "$OUTDIR/verdict.txt" 2>&1; then
    VERDICT_RC=0
fi

python3 ./scripts/772-dispatch-census.py \
    "$OUTDIR/serial_kernel.log" "$OUTDIR/serial_user.log" \
    --counters "$OUTDIR/gdb_output.txt" \
    > "$OUTDIR/census.json" 2>"$OUTDIR/census_err.txt"

echo "RESULT tag=$TAG outdir=$OUTDIR markers_found=$FOUND gdb_ok=$GDB_OK verdict_rc=$VERDICT_RC"
cat "$OUTDIR/census.json"
