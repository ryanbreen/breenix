#!/bin/bash
#
# Core-proof gate — the harness's own gate wrapper.
#
# It boots a `coreproof`-profile kernel once per seed and adjudicates the two
# marker lines the harness emits:
#
#   [COREPROOF:RUN:v1:comp=A:seed=0x…:iters=…:sites_declared=N:sites_visited=N:…]
#   [COREPROOF:VIOLATION:v1:comp=A:seed=0x…:iter=…:site=…:pred=…:…]
#
# THREE GATE-FAILING CONDITIONS, and they are the whole verdict:
#
#   1. Any VIOLATION line.
#   2. A missing or malformed RUN line — a boot that emitted no run record proves
#      nothing, and "no violations" from a harness that never ran is the exact
#      false green this gate exists to refuse.
#   3. sites_visited < sites_declared. This is the vacuity guard. Both numbers
#      come from the harness's own site census, so the gate compares two numbers
#      and never a literal list of site names: adding a site changes both sides
#      automatically, and a site that is declared but never reached is a red.
#      Pinning a name list here is the mistake this campaign has made three times
#      (#549, #551, #527-r1) and it is not repeated.
#
# A boot that fails its ordinary boot tests, panics, or takes a fatal exception
# fails the gate too — the harness rides a real boot and an unexplained boot
# failure is never absorbed. UNATTRIBUTED is gate-failing.
#
# VERDICT DISCIPLINE (#668)
#
# Every assertion is a plain command under `set -e`, and `set -E` plus the ERR
# trap make each one loud: a silent `set -e` abort prints nothing of its own, so
# a genuine red would otherwise die with no verdict text and no serial pointer.
# The trap fires on every uncaught nonzero exit, names the failing command and
# line, points at the serial, and re-raises the same status.
#
# Usage:
#   docker/qemu/run-coreproof-gate.sh [--component A] [--seeds N] [--profile max|cortex-a72|both]
#                                     [--mode pen|adversarial|ambient] [--seed 0xHEX]
#
# Defaults follow the 2026-08-18 gate-size directive: 25 boots per profile. More
# boots is deliberately the wrong lever for this harness — the lever is more
# iterations and better site labelling inside one boot, and a run that needs 200
# boots to find something is a run whose sites are wrong.

set -euo pipefail
# errtrace: without this the ERR trap is not inherited into shell functions, and
# the failure reporter is itself invoked from that trap.
set -E

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

COMPONENT="A"
SEEDS=25
PROFILE="both"
MODE="pen"
PINNED_SEED=""
GATE_TARGET_DIR="$BREENIX_ROOT/target/coreproof-gate"
OUTPUT_ROOT="/tmp/breenix_coreproof_gate"

while [ $# -gt 0 ]; do
    case "$1" in
        --component) COMPONENT="$2"; shift 2 ;;
        --seeds) SEEDS="$2"; shift 2 ;;
        --profile) PROFILE="$2"; shift 2 ;;
        --mode) MODE="$2"; shift 2 ;;
        --seed) PINNED_SEED="$2"; shift 2 ;;
        --features) EXTRA_FEATURES="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
EXTRA_FEATURES="${EXTRA_FEATURES:-}"

case "$PROFILE" in
    max) PROFILES=(max) ;;
    cortex-a72) PROFILES=(cortex-a72) ;;
    both) PROFILES=(max cortex-a72) ;;
    *) echo "unknown profile: $PROFILE" >&2; exit 2 ;;
esac

RUN_PREFIX='[COREPROOF:RUN:'
VIOLATION_PREFIX='[COREPROOF:VIOLATION:'
BOOT_TESTS_PASS_LITERAL='[BOOT_TESTS:PASS]'

QEMU_PID=""
CURRENT_SERIAL=""

report_gate_failure() {
    local status=$?
    local line=$1
    local command=$2
    echo ""
    echo "ARM64 CORE-PROOF GATE: FAILED"
    echo "  at line $line: $command (exit $status)"
    if [ -n "$CURRENT_SERIAL" ] && [ -f "$CURRENT_SERIAL" ]; then
        echo "  serial: $CURRENT_SERIAL"
        echo "  --- last 30 lines ---"
        tail -30 "$CURRENT_SERIAL" || true
    fi
    exit "$status"
}
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

cleanup() {
    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# Build. Its own target dir, so the shared aarch64 kernel other gates boot is
# never replaced by a harness-feature build [[gate-target-fidelity-528]], and
# always the SOFT-FLOAT kernel target — building the userspace target here would
# silently re-arm #528.
# ---------------------------------------------------------------------------
FEATURES="boot_tests,coreproof"
if [ -n "$EXTRA_FEATURES" ]; then
    FEATURES="$FEATURES,$EXTRA_FEATURES"
fi

echo "Building coreproof kernel (features: $FEATURES)..."
BUILD_ENV=(env "CARGO_TARGET_DIR=$GATE_TARGET_DIR")
if [ -n "$PINNED_SEED" ]; then
    # The seed channel is compile-time (`option_env!`), so a pinned seed is a
    # warm rebuild rather than a runtime knob. That is the pilot's deliberate
    # choice: three lines and no device support.
    BUILD_ENV+=("BREENIX_COREPROOF_SEED=$PINNED_SEED")
fi
# The mode is read the same way, so `--mode` has to reach the BUILD, not the
# boot. Passing it only to QEMU would leave every run in the default pen while
# the verdict line claimed otherwise.
BUILD_ENV+=("BREENIX_COREPROOF_MODE=$MODE")
( cd "$BREENIX_ROOT" && "${BUILD_ENV[@]}" cargo build --release \
    --features "$FEATURES" \
    --target aarch64-breenix-kernel.json \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
    -p kernel --bin kernel-aarch64 )
KERNEL="$GATE_TARGET_DIR/aarch64-breenix-kernel/release/kernel-aarch64"

"$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"
"$BREENIX_ROOT/scripts/check-coreproof-seams.sh"

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
test -f "$EXT2_DISK"

rm -rf "$OUTPUT_ROOT"
mkdir -p "$OUTPUT_ROOT"

TOTAL_BOOTS=0
TOTAL_VIOLATIONS=0
TOTAL_ITERS=0

boot_once() {
    local profile="$1"
    local index="$2"
    local dir="$OUTPUT_ROOT/${profile}_${index}"
    rm -rf "$dir"; mkdir -p "$dir"
    CURRENT_SERIAL="$dir/serial.txt"
    local ext2="$dir/ext2-writable.img"
    cp "$EXT2_DISK" "$ext2"

    timeout 40 qemu-system-aarch64 \
        -M virt,gic-version=3 -cpu "$profile" -m 512 -smp 4 \
        -kernel "$KERNEL" \
        -display none -no-reboot \
        -device virtio-gpu-device \
        -device virtio-keyboard-device \
        -device virtio-tablet-device \
        -device virtio-blk-device,drive=ext2 \
        -drive if=none,id=ext2,format=raw,file="$ext2" \
        -device virtio-net-device,netdev=net0 \
        -netdev user,id=net0 \
        -serial file:"$CURRENT_SERIAL" &
    QEMU_PID=$!

    local waited=0
    while [ "$waited" -lt 38 ]; do
        if [ -f "$CURRENT_SERIAL" ] && grep -qaF "$BOOT_TESTS_PASS_LITERAL" "$CURRENT_SERIAL" 2>/dev/null \
            && [ "$(grep -acF "$RUN_PREFIX" "$CURRENT_SERIAL" 2>/dev/null || echo 0)" -ge 2 ]; then
            break
        fi
        if ! kill -0 "$QEMU_PID" 2>/dev/null; then
            break
        fi
        sleep 1
        waited=$((waited + 1))
    done
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    QEMU_PID=""
}

adjudicate() {
    local serial="$1"
    local label="$2"

    # ---- an unexplained boot failure is never absorbed -------------------
    local fatal
    fatal="$(grep -aiE 'KERNEL PANIC|DATA_ABORT|INSTRUCTION_ABORT|Unhandled sync exception|soft lockup detected' "$serial" 2>/dev/null | tail -1 || true)"
    if [ -n "$fatal" ]; then
        echo "  $label: UNATTRIBUTED boot failure"
        echo "    $fatal"
        return 1
    fi
    if grep -qaF '[BOOT_TESTS:FAIL' "$serial" 2>/dev/null; then
        local failure
        failure="$(grep -ahoE '\[TEST:[^]]*:FAIL:[^]]*\]' "$serial" 2>/dev/null | head -1 || true)"
        echo "  $label: boot-test failure ${failure:-[TEST:<missing>:FAIL:<missing>]}"
        return 1
    fi

    # ---- condition 1: any violation --------------------------------------
    local violations
    violations="$(grep -acF "$VIOLATION_PREFIX" "$serial" 2>/dev/null || true)"
    violations="${violations:-0}"
    if [ "$violations" -ne 0 ]; then
        echo "  $label: $violations violation(s)"
        grep -aF "$VIOLATION_PREFIX" "$serial" | sed 's/^/    /'
        TOTAL_VIOLATIONS=$((TOTAL_VIOLATIONS + violations))
        return 1
    fi

    # ---- condition 2: a well-formed closing run record --------------------
    # The harness emits an opening record carrying the seed before its first
    # iteration and a closing record with the achieved counts, so a run that
    # dies mid-flight still has its seed on the wire. The gate adjudicates the
    # LAST record, which is the closing one.
    local run
    run="$(grep -aF "$RUN_PREFIX" "$serial" 2>/dev/null | tail -1 || true)"
    if [ -z "$run" ]; then
        echo "  $label: no RUN record"
        return 1
    fi
    local seed declared visited iters
    seed="$(echo "$run" | grep -oE 'seed=0x[0-9a-f]+' | head -1 || true)"
    declared="$(echo "$run" | grep -oE 'sites_declared=[0-9]+' | head -1 | cut -d= -f2 || true)"
    visited="$(echo "$run" | grep -oE 'sites_visited=[0-9]+' | head -1 | cut -d= -f2 || true)"
    iters="$(echo "$run" | grep -oE 'iters=[0-9]+' | head -1 | cut -d= -f2 || true)"
    if [ -z "$seed" ] || [ -z "$declared" ] || [ -z "$visited" ] || [ -z "$iters" ]; then
        echo "  $label: malformed RUN record"
        echo "    $run"
        return 1
    fi

    # ---- condition 3: the vacuity guard ----------------------------------
    if [ "$visited" -lt "$declared" ]; then
        echo "  $label: vacuous run — visited $visited of $declared declared sites"
        echo "    $run"
        return 1
    fi

    TOTAL_ITERS=$((TOTAL_ITERS + iters))
    local degraded
    degraded="$(echo "$run" | grep -oE 'degraded=[01]' | head -1 | cut -d= -f2 || true)"
    if [ "${degraded:-0}" != "0" ]; then
        # Not a gate failure: an ambient run is a weaker measurement, not a wrong
        # one. It is surfaced so a degraded run is never read as a penned one.
        echo "  $label: clean but DEGRADED to ambient ($seed iters=$iters sites=$visited/$declared)"
        echo "    $run"
        TOTAL_ITERS=$((TOTAL_ITERS + iters))
        return 0
    fi
    echo "  $label: clean ($seed iters=$iters sites=$visited/$declared)"
    return 0
}

FAILED_BOOTS=0
for profile in "${PROFILES[@]}"; do
    echo ""
    echo "=== profile $profile — $SEEDS boot(s), component $COMPONENT, mode $MODE ==="
    for index in $(seq 1 "$SEEDS"); do
        boot_once "$profile" "$index"
        TOTAL_BOOTS=$((TOTAL_BOOTS + 1))
        # `adjudicate` returning nonzero is an expected outcome, not a script
        # error, so it is called in a condition rather than under the ERR trap.
        if ! adjudicate "$OUTPUT_ROOT/${profile}_${index}/serial.txt" "$profile#$index"; then
            FAILED_BOOTS=$((FAILED_BOOTS + 1))
        fi
    done
done

echo ""
echo "boots=$TOTAL_BOOTS failed=$FAILED_BOOTS violations=$TOTAL_VIOLATIONS iters_total=$TOTAL_ITERS"
if [ "$FAILED_BOOTS" -ne 0 ]; then
    echo "ARM64 CORE-PROOF GATE: FAILED"
    echo "Serials preserved under $OUTPUT_ROOT"
    exit 1
fi
echo "ARM64 CORE-PROOF GATE: PASSED"
exit 0
