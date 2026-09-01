#!/bin/bash
# x86_64 production-profile TTY evidence gate (green program, TTY-x86 port).
#
# The x86 port of docker/qemu/run-aarch64-tty-oracle-gate.sh. Scores
# /bin/tty_oracle, which init now launches on every x86_64 production boot
# (userspace/programs/src/init.rs's x86_64-gated run_tty_oracle(), called
# right before start_bsshd(), same placement as the aarch64 launcher and
# strictly before init's boot-script chain -- see #722). The point of this
# gate is the profile: it builds qemu-uefi with NO --features, exactly as
# the image ships, so the TTY, PTY, line-discipline and termios surface is
# measured on the kernel that actually deploys rather than on a boot_tests
# build that carries registry tests nothing ships. Boot/build mechanics
# below match run-x86-prod-profile-boot-test.sh's own invocation (three
# virtio-blk devices: UEFI image, placeholder, ext2 at index 2).
#
# ONE ARM FEWER THAN AARCH64, VISIBLY: cloexec_exec (arm 14 on aarch64) is
# excluded here, not silently dropped. #721 (x86 exec() ENOSYS in the
# zero-feature production build) is CLOSED -- exec works -- but re-admitting
# this arm surfaced a second, distinct gap: #745, x86 fork() is
# unconditionally refused in that same production build. The arm's child
# forks before it ever execs, so running it today would misattribute #745 to
# the TTY/PTY layer instead of the process layer. tty_oracle.rs's run() gates
# the arm_cloexec_exec() call behind #[cfg(target_arch = "aarch64")] and
# ARM_COUNT is arch-conditional (14 aarch64, 13 x86) for exactly this
# reason -- see that file's own #745 comment. EXPECTED_ARMS below is
# therefore the 13-entry x86 list, and this gate ALSO asserts the arm
# never reports a verdict at all, so a regression that re-enables it
# unconditionally (bypassing the cfg) is caught here even though nothing
# in EXPECTED_ARMS would otherwise notice a bonus PASS.
# tests/tty_oracle_structure.rs carries the census that keeps this file's
# EXPECTED_ARMS in sync with the oracle's actual x86-reachable arm set.
#
# Carries the #668 ERR-trap discipline: a red gate names the failing
# assertion, its file and line, and dumps serial, instead of dying
# silently under `set -e`. Crash checks run BEFORE completion checks so a
# panic is reported as a panic rather than as "the leg never finished".
#
# Usage: run-x86-tty-oracle-gate.sh [--boots N] [--rebuild-userspace]

set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

OUTPUT_ROOT="/tmp/breenix_x86_tty_oracle"
BOOTS=1
REBUILD_USERSPACE=false
QEMU_PID=""
CURRENT_RUN_DIR=""

# Every arm the oracle is required to report PASS for on x86 -- the 14
# aarch64 arms minus cloexec_exec (see the header, #745).
# tests/tty_oracle_structure.rs holds this list to the arms the oracle
# actually drives on x86 (its own arch-aware census reads run()'s cfg
# gates, not this array), so an arm can neither be dropped from the
# program nor added without the gate scoring it.
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
)
EXPECTED_ARM_COUNT=${#EXPECTED_ARMS[@]}

# The oracle's own summary. Anti-vacuity: pass=0 or a missing marker is a
# FAIL, never a skip.
COMPLETE_LITERAL="[TTY_ORACLE:COMPLETE:pass=${EXPECTED_ARM_COUNT}:fail=0]"
ANY_COMPLETE_LITERAL='[TTY_ORACLE:COMPLETE:'
ARM_FAIL_LITERAL='[TTY_ORACLE:FAIL:'
CLOEXEC_EXEC_VERDICT_LITERAL='[TTY_ORACLE:cloexec_exec:'
# init's post-wait record. This line only prints on a genuine `Ok` reap from
# `waitpid` (review finding B3: run_tty_oracle() used to discard the
# `Result` with `let _ =`, so a failed reap could still fabricate
# `code=0` off the pre-zeroed status -- fixed to match run_spawn_smoke()'s
# honest branch). Combined with the code=0 regex check below and
# INIT_REAP_FAILED_LITERAL staying absent, this genuinely proves the child
# was reaped with status 0, not merely that init printed a line saying so.
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
CRASH_MARKERS_PATTERN='KERNEL PANIC|panic!|DOUBLE FAULT|TRIPLE FAULT|soft lockup detected'

report_gate_failure() {
    local exit_code=$?
    local line=${BASH_LINENO[0]}
    trap - ERR
    echo "x86 TTY oracle gate: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line}, exit ${exit_code})"
    echo "  failing command: ${BASH_COMMAND}"
    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
        QEMU_PID=""
    fi
    if [ -n "$CURRENT_RUN_DIR" ] && compgen -G "$CURRENT_RUN_DIR/serial_*.txt" >/dev/null 2>&1; then
        echo "--- TTY oracle lines ---"
        grep -aF '[TTY_ORACLE:' "$CURRENT_RUN_DIR"/serial_*.txt | sort -u || true
        echo "--- serial tail (last 200 lines) ---"
        tail -n 200 "$CURRENT_RUN_DIR"/serial_*.txt || true
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

# Single-argument form, scoped to the current boot's run dir (matches
# run-x86-prod-profile-boot-test.sh's own marker_count(), which greps the
# same two-serial-stream layout this gate produces per boot).
marker_count() {
    local literal="$1"
    local total
    total=$( { grep -aF -h -c -- "$literal" "$CURRENT_RUN_DIR"/serial_*.txt 2>/dev/null || true; } \
        | awk '{ total += $1 } END { print total + 0 }')
    printf '%s' "$total"
}

crash_count() {
    local total
    total=$( { grep -aE -h -c -- "$CRASH_MARKERS_PATTERN" "$CURRENT_RUN_DIR"/serial_*.txt 2>/dev/null || true; } \
        | awk '{ total += $1 } END { print total + 0 }')
    printf '%s' "$total"
}

cd "$BREENIX_ROOT"

echo "Building the shipped x86_64 production kernel profile..."
# The absence of --features is the point: adding one would make this gate
# measure a different profile than the one that ships.
rm -f target/release/build/breenix-*/out/breenix-uefi.img
BUILD_LOG=/tmp/breenix_x86_tty_oracle_build.log
cargo build --release --bin qemu-uefi 2>&1 | tee "$BUILD_LOG"
# Zero-warning law. grep exits 1 on the clean case, so the status is
# swallowed in the group and awk -- which always exits 0 -- produces the
# number.
test "$( { grep -c '^warning' "$BUILD_LOG" || true; } | awk '{ print $1 + 0 }')" -eq 0
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release --bin qemu-uefi >/dev/null
UEFI_IMG=$(ls -t target/release/build/breenix-*/out/breenix-uefi.img | head -1)
test -n "$UEFI_IMG"

EXT2_IMG="$BREENIX_ROOT/target/ext2.img"
if $REBUILD_USERSPACE; then
    ./userspace/programs/build.sh
    rm -f target/ext2.img
    ./scripts/create_ext2_disk.sh
fi
if [ ! -f "$EXT2_IMG" ]; then
    echo "FAIL: ext2 disk not found at $EXT2_IMG"
    echo "Re-run with --rebuild-userspace to build userspace and create it."
    exit 1
fi

rm -rf "$OUTPUT_ROOT"
mkdir -p "$OUTPUT_ROOT"
cp target/ovmf/x64/code.fd "$OUTPUT_ROOT/OVMF_CODE.fd"
cp target/ovmf/x64/vars.fd "$OUTPUT_ROOT/OVMF_VARS.fd"
# Zero-filled stand-in for the test-binaries disk production does not carry.
# Its only job is to occupy virtio-blk index 1 so the ext2 root lands on
# index 2, which is where init_root_fs() looks for it.
dd if=/dev/zero of="$OUTPUT_ROOT/placeholder.img" bs=1M count=16 status=none

boot=1
while [ "$boot" -le "$BOOTS" ]; do
    RUN_DIR="$OUTPUT_ROOT/boot_$boot"
    mkdir -p "$RUN_DIR"
    CURRENT_RUN_DIR="$RUN_DIR"

    echo "Booting the x86_64 production profile with the TTY oracle (boot $boot/$BOOTS)..."
    qemu-system-x86_64 \
        -pflash "$OUTPUT_ROOT/OVMF_CODE.fd" \
        -pflash "$OUTPUT_ROOT/OVMF_VARS.fd" \
        -drive "if=none,id=hd,format=raw,readonly=on,file=$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=placeholder,format=raw,readonly=on,file=$OUTPUT_ROOT/placeholder.img" \
        -device virtio-blk-pci,drive=placeholder,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$EXT2_IMG" \
        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
        -netdev user,id=net0 \
        -device e1000,netdev=net0,mac=52:54:00:12:34:56 \
        -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
        -display none -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -chardev "socket,id=console,path=$RUN_DIR/console.sock,server=on,wait=off,logfile=$RUN_DIR/serial_user.txt" \
        -serial chardev:console \
        -serial "file:$RUN_DIR/serial_kernel.txt" \
        >"$RUN_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!

    # This gate boots the identical profile run-x86-prod-profile-boot-test.sh
    # measures (steady state at 14s under TCG on beast) plus one extra spawn
    # (tty_oracle) ahead of bsshd, so its own bound matches that sibling
    # gate's 240s -- an order of magnitude of headroom above the measured
    # steady state, not an unrationalized round number.
    POLL=0
    while [ "$POLL" -lt 240 ]; do
        if grep -aqF "$BSSHD_LITERAL" "$RUN_DIR"/serial_*.txt 2>/dev/null; then break; fi
        if grep -aqE "$CRASH_MARKERS_PATTERN" "$RUN_DIR"/serial_*.txt 2>/dev/null; then break; fi
        kill -0 "$QEMU_PID" 2>/dev/null || break
        POLL=$((POLL + 1))
        sleep 1
    done
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    QEMU_PID=""

    # --- Crash checks first: a panic must be reported as a panic. ---
    CRASH_COUNT=$(crash_count)
    if [ "$CRASH_COUNT" -ne 0 ]; then
        echo "FAIL: boot $boot crashed - $CRASH_COUNT crash marker(s)"
        grep -aiE "$CRASH_MARKERS_PATTERN" "$RUN_DIR"/serial_*.txt | head -5
        exit 1
    fi

    # --- The leg must have run at all. ---
    if [ "$(marker_count "$ANY_COMPLETE_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot produced no [TTY_ORACLE:COMPLETE:] marker - the leg never ran"
        echo "  (a boot that does not drive the TTY surface cannot satisfy this gate)"
        exit 1
    fi

    # --- No arm may report a failure. ---
    ARM_FAIL_COUNT=$(marker_count "$ARM_FAIL_LITERAL")
    if [ "$ARM_FAIL_COUNT" -ne 0 ]; then
        echo "FAIL: boot $boot - $ARM_FAIL_COUNT TTY arm failure(s)"
        grep -aF "$ARM_FAIL_LITERAL" "$RUN_DIR"/serial_*.txt | sort -u
        exit 1
    fi

    # --- Every expected arm must have reported PASS. ---
    for arm in "${EXPECTED_ARMS[@]}"; do
        if [ "$(marker_count "[TTY_ORACLE:${arm}:verdict=PASS")" -eq 0 ]; then
            echo "FAIL: boot $boot - arm '${arm}' produced no PASS verdict"
            grep -aF '[TTY_ORACLE:' "$RUN_DIR"/serial_*.txt | sort -u
            exit 1
        fi
    done

    # --- cloexec_exec must report NO verdict at all on x86 (#745). A verdict
    #     here -- pass or fail -- means the aarch64-only cfg in tty_oracle.rs's
    #     run() was bypassed, so the arm ran against #745's fork() refusal
    #     unnoticed. ---
    if [ "$(marker_count "$CLOEXEC_EXEC_VERDICT_LITERAL")" -ne 0 ]; then
        echo "FAIL: boot $boot - cloexec_exec reported a verdict on x86 (excluded pending #745)"
        grep -aF "$CLOEXEC_EXEC_VERDICT_LITERAL" "$RUN_DIR"/serial_*.txt | sort -u
        exit 1
    fi

    # --- The oracle's own tally must agree with the arm census. ---
    if [ "$(marker_count "$COMPLETE_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - missing '$COMPLETE_LITERAL'"
        grep -aF "$ANY_COMPLETE_LITERAL" "$RUN_DIR"/serial_*.txt | sort -u
        exit 1
    fi

    # --- init must have reaped the child with status 0, via a genuine
    #     waitpid() success -- not a failed reap over a pre-zeroed status. ---
    if [ "$(marker_count "$INIT_REAP_FAILED_LITERAL")" -ne 0 ]; then
        echo "FAIL: boot $boot - init's waitpid() on tty_oracle failed"
        grep -aF "$INIT_REAP_FAILED_LITERAL" "$RUN_DIR"/serial_*.txt | head -2
        exit 1
    fi
    if [ "$(marker_count "$INIT_EXIT_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - init never recorded the tty_oracle child exiting"
        exit 1
    fi
    if [ "$(grep -aE -h -c '\[init\] tty_oracle exited pid=[0-9]+ code=0' "$RUN_DIR"/serial_*.txt 2>/dev/null | awk '{ total += $1 } END { print total + 0 }')" -eq 0 ]; then
        echo "FAIL: boot $boot - tty_oracle exited nonzero"
        grep -aF "$INIT_EXIT_LITERAL" "$RUN_DIR"/serial_*.txt | head -2
        exit 1
    fi

    # --- The shipped profile must carry no boot_tests-only output. ---
    if [ "$(marker_count "$BOOT_TESTS_LITERAL")" -ne 0 ]; then
        echo "FAIL: boot $boot - boot_tests-only markers present in the production profile"
        exit 1
    fi

    # --- Liveness AFTER the leg: the kernel is still usable. ---
    if [ "$(marker_count "$BSSHD_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - kernel did not reach bsshd after the TTY leg"
        exit 1
    fi

    echo "  boot $boot: $EXPECTED_ARM_COUNT/$EXPECTED_ARM_COUNT arms PASS, kernel live (bsshd reached)"
    boot=$((boot + 1))
done

CURRENT_RUN_DIR=""
trap - ERR
echo "PASS: x86 TTY oracle gate - $BOOTS/$BOOTS boots, $EXPECTED_ARM_COUNT arms green on the shipped production profile (cloexec_exec excluded pending #745)"
echo "Serials: $OUTPUT_ROOT/boot_*/serial_*.txt"
