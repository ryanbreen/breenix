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

# #825: two concurrent runs of this gate on the same host each hardcoded the
# identical /tmp/breenix_aarch64_tty_oracle path, so one run's rm -rf/mkdir
# could delete and rewrite another run's in-flight boot output -- the same
# hazard PR #801 fixed for this gate's x86 twin, run-x86-tty-oracle-gate.sh,
# for #797. Defaulting to /tmp keeps every existing caller byte-identical; a
# concurrent-lane launcher sets this to a per-worktree directory instead.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: a relative value would resolve against whatever
# directory happens to be current when it is read (the same F6 guard PR
# #801 gave the x86 gate scripts for #797).
# This gate's own house convention (see the comment above trap
# report_gate_failure ERR further down) is echo + bare `false`, never
# `exit`, so every rejection is a textually uniform statement the whole-file
# no-pre-empting-exit ratchet can police without a position-dependent
# exemption for a guard that runs before the trap below is installed.
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "FAIL: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP"; false ;;
esac
OUTPUT_ROOT="$BREENIX_GATE_TMP/breenix_aarch64_tty_oracle"
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
    ctty
    cloexec_exec
)
EXPECTED_ARM_COUNT=${#EXPECTED_ARMS[@]}

# The oracle's own summary. Anti-vacuity: pass=0 or a missing marker is a FAIL,
# never a skip.
COMPLETE_LITERAL="[TTY_ORACLE:COMPLETE:pass=${EXPECTED_ARM_COUNT}:fail=0]"
ANY_COMPLETE_LITERAL='[TTY_ORACLE:COMPLETE:'
ARM_FAIL_LITERAL='[TTY_ORACLE:FAIL:'
# init's post-wait record. This line only prints on a genuine `Ok` reap from
# `waitpid` (review finding B3, x86 fix round: run_tty_oracle() used to
# discard the `Result` with `let _ =` on both arches, so a failed reap could
# still fabricate `code=0` off the pre-zeroed status -- fixed to match
# run_spawn_smoke()'s honest branch). Combined with the code=0 regex check
# below and INIT_REAP_FAILED_LITERAL staying absent, this genuinely proves
# the child was reaped with status 0, not merely that init printed a line
# saying so.
INIT_EXIT_LITERAL='[init] tty_oracle exited pid='
# The distinct literal a genuine waitpid() failure prints instead of the
# line above -- its presence would mean the exit-record pin above was never
# actually reached via a real reap.
INIT_REAP_FAILED_LITERAL='[init] Warning: tty_oracle reap failed'
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

# The 17 checks below this point that used to stop with a bare `exit`
# (13 standalone plus 4 case-arm/`||`-group forms) now fail with `echo` +
# bare `false` instead (#802/#805 idiom, widened to this gate): a bare
# `exit` does not reach the trap above -- it terminates the process
# directly, the same way it would with no trap installed at all (verified:
# `exit N` does not fire an ERR trap under `set -e`, unlike a
# nonzero-returning command) -- so it can end the gate with no verdict
# line. `false` under `set -e`/`set -E` fires the trap, so a rejection is
# spent through report_gate_failure and the trap's own re-raise
# (`exit "$exit_code"` above) stays the only `exit` statement left in this
# script.
while [ $# -gt 0 ]; do
    case "$1" in
        --boots) BOOTS="$2"; shift 2 ;;
        --rebuild-userspace) REBUILD_USERSPACE=true; shift ;;
        *) echo "FAIL: unknown argument: $1"; false ;;
    esac
done

case "$BOOTS" in
    ''|*[!0-9]*) echo "FAIL: --boots must be a positive integer"; false ;;
esac
[ "$BOOTS" -ge 1 ] || { echo "FAIL: --boots must be at least 1"; false; }

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
    false
fi

KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
[ -f "$KERNEL" ] || { echo "FAIL: production kernel missing at $KERNEL"; false; }

# Durable #528 guard: the shipped kernel must remain on the soft-float target.
if ! "$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"; then
    echo "FAIL: production kernel failed the no-NEON guard"
    false
fi

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if $REBUILD_USERSPACE; then
    "$BREENIX_ROOT/userspace/programs/build.sh" --arch aarch64
    "$BREENIX_ROOT/scripts/create_ext2_disk.sh" --arch aarch64
fi
if [ ! -f "$EXT2_DISK" ]; then
    echo "FAIL: ext2 disk not found at $EXT2_DISK"
    echo "Re-run with --rebuild-userspace to build userspace and create it."
    false
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
        false
    fi

    # --- The leg must have run at all. ---
    if [ "$(marker_count "$SERIAL" "$ANY_COMPLETE_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot produced no [TTY_ORACLE:COMPLETE:] marker - the leg never ran"
        echo "  (a boot that does not drive the TTY surface cannot satisfy this gate)"
        false
    fi

    # --- No arm may report a failure. ---
    ARM_FAIL_COUNT=$(marker_count "$SERIAL" "$ARM_FAIL_LITERAL")
    if [ "$ARM_FAIL_COUNT" -ne 0 ]; then
        echo "FAIL: boot $boot - $ARM_FAIL_COUNT TTY arm failure(s)"
        grep -aF "$ARM_FAIL_LITERAL" "$SERIAL" | sort -u
        false
    fi

    # --- Every expected arm must have reported PASS. ---
    for arm in "${EXPECTED_ARMS[@]}"; do
        if [ "$(marker_count "$SERIAL" "[TTY_ORACLE:${arm}:verdict=PASS")" -eq 0 ]; then
            echo "FAIL: boot $boot - arm '${arm}' produced no PASS verdict"
            grep -aF '[TTY_ORACLE:' "$SERIAL" | sort -u
            false
        fi
    done

    # --- The oracle's own tally must agree with the arm census. ---
    if [ "$(marker_count "$SERIAL" "$COMPLETE_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - missing '$COMPLETE_LITERAL'"
        grep -aF "$ANY_COMPLETE_LITERAL" "$SERIAL" | sort -u
        false
    fi

    # --- init must have reaped the child with status 0, via a genuine
    #     waitpid() success -- not a failed reap over a pre-zeroed status. ---
    if [ "$(marker_count "$SERIAL" "$INIT_REAP_FAILED_LITERAL")" -ne 0 ]; then
        echo "FAIL: boot $boot - init's waitpid() on tty_oracle failed"
        grep -aF "$INIT_REAP_FAILED_LITERAL" "$SERIAL" | head -2
        false
    fi
    if [ "$(marker_count "$SERIAL" "${INIT_EXIT_LITERAL}")" -eq 0 ]; then
        echo "FAIL: boot $boot - init never recorded the tty_oracle child exiting"
        false
    fi
    if [ "$(grep -acE '\[init\] tty_oracle exited pid=[0-9]+ code=0' "$SERIAL" || true)" -eq 0 ]; then
        echo "FAIL: boot $boot - tty_oracle exited nonzero"
        grep -aF "$INIT_EXIT_LITERAL" "$SERIAL" | head -2
        false
    fi

    # --- The shipped profile must carry no boot_tests-only output. ---
    if [ "$(marker_count "$SERIAL" "$BOOT_TESTS_LITERAL")" -ne 0 ]; then
        echo "FAIL: boot $boot - boot_tests-only markers present in the production profile"
        false
    fi

    # --- Liveness AFTER the leg: the kernel is still usable. ---
    if [ "$(marker_count "$SERIAL" "$BSSHD_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - kernel did not reach bsshd after the TTY leg"
        false
    fi

    echo "  boot $boot: $EXPECTED_ARM_COUNT/$EXPECTED_ARM_COUNT arms PASS, kernel live (bsshd reached)"
    boot=$((boot + 1))
done

CURRENT_SERIAL=""
trap - ERR
echo "PASS: aarch64 TTY oracle gate - $BOOTS/$BOOTS boots, $EXPECTED_ARM_COUNT arms green on the shipped production profile"
echo "Serials: $OUTPUT_ROOT/boot_*/serial.txt"
