#!/bin/bash
# ARM64 production-profile TTY evidence gate (green program, arc 4).
#
# Scores /bin/tty_oracle, which init launches on every aarch64 boot. The point
# of this gate is the profile: it builds the kernel with NO --features, exactly
# as scripts/parallels/build-efi.sh ships it, so the TTY, PTY, line-discipline
# and termios surface is measured on the kernel that actually deploys rather
# than on a boot_tests build that carries registry tests nothing ships.
#
# Carries the #668 ERR-trap discipline: a red gate names the failing assertion,
# its file and line, and dumps serial, instead of dying silently under `set -e`.
# Crash checks run BEFORE completion checks so a panic is reported as a panic
# rather than as "the leg never finished".
#
# Usage: run-aarch64-tty-oracle-gate.sh [--boots N] [--rebuild-userspace]

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

OUTPUT_ROOT="/tmp/breenix_aarch64_tty_oracle"
BOOTS=1
REBUILD_USERSPACE=false
CURRENT_SERIAL=""

# Every arm the oracle is required to report PASS for. tests/tty_oracle_structure.rs
# holds this list to the arms the oracle actually emits, so an arm can neither be
# dropped from the program nor added without the gate scoring it.
EXPECTED_ARMS=(
    openpt
    nonblock_open
    isatty
    termios_roundtrip
    canonical_line
    icrnl
    raw_passthrough
    echo
    onlcr
    winsize
    foreground_pgrp
    hangup
)
EXPECTED_ARM_COUNT=${#EXPECTED_ARMS[@]}

# The oracle's own summary. Anti-vacuity: pass=0 or a missing marker is a FAIL,
# never a skip.
COMPLETE_LITERAL="[TTY_ORACLE:COMPLETE:pass=${EXPECTED_ARM_COUNT}:fail=0]"
ANY_COMPLETE_LITERAL='[TTY_ORACLE:COMPLETE:'
ARM_FAIL_LITERAL='[TTY_ORACLE:FAIL:'
# init's post-wait record: proves the child was actually reaped with status 0.
INIT_EXIT_LITERAL='[init] tty_oracle exited pid='
# Liveness after the leg. The oracle's own final line is not accepted as
# evidence that the kernel is still usable.
BSSHD_LITERAL='bsshd: listening'
# boot_tests-only markers that must be wholly absent from a shipped profile.
BOOT_TESTS_LITERAL='[BOOT_TESTS:'
CRASH_MARKERS_PATTERN='KERNEL PANIC|panic!|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup detected'

report_gate_failure() {
    local exit_code=$?
    local line=${BASH_LINENO[0]}
    trap - ERR EXIT
    echo "aarch64 TTY oracle gate: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line}, exit ${exit_code})"
    echo "  failing command: ${BASH_COMMAND}"
    if [ -n "$CURRENT_SERIAL" ] && [ -f "$CURRENT_SERIAL" ]; then
        echo "--- TTY oracle lines ---"
        grep -aF '[TTY_ORACLE:' "$CURRENT_SERIAL" | sort -u || true
        echo "--- serial tail (last 200 lines, $CURRENT_SERIAL) ---"
        tail -200 "$CURRENT_SERIAL" || true
    fi
    exit "$exit_code"
}
trap report_gate_failure ERR

while [ $# -gt 0 ]; do
    case "$1" in
        --boots) BOOTS="$2"; shift 2 ;;
        --rebuild-userspace) REBUILD_USERSPACE=true; shift ;;
        *) echo "FAIL: unknown argument: $1"; exit 1 ;;
    esac
done

case "$BOOTS" in
    ''|*[!0-9]*) echo "FAIL: --boots must be a positive integer"; exit 1 ;;
esac
[ "$BOOTS" -ge 1 ] || { echo "FAIL: --boots must be at least 1"; exit 1; }

marker_count() {
    grep -aF -c "$2" "$1" 2>/dev/null || true
}

echo "Building the shipped ARM64 production kernel profile..."
# The absence of --features is the point: adding one would make this gate
# measure a different profile than the image scripts/parallels/build-efi.sh ships.
if ! (cd "$BREENIX_ROOT" && cargo build --release --target aarch64-breenix-kernel.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
        -p kernel --bin kernel-aarch64); then
    echo "FAIL: production-profile kernel build failed"
    exit 1
fi

KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
[ -f "$KERNEL" ] || { echo "FAIL: production kernel missing at $KERNEL"; exit 1; }

# Durable #528 guard: the shipped kernel must remain on the soft-float target.
if ! "$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"; then
    echo "FAIL: production kernel failed the no-NEON guard"
    exit 1
fi

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if $REBUILD_USERSPACE; then
    "$BREENIX_ROOT/userspace/programs/build.sh" --arch aarch64
    "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64
fi
if [ ! -f "$EXT2_DISK" ]; then
    echo "FAIL: ext2 disk not found at $EXT2_DISK"
    echo "Re-run with --rebuild-userspace to build userspace and create it."
    exit 1
fi

rm -rf "$OUTPUT_ROOT"
mkdir -p "$OUTPUT_ROOT"

boot=1
while [ "$boot" -le "$BOOTS" ]; do
    RUN_DIR="$OUTPUT_ROOT/boot_$boot"
    mkdir -p "$RUN_DIR"
    SERIAL="$RUN_DIR/serial.txt"
    CURRENT_SERIAL="$SERIAL"
    : > "$SERIAL"
    cp "$EXT2_DISK" "$RUN_DIR/ext2-writable.img"

    echo "Booting the ARM64 production profile (boot $boot/$BOOTS)..."
    timeout 120 qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu max -m 512 -smp 4 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-blk-device,drive=ext2 \
        -drive if=none,id=ext2,format=raw,file="$RUN_DIR/ext2-writable.img" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$SERIAL" >/dev/null 2>&1 &
    QEMU_PID=$!

    POLL=0
    while [ "$POLL" -lt 120 ]; do
        if grep -aqF "$BSSHD_LITERAL" "$SERIAL" 2>/dev/null; then break; fi
        if grep -aqiE "$CRASH_MARKERS_PATTERN" "$SERIAL" 2>/dev/null; then break; fi
        kill -0 "$QEMU_PID" 2>/dev/null || break
        POLL=$((POLL + 1))
        sleep 1
    done
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true

    # --- Crash checks first: a panic must be reported as a panic. ---
    CRASH_COUNT=$(grep -aiE -c "$CRASH_MARKERS_PATTERN" "$SERIAL" 2>/dev/null || true)
    if [ "$CRASH_COUNT" -ne 0 ]; then
        echo "FAIL: boot $boot crashed - $CRASH_COUNT crash marker(s)"
        grep -aiE "$CRASH_MARKERS_PATTERN" "$SERIAL" | head -5
        exit 1
    fi

    # --- The leg must have run at all. ---
    if [ "$(marker_count "$SERIAL" "$ANY_COMPLETE_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot produced no [TTY_ORACLE:COMPLETE:] marker - the leg never ran"
        echo "  (a boot that does not drive the TTY surface cannot satisfy this gate)"
        exit 1
    fi

    # --- No arm may report a failure. ---
    ARM_FAIL_COUNT=$(marker_count "$SERIAL" "$ARM_FAIL_LITERAL")
    if [ "$ARM_FAIL_COUNT" -ne 0 ]; then
        echo "FAIL: boot $boot - $ARM_FAIL_COUNT TTY arm failure(s)"
        grep -aF "$ARM_FAIL_LITERAL" "$SERIAL" | sort -u
        exit 1
    fi

    # --- Every expected arm must have reported PASS. ---
    for arm in "${EXPECTED_ARMS[@]}"; do
        if [ "$(marker_count "$SERIAL" "[TTY_ORACLE:${arm}:verdict=PASS")" -eq 0 ]; then
            echo "FAIL: boot $boot - arm '${arm}' produced no PASS verdict"
            grep -aF '[TTY_ORACLE:' "$SERIAL" | sort -u
            exit 1
        fi
    done

    # --- The oracle's own tally must agree with the arm census. ---
    if [ "$(marker_count "$SERIAL" "$COMPLETE_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - missing '$COMPLETE_LITERAL'"
        grep -aF "$ANY_COMPLETE_LITERAL" "$SERIAL" | sort -u
        exit 1
    fi

    # --- init must have reaped the child with status 0. ---
    if [ "$(marker_count "$SERIAL" "${INIT_EXIT_LITERAL}")" -eq 0 ]; then
        echo "FAIL: boot $boot - init never recorded the tty_oracle child exiting"
        exit 1
    fi
    if [ "$(grep -acE '\[init\] tty_oracle exited pid=[0-9]+ code=0' "$SERIAL" || true)" -eq 0 ]; then
        echo "FAIL: boot $boot - tty_oracle exited nonzero"
        grep -aF "$INIT_EXIT_LITERAL" "$SERIAL" | head -2
        exit 1
    fi

    # --- The shipped profile must carry no boot_tests-only output. ---
    if [ "$(marker_count "$SERIAL" "$BOOT_TESTS_LITERAL")" -ne 0 ]; then
        echo "FAIL: boot $boot - boot_tests-only markers present in the production profile"
        exit 1
    fi

    # --- Liveness AFTER the leg: the kernel is still usable. ---
    if [ "$(marker_count "$SERIAL" "$BSSHD_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - kernel did not reach bsshd after the TTY leg"
        exit 1
    fi

    echo "  boot $boot: $EXPECTED_ARM_COUNT/$EXPECTED_ARM_COUNT arms PASS, kernel live (bsshd reached)"
    boot=$((boot + 1))
done

CURRENT_SERIAL=""
trap - ERR
echo "PASS: aarch64 TTY oracle gate - $BOOTS/$BOOTS boots, $EXPECTED_ARM_COUNT arms green on the shipped production profile"
echo "Serials: $OUTPUT_ROOT/boot_*/serial.txt"
