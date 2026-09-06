#!/bin/bash
#
# run-lockup-capture-oracle.sh - the GDB-supervised `edge=LOCKUP` oracle run.
#
# WHAT IT RUNS. A kernel built `--features boot_tests,capture_lockup_oracle` on
# `-M virt,gic-version=3 -cpu cortex-a72 -m 512 -smp 2`. CPU1 holds the REAL
# scheduler guard with interrupts masked for longer than the soft-lockup
# detector's own threshold while CPU0 stays in the oracle coordinator with
# interrupts enabled, so CPU0's hardware timer keeps reaching
# `check_soft_lockup`. It repeats that twice in one boot.
#
# WHAT GDB IS FOR. Two things this run cannot get from serial:
#   * the ARMING PRECONDITION -- CPU0's deferred-requeue slot must be empty
#     before a hold starts, because the exception-return path drains that slot
#     under a BLOCKING scheduler acquisition. A nonzero slot is a setup failure
#     to report, not a condition to clear.
#   * the RECEIPTS -- the kernel's own tick, context-switch, syscall and
#     exit-kick-heartbeat counters at acquire and at release, plus which CPU
#     actually held the guard and whether the hold ended by release or by its
#     hardware-clock fail-safe.
# Both are sampled at explicit coordinator checkpoints, OUTSIDE the held
# interval, through a watchpoint on the phase word. No breakpoint is left in
# the detector while the acceptance serial is being collected.
#
# WHAT IT DOES NOT DO. It does not call `dump_lockup_state`, `check_soft_lockup`
# or `capture::emit`; does not write a tick counter, the watchdog baseline or
# `WATCHDOG_REPORTED`; does not shorten the threshold. The only guest memory it
# writes is the oracle's own host-ack word.
#
# Exit code:
#   0 - the run reached DONE and its 15 per-episode receipt slots were collected
#   1 - setup/usage/tooling failure, or the run did not reach DONE
#
# Usage:
#   ./scripts/run-lockup-capture-oracle.sh <kernel-elf> [--out DIR]

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# #826/#834/R181: at most one aarch64 QEMU boot alive on this host at a time.
# This oracle holds a peer CPU for five seconds of GUEST time twice, so a
# starved guest clock would change what it measures, not merely how long it
# takes.
# shellcheck source=../docker/qemu/lib/qemu-host-lock.sh
source "$REPO_ROOT/docker/qemu/lib/qemu-host-lock.sh"

KERNEL_ELF=""
OUT_DIR=""
HOST_CEILING_SECS=60

while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT_DIR="${2:-}"; shift 2 ;;
        -*) echo "ERROR: unknown option $1" >&2; exit 1 ;;
        *) KERNEL_ELF="$1"; shift ;;
    esac
done

if [ -z "$KERNEL_ELF" ] || [ ! -f "$KERNEL_ELF" ]; then
    echo "ERROR: give the aarch64 oracle kernel ELF as the first argument." >&2
    exit 1
fi
if [ -z "$OUT_DIR" ]; then
    OUT_DIR="$REPO_ROOT/target/lockup-capture-oracle"
fi
mkdir -p "$OUT_DIR"
SERIAL_FILE="$OUT_DIR/serial.txt"
RECEIPT_FILE="$OUT_DIR/receipts.txt"
GDB_LOG="$OUT_DIR/gdb.log"
: > "$SERIAL_FILE"
: > "$RECEIPT_FILE"

find_nm() {
    if command -v llvm-nm >/dev/null 2>&1; then
        command -v llvm-nm; return 0
    fi
    local sysroot cand
    sysroot="$(rustc --print sysroot 2>/dev/null)"
    if [ -n "$sysroot" ]; then
        cand="$(ls "$sysroot"/lib/rustlib/*/bin/llvm-nm 2>/dev/null | head -1)"
        if [ -n "$cand" ]; then echo "$cand"; return 0; fi
    fi
    if command -v nm >/dev/null 2>&1; then
        command -v nm; return 0
    fi
    return 1
}

NM="$(find_nm)"
if [ -z "$NM" ]; then
    echo "ERROR: no nm found (need llvm-nm or nm)." >&2
    exit 1
fi
SYMS="$OUT_DIR/symbols.txt"
"$NM" "$KERNEL_ELF" > "$SYMS" 2>/dev/null

# Profile refusal. This oracle scores an `edge=LOCKUP` capture; a kernel that
# also fires the self-test or panic edge, or one built with a shrunken byte
# budget, produces captures this runner would be scoring by accident.
require_symbol() {
    if ! grep -q "$1" "$SYMS"; then
        echo "ERROR: this ELF carries no symbol matching: $1" >&2
        echo "Build it with --features boot_tests,capture_lockup_oracle" >&2
        exit 1
    fi
}

refuse_symbol() {
    if grep -q "$1" "$SYMS"; then
        echo "ERROR: this ELF carries another capture edge or budget mutation." >&2
        echo "Refused symbol pattern: $1" >&2
        exit 1
    fi
}

require_symbol "run_lockup_capture_oracle"
require_symbol "dump_lockup_state"
refuse_symbol "capture8selftest"
refuse_symbol "capture_oracle4fire"

if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then
    echo "ERROR: qemu-system-aarch64 not on PATH." >&2
    exit 1
fi
GDB_BIN="${BREENIX_GDB_BIN:-gdb}"
if ! command -v "$GDB_BIN" >/dev/null 2>&1; then
    echo "ERROR: no gdb on PATH; set BREENIX_GDB_BIN." >&2
    exit 1
fi

EXT2_DISK="$REPO_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "ERROR: ext2 disk not found at $EXT2_DISK" >&2
    exit 1
fi
WRITABLE="$OUT_DIR/ext2-writable.img"
cp "$EXT2_DISK" "$WRITABLE"

GDB_PORT="${BREENIX_LOCKUP_GDB_PORT:-1244}"
QEMU_PID=""
GDB_PID=""

# Only PIDs this script started are ever signalled.
stop_pid() {
    local pid="$1"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
        kill "$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
    fi
}

cleanup() {
    stop_pid "$GDB_PID"
    stop_pid "$QEMU_PID"
    qemu_host_lock_release
}
trap cleanup EXIT INT TERM

echo "Oracle: BXCAP edge=LOCKUP, two episodes, GDB-supervised"
echo "  kernel:   $KERNEL_ELF"
echo "  out:      $OUT_DIR"
echo "  gdb port: $GDB_PORT"

qemu_host_lock_acquire
qemu-system-aarch64 \
    -M virt,gic-version=3 -cpu cortex-a72 -m 512 -smp 2 \
    -kernel "$KERNEL_ELF" \
    -display none -no-reboot \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -device virtio-tablet-device \
    -device virtio-blk-device,drive=ext2 \
    -drive if=none,id=ext2,format=raw,file="$WRITABLE" \
    -device virtio-net-device,netdev=net0 \
    -netdev user,id=net0 \
    -serial file:"$SERIAL_FILE" \
    -S -gdb tcp::"$GDB_PORT" &
QEMU_PID=$!
sleep 1

# The kernel is built without DWARF, so the driver is handed ADDRESSES from
# the ELF symbol table rather than names for GDB to resolve. A symbol that
# does not resolve is a hard failure: a driver reading address 0 would report
# zeroes as receipts.
addr_of() {
    local pattern="$1"
    local line
    line="$(grep -E "$pattern" "$SYMS" | head -1)"
    if [ -z "$line" ]; then
        echo "ERROR: no symbol matching: $pattern" >&2
        exit 1
    fi
    echo "$line" | awk '{print $1}'
}

export BXCAP_ADDR_ORACLE_PHASE="$(addr_of 'capture_lockup_oracle12ORACLE_PHASE')"
export BXCAP_ADDR_ORACLE_EPISODE="$(addr_of 'capture_lockup_oracle14ORACLE_EPISODE')"
export BXCAP_ADDR_ORACLE_HOST_ACK="$(addr_of 'capture_lockup_oracle15ORACLE_HOST_ACK')"
export BXCAP_ADDR_ORACLE_SETUP_FAILURE="$(addr_of 'capture_lockup_oracle20ORACLE_SETUP_FAILURE')"
export BXCAP_ADDR_ORACLE_THRESHOLD_TICKS="$(addr_of 'capture_lockup_oracle22ORACLE_THRESHOLD_TICKS')"
export BXCAP_ADDR_ORACLE_TSFREQ_HZ="$(addr_of 'capture_lockup_oracle16ORACLE_TSFREQ_HZ')"
export BXCAP_ADDR_DEFERRED_REQUEUE="$(addr_of 'context_switch16DEFERRED_REQUEUE')"
export BXCAP_ADDR_EXIT_KICK_HEARTBEAT="$(addr_of 'EXIT_KICK_GATE_WATCHDOG_HEARTBEAT')"

# The 15 per-episode receipt slots, by name. `llvm-nm`'s mangling puts
# the identifier's byte length before it, so each pattern carries that length
# and cannot match a differently named symbol by prefix.
for pair in \
    "RCPT_ACQUIRED:13RCPT_ACQUIRED" \
    "RCPT_CPU:8RCPT_CPU" \
    "RCPT_TICK_AT_ACQUIRE:20RCPT_TICK_AT_ACQUIRE" \
    "RCPT_TICK_AT_RELEASE:20RCPT_TICK_AT_RELEASE" \
    "RCPT_HELD_TICKS:15RCPT_HELD_TICKS" \
    "RCPT_CTX_AT_ACQUIRE:19RCPT_CTX_AT_ACQUIRE" \
    "RCPT_CTX_AT_RELEASE:19RCPT_CTX_AT_RELEASE" \
    "RCPT_SYSCALL_AT_ACQUIRE:23RCPT_SYSCALL_AT_ACQUIRE" \
    "RCPT_SYSCALL_AT_RELEASE:23RCPT_SYSCALL_AT_RELEASE" \
    "RCPT_HEARTBEAT_AT_ACQUIRE:25RCPT_HEARTBEAT_AT_ACQUIRE" \
    "RCPT_HEARTBEAT_AT_RELEASE:25RCPT_HEARTBEAT_AT_RELEASE" \
    "RCPT_PROGRESS_MOVED_DURING_HOLD:31RCPT_PROGRESS_MOVED_DURING_HOLD" \
    "RCPT_EXPIRED:12RCPT_EXPIRED" \
    "RCPT_TS_AT_ACQUIRE:18RCPT_TS_AT_ACQUIRE" \
    "RCPT_TS_AT_RELEASE:18RCPT_TS_AT_RELEASE" \
; do
    name="${pair%%:*}"
    pattern="capture_lockup_oracle${pair#*:}"
    export "BXCAP_ADDR_$name=$(addr_of "$pattern")"
done

export BXCAP_LOCKUP_RECEIPTS="$RECEIPT_FILE"
export BXCAP_LOCKUP_GDB_PORT="$GDB_PORT"
export BXCAP_LOCKUP_HOST_CEILING="$HOST_CEILING_SECS"

"$GDB_BIN" -batch -nx \
    -ex "source $SCRIPT_DIR/lockup-capture-oracle.py" \
    > "$GDB_LOG" 2>&1 &
GDB_PID=$!

WAITED=0
while kill -0 "$GDB_PID" 2>/dev/null; do
    if [ "$WAITED" -ge "$((HOST_CEILING_SECS + 15))" ]; then
        echo "HOST CEILING: the GDB driver did not finish within $WAITED s." >&2
        stop_pid "$GDB_PID"
        GDB_PID=""
        echo "verdict=HOST_CEILING_SHELL" >> "$RECEIPT_FILE"
        break
    fi
    sleep 1
    WAITED=$((WAITED + 1))
done

stop_pid "$QEMU_PID"
QEMU_PID=""

echo ""
echo "  serial:   $SERIAL_FILE"
echo "  receipts: $RECEIPT_FILE"
echo "  gdb log:  $GDB_LOG"
echo ""
sed -n '1,200p' "$RECEIPT_FILE"

if grep -q '^verdict=DONE$' "$RECEIPT_FILE"; then
    echo ""
    echo "RUN REACHED DONE. Scoring the serial is the schema suite's job, not this"
    echo "script's: see tests/capture_bxcap_schema_structure.rs."
    exit 0
fi
echo ""
echo "RUN DID NOT REACH DONE. The receipts above say where it stopped." >&2
exit 1
