#!/bin/bash
# The x86_64 processor-enumeration gate (#814 PR-1, #629).
#
# WHAT IT SCORES
#
# One boot per `-smp` leg (1, 2 and 4 by default) of the same boot_tests
# kernel image, checking two things per leg:
#
#   (a) the kernel emits exactly one [X86_SMP_ENUM:...] line, and its
#       madt_cpus field equals that leg's -smp value while online stays 1;
#   (b) the boot still reaches the three existing boot_tests pass markers
#       named in PASS_MARKER_* below, identically on each leg.
#
# (a) is the enumeration claim: the count comes from the firmware's MADT, not
# from a compile-time constant, so it MOVES with -smp. (b) is the
# no-regression claim: the extra vCPUs leave the boot as it was, because the
# kernel does not address them.
#
# WHAT HAPPENS TO THE EXTRA CPUs
#
# This kernel does not act on them. It sends no INIT/SIPI, writes no LAPIC
# register and has no AP entry point (#814's census: `grep -rn "SIPI|LAPIC" kernel/src`
# has no output). On a -smp 2 or -smp 4 boot, OVMF starts the application
# processors during its own initialisation and parks them before handing over;
# they stay parked, and the kernel schedules on the boot processor alone. That
# is what online=1 in the marker reports, and it is why (b) is expected to be
# identical across the three legs rather than merely similar.
#
# NOT WIRED INTO ANY OTHER GATE
#
# No existing gate calls this script and this script changes no existing
# gate's QEMU line -- run-x86-boot-tests.sh and
# run-x86-prod-profile-boot-test.sh keep their explicit `-smp 1`, which
# tests/green_program_envelope_structure.rs pins. This gate is run on demand.
#
# Usage:
#   docker/qemu/run-x86-smp-enum-gate.sh              # legs 1, 2, 4
#   docker/qemu/run-x86-smp-enum-gate.sh 2            # only the -smp 2 leg
#   docker/qemu/run-x86-smp-enum-gate.sh 1 2 4        # explicit legs
#
# BREENIX_GATE_TMP (R18/#797) relocates the per-leg output directories so
# concurrent lanes on one host do not clobber each other.

set -euo pipefail
# errtrace: without this the ERR trap below is not inherited into functions.
set -E

report_gate_failure() {
    local exit_code=$?
    local line_no="$1"
    local failing_cmd="$2"
    # #717 idiom: an assertion shaped `test "$(cmd | awk ...)" -eq N` fires this
    # trap inside its own command-substitution subshell first, misattributing
    # the failure; stay silent there and report from depth 0, where the
    # attribution is correct.
    if [ "$BASH_SUBSHELL" -gt 0 ]; then
        exit "$exit_code"
    fi
    echo "x86 SMP enumeration gate: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line_no}, exit ${exit_code})"
    echo "  failing command: ${failing_cmd}"
    if [ -n "${OUTPUT_DIR:-}" ] && compgen -G "$OUTPUT_DIR/serial_*.txt" >/dev/null 2>&1; then
        echo "--- serial tail (last 120 lines per file, $OUTPUT_DIR) ---"
        tail -n 120 "$OUTPUT_DIR"/serial_*.txt
    fi
    exit "$exit_code"
}
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# #865: serialize against other qemu-system-x86_64 lanes on this host --
# this gate's own legs already run one at a time (see the per-leg poll loop
# below), but a concurrent OTHER x86 gate on the same beast host would still
# starve both under TCG the same way #865's own report describes.
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"

# R18/#797: per-lane output base. Defaulting to /tmp keeps a bare invocation
# byte-identical to the other x86 gates' convention.
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "x86 SMP enumeration gate preflight: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP" >&2
       false ;;
esac

# Legs: each argument is one -smp value. Defaults to the three the round
# measured.
if [ "$#" -gt 0 ]; then
    SMP_LEGS=("$@")
else
    SMP_LEGS=(1 2 4)
fi
for leg in "${SMP_LEGS[@]}"; do
    case "$leg" in
        ''|*[!0-9]*) echo "x86 SMP enumeration gate preflight: -smp leg must be a positive integer, got: $leg" >&2
                     false ;;
        0) echo "x86 SMP enumeration gate preflight: -smp leg must be nonzero" >&2
           false ;;
    esac
done

# The marker, by shape. madt_cpus/enabled are substituted per leg; the three
# fields left as [0-9]+ are reported, not asserted: bsp_apic_id and
# cpuid_logical are CPUID readings whose values are the processor model's
# business, and x2apic counts type-9 MADT entries, which QEMU's `pc` machine
# does not emit at these APIC ids.
#
# online=1:max_cpus=1:present=1 is the honest half of this PR: the enumeration
# moved, the dispatch surface did not.
marker_pattern_for_leg() {
    local leg="$1"
    printf '%s' "\[X86_SMP_ENUM:madt_cpus=${leg}:enabled=${leg}:x2apic=[0-9]+:bsp_apic_id=[0-9]+:cpuid_logical=[0-9]+:present=1:online=1:max_cpus=1:src=madt:reason=none\]"
}

# The existing boot_tests pass markers this gate re-checks on each leg. They
# are not new: run-x86-boot-tests.sh pins the same 3 in its own poll condition.
# One is emitted from memory::init (before the enumeration runs), one from the
# mid-boot process/reclaim cohort, and one is the test runner's terminal
# verdict.
PASS_MARKER_EARLY='[TEST:process:frame_custody_refusal_gate:PASS]'
PASS_MARKER_MID='[TEST:process:x86_retire_cohort:PASS]'
PASS_MARKER_TERMINAL='TEST RUNNER: All tests passed'

# Seconds to wait for a leg's terminal marker. run-x86-boot-tests.sh uses 900
# for the same profile under TCG; the extra vCPUs of the -smp 2 and -smp 4
# legs are emulated too, so the same bound is used unchanged rather than
# tightened.
POLL_BOUND_SECONDS="${BREENIX_SMP_ENUM_POLL_SECONDS:-900}"

cd "$BREENIX_ROOT"

BUILD_LOG="$BREENIX_GATE_TMP/breenix_x86_smp_enum_build.log"
mkdir -p "$BREENIX_GATE_TMP"
cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi 2>&1 | tee "$BUILD_LOG"
# Zero-warning law. grep exits 1 on the clean case, so the status is swallowed
# in the group and awk -- whose own exit status here is 0 -- produces the number.
test "$( { grep -c '^warning' "$BUILD_LOG" || true; } | awk '{ print $1 + 0 }')" -eq 0

BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release \
    --features boot_tests,testing,external_test_bins --bin qemu-uefi >/dev/null
# create-test-disk packs userspace/programs/*.elf without rebuilding them, so
# repack on each run to pick up rebuilt userspace.
rm -f target/test_binaries.img
cargo run -p xtask -- create-test-disk
# The ext2 image carries the same userspace binaries, so rebuild it on each run.
rm -f target/ext2.img
./scripts/create_ext2_disk.sh

UEFI_IMG=$(ls -t target/release/build/breenix-*/out/breenix-uefi.img | head -1)
test -n "$UEFI_IMG"
echo "x86 SMP enumeration gate: booted image sha256 $(sha256sum "$BREENIX_ROOT/$UEFI_IMG" | awk '{ print $1 }')"

# #865: acquired once for the whole leg loop below (each leg's own poll
# loop below waits for that leg's QEMU_PID to exit or reach a terminal
# marker, then kills and waits on it, before the next leg starts, so this
# is one occupant of the x86 lock domain for the batch, matching
# run-boot-parallel.sh's own batch-acquire shape), released once after the
# loop completes.
qemu_host_lock_acquire qemu-system-x86_64

failures=0
for smp in "${SMP_LEGS[@]}"; do
    OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_x86_smp_enum_$smp"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
    cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"

    echo "=== leg -smp $smp ==="
    qemu-system-x86_64 \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img" \
        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
        -machine pc -accel tcg,thread=multi -cpu qemu64 -smp "$smp" -m 512 \
        -display none -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -serial "file:$OUTPUT_DIR/serial_user.txt" \
        -serial "file:$OUTPUT_DIR/serial_kernel.txt" \
        >"$OUTPUT_DIR/qemu.log" 2>&1 &
    QEMU_PID=$!
    qemu_host_lock_track_pid "$QEMU_PID"

    reached=false
    for _ in $(seq 1 "$POLL_BOUND_SECONDS"); do
        if grep -qF -- "$PASS_MARKER_TERMINAL" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            reached=true
            break
        fi
        if grep -qF -- 'TEST RUNNER: FAILED' "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            break
        fi
        if ! kill -0 "$QEMU_PID" 2>/dev/null; then
            break
        fi
        sleep 1
    done

    # Own PID only: this gate kills the process it started, not a process name.
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true

    leg_ok=true

    marker_count=$( { grep -hcE -- '\[X86_SMP_ENUM:' "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; } | awk '{ total += $1 } END { print total + 0 }')
    if [ "$marker_count" -ne 1 ]; then
        echo "  leg -smp $smp: FAIL (expected exactly 1 [X86_SMP_ENUM:] line, found $marker_count)"
        leg_ok=false
    fi

    expected_pattern="$(marker_pattern_for_leg "$smp")"
    if grep -qE -- "$expected_pattern" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
        echo "  leg -smp $smp: marker $(grep -hoE -- '\[X86_SMP_ENUM:[^]]*\]' "$OUTPUT_DIR"/serial_*.txt | head -1)"
    else
        echo "  leg -smp $smp: FAIL (no marker matching madt_cpus=$smp:enabled=$smp:...:online=1)"
        echo "    found: $( { grep -hoE -- '\[X86_SMP_ENUM:[^]]*\]' "$OUTPUT_DIR"/serial_*.txt 2>/dev/null || true; } | head -1)"
        leg_ok=false
    fi

    for marker in "$PASS_MARKER_EARLY" "$PASS_MARKER_MID" "$PASS_MARKER_TERMINAL"; do
        if ! grep -qF -- "$marker" "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            echo "  leg -smp $smp: FAIL (existing pass marker absent: $marker)"
            leg_ok=false
        fi
    done

    if [ "$reached" != true ]; then
        echo "  leg -smp $smp: FAIL (terminal marker not reached within ${POLL_BOUND_SECONDS}s)"
        leg_ok=false
    fi

    if [ "$leg_ok" = true ]; then
        echo "  leg -smp $smp: PASS"
    else
        failures=$((failures + 1))
        echo "  leg -smp $smp: serial at $OUTPUT_DIR"
    fi
done

qemu_host_lock_release

if [ "$failures" -eq 0 ]; then
    echo "x86 SMP enumeration gate: PASS (${#SMP_LEGS[@]} leg(s): ${SMP_LEGS[*]})"
else
    echo "x86 SMP enumeration gate: FAIL ($failures of ${#SMP_LEGS[@]} leg(s) red)"
    # #802/#805 idiom, pinned by tests/teardown_structure.rs's
    # gate_scripts_with_verdict_trap_have_no_preempting_exits: a bare `exit 1`
    # here would end the script without the ERR trap's own verdict line, so
    # fail through `false` and let report_gate_failure print and re-raise.
    false
fi
