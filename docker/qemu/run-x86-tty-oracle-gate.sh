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
# claim-lint:ok: the arc's negative control,
# docs/planning/745-x86-fork/serials/anti-vacuity-pre-fix-refused-gate-2026-09-02.txt,
# does NOT carry arm 14's own failure -- that boot ran a 13-arm build in
# which cloexec_exec never executed (`[TTY_ORACLE:COMPLETE:pass=13:fail=0]`,
# no cloexec_exec verdict line at all). What it carries is fork_smoke's own
# refusal effect on the same pre-#745 kernel (`[FORK_SMOKE:FORK_FAILED
# ENOMEM]`), evidence that x86 fork() was refused, not a record of this
# specific arm failing under that refusal.
# FULL PARITY WITH AARCH64: cloexec_exec (arm 14) is re-admitted. It was
# excluded first pending #721 (sys_execv_with_frame returned ENOSYS in the
# x86 zero-feature production build -- the arm's child could never actually
# exec()), then -- once #721 closed and re-admission surfaced a second,
# distinct blocker -- pending #745 (x86 fork() unconditionally refused in
# that same profile; the arm's child forks before it ever execs). Both are
# now closed: tty_oracle.rs's run() calls arm_cloexec_exec() unconditionally
# and ARM_COUNT is 14 on both arches. EXPECTED_ARMS below is therefore the
# full 14-entry list, the same one run-aarch64-tty-oracle-gate.sh scores.
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

# #797: concurrent lanes sharing one host (e.g. the beast Incus container)
# each invoking this script hardcode the identical /tmp/breenix_x86_tty_oracle
# path, so one lane's rm -rf/mkdir can clobber another lane's in-flight run.
# Defaulting to /tmp keeps every existing caller byte-identical; a
# concurrent-lane launcher sets this to a per-clone directory instead. Must
# be absolute -- a relative value would resolve against whatever directory
# happens to be current when each command runs (review finding F6 on #797).
# claim-lint:ok: #797, diff-empty against origin/main except one line
# (BUILD_LOG) that only gained quotes -- see
# docs/planning/green-program/gates/GATE-TMP-BASEDIR-2026-09-05.md
#
# The absolute-path VALUE is not judged here -- that check is spent in the
# BASE-DIR PREFLIGHT block right after the ERR trap is installed below
# (#802/#805 idiom, widened to this gate: a bare `exit` here would run before
# the trap exists and could end the gate with no verdict line at all).
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
OUTPUT_ROOT="$BREENIX_GATE_TMP/breenix_x86_tty_oracle"
BOOTS=1
REBUILD_USERSPACE=false
QEMU_PID=""
CURRENT_RUN_DIR=""

# Every arm the oracle is required to report PASS for on x86 -- all 14,
# matching aarch64 (see the header, #721/#745). 14 of 14 observed green on the
# shipped profile, 25 boots running:
# docs/planning/745-x86-fork/serials/tty-oracle-25boot-soak-2026-09-02.txt
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
    cloexec_exec
)
EXPECTED_ARM_COUNT=${#EXPECTED_ARMS[@]}

# The oracle's own summary. Anti-vacuity: pass=0 or a missing marker is a
# FAIL, never a skip.
COMPLETE_LITERAL="[TTY_ORACLE:COMPLETE:pass=${EXPECTED_ARM_COUNT}:fail=0]"
ANY_COMPLETE_LITERAL='[TTY_ORACLE:COMPLETE:'
ARM_FAIL_LITERAL='[TTY_ORACLE:FAIL:'
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

# --- BASE-DIR PREFLIGHT (#797 F6, routed through the verdict path by the
# #802/#805 idiom widened to this gate) ---
#
# This is the check on the operator-controlled BREENIX_GATE_TMP, and it runs
# HERE -- immediately after the ERR trap is installed, as the first command
# to run under it -- rather than beside the assignment that derives its
# subject. A bare `exit` there, before the trap exists, can end the gate with
# no verdict line at all. Failing here instead spends the rejection through
# report_gate_failure: the gate's own "x86 TTY oracle gate: FAIL (...)" line
# prints, names the failing command, and re-raises the nonzero status.
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "x86 TTY oracle gate preflight: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP" >&2
       false ;;
esac

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

# AF_UNIX sun_path is 108 bytes on Linux including the terminating NUL, so a
# console-socket path over 107 characters cannot be bound (review finding F7
# on #797, carried here from the same guard in
# run-x86-prod-profile-boot-test.sh). Checked here against the widest run
# dir this invocation can produce ($BOOTS, its own last and longest boot
# number) rather than inside the per-boot loop, so a too-long
# BREENIX_GATE_TMP fails before the build below runs at all.
WIDEST_CONSOLE_SOCK_PATH="$OUTPUT_ROOT/boot_$BOOTS/console.sock"
if [ "${#WIDEST_CONSOLE_SOCK_PATH}" -gt 107 ]; then
    echo "FAIL: console socket path exceeds the AF_UNIX sun_path limit of 107 chars: \"$WIDEST_CONSOLE_SOCK_PATH\" is ${#WIDEST_CONSOLE_SOCK_PATH} chars -- shorten BREENIX_GATE_TMP"
    false
fi

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
BUILD_LOG="$BREENIX_GATE_TMP/breenix_x86_tty_oracle_build.log"
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
    false
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
        false
    fi

    # --- The leg must have run at all. ---
    if [ "$(marker_count "$ANY_COMPLETE_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot produced no [TTY_ORACLE:COMPLETE:] marker - the leg never ran"
        echo "  (a boot that does not drive the TTY surface cannot satisfy this gate)"
        false
    fi

    # --- No arm may report a failure. ---
    ARM_FAIL_COUNT=$(marker_count "$ARM_FAIL_LITERAL")
    if [ "$ARM_FAIL_COUNT" -ne 0 ]; then
        echo "FAIL: boot $boot - $ARM_FAIL_COUNT TTY arm failure(s)"
        grep -aF "$ARM_FAIL_LITERAL" "$RUN_DIR"/serial_*.txt | sort -u
        false
    fi

    # --- Every expected arm must have reported PASS. ---
    for arm in "${EXPECTED_ARMS[@]}"; do
        if [ "$(marker_count "[TTY_ORACLE:${arm}:verdict=PASS")" -eq 0 ]; then
            echo "FAIL: boot $boot - arm '${arm}' produced no PASS verdict"
            grep -aF '[TTY_ORACLE:' "$RUN_DIR"/serial_*.txt | sort -u
            false
        fi
    done

    # --- The oracle's own tally must agree with the arm census. ---
    if [ "$(marker_count "$COMPLETE_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - missing '$COMPLETE_LITERAL'"
        grep -aF "$ANY_COMPLETE_LITERAL" "$RUN_DIR"/serial_*.txt | sort -u
        false
    fi

    # --- init must have reaped the child with status 0, via a genuine
    #     waitpid() success -- not a failed reap over a pre-zeroed status. ---
    if [ "$(marker_count "$INIT_REAP_FAILED_LITERAL")" -ne 0 ]; then
        echo "FAIL: boot $boot - init's waitpid() on tty_oracle failed"
        grep -aF "$INIT_REAP_FAILED_LITERAL" "$RUN_DIR"/serial_*.txt | head -2
        false
    fi
    if [ "$(marker_count "$INIT_EXIT_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - init never recorded the tty_oracle child exiting"
        false
    fi
    if [ "$(grep -aE -h -c '\[init\] tty_oracle exited pid=[0-9]+ code=0' "$RUN_DIR"/serial_*.txt 2>/dev/null | awk '{ total += $1 } END { print total + 0 }')" -eq 0 ]; then
        echo "FAIL: boot $boot - tty_oracle exited nonzero"
        grep -aF "$INIT_EXIT_LITERAL" "$RUN_DIR"/serial_*.txt | head -2
        false
    fi

    # --- The shipped profile must carry no boot_tests-only output. ---
    if [ "$(marker_count "$BOOT_TESTS_LITERAL")" -ne 0 ]; then
        echo "FAIL: boot $boot - boot_tests-only markers present in the production profile"
        false
    fi

    # --- Liveness AFTER the leg: the kernel is still usable. ---
    if [ "$(marker_count "$BSSHD_LITERAL")" -eq 0 ]; then
        echo "FAIL: boot $boot - kernel did not reach bsshd after the TTY leg"
        false
    fi

    echo "  boot $boot: $EXPECTED_ARM_COUNT/$EXPECTED_ARM_COUNT arms PASS, kernel live (bsshd reached)"
    boot=$((boot + 1))
done

CURRENT_RUN_DIR=""
trap - ERR
echo "PASS: x86 TTY oracle gate - $BOOTS/$BOOTS boots, $EXPECTED_ARM_COUNT arms green on the shipped production profile"
echo "Serials: $OUTPUT_ROOT/boot_*/serial_*.txt"
