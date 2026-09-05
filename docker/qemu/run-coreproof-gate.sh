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
# FIVE GATE-FAILING CONDITIONS, and they are the whole verdict:
#
#   1. Any VIOLATION line.
#   2. A missing or malformed RUN line — a boot that emitted no run record proves
#      nothing, and "no violations" from a harness that never ran is the exact
#      false green this gate exists to refuse.
#   3. A RUN record whose comp= byte does not match --component. Adjudicating a
#      mis-wired driver under another component's rules is a false green (m5).
#   4. iters=0. A zero-iteration run measured nothing, even if ordinary boot
#      traffic happened to satisfy its whole site census (B1).
#   5. sites_visited < sites_declared. This is the vacuity guard. Both numbers
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
#   docker/qemu/run-coreproof-gate.sh [--component A|C|H] [--seeds N] [--profile max|cortex-a72|both]
#                                     [--mode pen|adversarial|ambient]
#                                     [--window post_cohort|overlap] [--disarm]
#                                     [--require-cov mutation-feature|short-name] [--seed 0xHEX]
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
MODE=""
WINDOW=""
DISARM=0
REQUIRE_COV=""
PINNED_SEED=""
GATE_TARGET_DIR="$BREENIX_ROOT/target/coreproof-gate"
OUTPUT_BASE="/tmp/breenix_coreproof_gate"
STARTED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
RUN_STAMP="$(date -u '+%Y%m%dT%H%M%SZ')"
OUTPUT_ROOT="$OUTPUT_BASE/${RUN_STAMP}_$$"

QEMU_PID=""
CURRENT_SERIAL=""

# report_gate_failure/the ERR trap install moved here, ahead of argument
# parsing and validation (#802/#805 idiom, widened to this gate): each
# usage/validation error below used to be a bare `exit 2` reached before the
# trap existed at all, the same shape #802 found in
# run-x86-prod-profile-boot-test.sh's own preflights. Installing the trap
# first and routing each rejection through `false`/`redden N` instead means
# a verdict line prints for a bad argument the same way it does for a bad
# boot.
report_gate_failure() {
    local status=$?
    local line=$1
    local command=$2
    echo ""
    echo "ARM64 CORE-PROOF GATE: FAILED"
    echo "  at line $line: $command (exit $status)"
    echo "  started=$STARTED_AT ended=$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
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

# Each non-1 exit code below (2 for a usage/validation error) reaches the
# trap via `redden N` rather than a bare `exit N`: `return N` inside a
# function is an ordinary command as far as `set -e`/errtrace are
# concerned, so it fires the ERR trap with $? equal to N, and
# report_gate_failure's own `exit "$status"` re-raise carries it through --
# the trap's re-raise stays the only literal `exit` statement in this
# script.
redden() {
    return "$1"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --component) COMPONENT="$2"; shift 2 ;;
        --seeds) SEEDS="$2"; shift 2 ;;
        --profile) PROFILE="$2"; shift 2 ;;
        --mode) MODE="$2"; shift 2 ;;
        --window) WINDOW="$2"; shift 2 ;;
        --disarm) DISARM=1; shift ;;
        --require-cov) REQUIRE_COV="$2"; shift 2 ;;
        --seed) PINNED_SEED="$2"; shift 2 ;;
        --features) EXTRA_FEATURES="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; redden 2 ;;
    esac
done
EXTRA_FEATURES="${EXTRA_FEATURES:-}"

case "$COMPONENT" in
    A) COMPONENT_FEATURE="coreproof_component_a" ;;
    C) COMPONENT_FEATURE="coreproof_component_c" ;;
    H) COMPONENT_FEATURE="coreproof_component_h" ;;
    *) echo "unknown component: $COMPONENT" >&2; redden 2 ;;
esac

# This script is the SOLE owner of the per-component mode default (rung 2
# review, m1) - Pen for A, Adversarial for C and H, because Pen suppresses the
# conditions those components hunt. `kernel/src/proof/quiesce.rs`'s own
# `Mode::selected()` used to compute a second, duplicate default that was
# dead under this script (which always passes an explicit
# `BREENIX_COREPROOF_MODE`) and has been removed; its compile-time default
# is now plain `Pen` for a build invoked outside this script. An explicit
# `--mode` always wins over this script's own default.
if [ -z "$MODE" ]; then
    case "$COMPONENT" in
        C|H) MODE="adversarial" ;;
        *) MODE="pen" ;;
    esac
fi

case "$MODE" in
    pen|adversarial|ambient) ;;
    *) echo "unknown mode: $MODE" >&2; redden 2 ;;
esac

if [ -z "$WINDOW" ]; then
    if [ "$MODE" = "ambient" ]; then
        WINDOW="overlap"
    else
        WINDOW="post_cohort"
    fi
fi
case "$WINDOW" in
    post_cohort|overlap) ;;
    *) echo "unknown window: $WINDOW" >&2; redden 2 ;;
esac

REQUIRE_COV_SHORT="${REQUIRE_COV#coreproof_mut_}"
if [ -n "$REQUIRE_COV" ] && [ -z "$REQUIRE_COV_SHORT" ]; then
    echo "invalid coverage name: $REQUIRE_COV" >&2
    redden 2
fi
if [ -n "$REQUIRE_COV_SHORT" ]; then
    case "$REQUIRE_COV_SHORT" in
        *[!a-z0-9_]*) echo "invalid coverage name: $REQUIRE_COV" >&2; redden 2 ;;
    esac
    if ! grep -qE "^coreproof_mut_${REQUIRE_COV_SHORT}[[:space:]]*=" \
        "$BREENIX_ROOT/kernel/Cargo.toml"; then
        echo "unknown mutation coverage name: $REQUIRE_COV" >&2
        redden 2
    fi
fi

case "$PROFILE" in
    max) PROFILES=(max) ;;
    cortex-a72) PROFILES=(cortex-a72) ;;
    both) PROFILES=(max cortex-a72) ;;
    *) echo "unknown profile: $PROFILE" >&2; redden 2 ;;
esac

RUN_PREFIX='[COREPROOF:RUN:'
# The harness names each record's phase. `close` is the run's own statement that
# it finished, and it is BOTH the signal to stop the guest and the record the
# gate adjudicates. Counting RUN lines instead would confuse "about to start"
# with "over": the open and settled records both exist before the first
# iteration, so a count of two killed the guest at iteration zero and then read
# that as the result.
RUN_CLOSE_MARKER=':phase=close:'
VIOLATION_PREFIX='[COREPROOF:VIOLATION:'
BOOT_TESTS_PASS_LITERAL='[BOOT_TESTS:PASS]'

# ---------------------------------------------------------------------------
# Build. Its own target dir, so the shared aarch64 kernel other gates boot is
# never replaced by a harness-feature build [[gate-target-fidelity-528]], and
# always the SOFT-FLOAT kernel target — building the userspace target here would
# silently re-arm #528.
# ---------------------------------------------------------------------------
FEATURES="boot_tests,coreproof"
if [ -n "$COMPONENT_FEATURE" ]; then
    FEATURES="$FEATURES,$COMPONENT_FEATURE"
fi
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
BUILD_ENV+=("BREENIX_COREPROOF_WINDOW=$WINDOW")
BUILD_ENV+=("BREENIX_COREPROOF_DISARM=$DISARM")
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

mkdir -p "$OUTPUT_ROOT"
echo "Serial output root: $OUTPUT_ROOT"

TOTAL_BOOTS=0
TOTAL_VIOLATIONS=0
TOTAL_ITERS=0

boot_once() {
    local profile="$1"
    local index="$2"
    local dir="$OUTPUT_ROOT/${profile}_${index}"
    mkdir -p "$dir"
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
            && grep -qaF "$RUN_CLOSE_MARKER" "$CURRENT_SERIAL" 2>/dev/null; then
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

    # Read and report the harness's own evidence BEFORE any ordinary boot
    # verdict can return. A boot-test failure and a core-proof catch can coexist;
    # the former must still fail the boot, but it must not erase the latter from
    # this invocation's counts (round 1's M1 evidence did exactly that).
    local violations run seed declared visited iters comp cov_field cov_value cov_failure
    violations="$(grep -acF "$VIOLATION_PREFIX" "$serial" 2>/dev/null || true)"
    violations="${violations:-0}"
    # Prefer the run's own closing record. Falling back to the last record of any
    # phase is deliberate: a boot that died mid-flight then still adjudicates the
    # furthest record it reached, which reports iters=0 and an incomplete site
    # census and reddens on its own terms rather than as "no RUN record".
    run="$(grep -aF "$RUN_CLOSE_MARKER" "$serial" 2>/dev/null | tail -1 || true)"
    if [ -z "$run" ]; then
        run="$(grep -aF "$RUN_PREFIX" "$serial" 2>/dev/null | tail -1 || true)"
    fi
    iters="$(echo "$run" | grep -oE 'iters=[0-9]+' | head -1 | cut -d= -f2 || true)"
    cov_value=""
    cov_failure=0
    if [ -n "$REQUIRE_COV_SHORT" ]; then
        cov_field="$(echo "$run" | sed -n 's/.*:cov=\([^]]*\)\].*/\1/p')"
        cov_value="$(echo "$cov_field" | tr ',' '\n' \
            | awk -F= -v required="$REQUIRE_COV_SHORT" '$1 == required { print $2; exit }')"
        case "$cov_value" in
            "")
                echo "  $label: RUN record has no cov=$REQUIRE_COV_SHORT entry"
                cov_failure=1
                ;;
            *[!0-9]*)
                echo "  $label: RUN record has malformed cov=$REQUIRE_COV_SHORT=$cov_value"
                cov_failure=1
                ;;
            0)
                echo "  $label: mutation window is vacuous — cov=$REQUIRE_COV_SHORT=0"
                cov_failure=1
                ;;
        esac
    fi
    TOTAL_VIOLATIONS=$((TOTAL_VIOLATIONS + violations))
    if [ -n "$iters" ]; then
        TOTAL_ITERS=$((TOTAL_ITERS + iters))
    fi
    if [ -n "$REQUIRE_COV_SHORT" ]; then
        echo "  $label: harness violations=$violations iters=${iters:-0} cov=$REQUIRE_COV_SHORT=${cov_value:-missing}"
    else
        echo "  $label: harness violations=$violations iters=${iters:-0}"
    fi
    if [ "$violations" -ne 0 ]; then
        grep -aF "$VIOLATION_PREFIX" "$serial" | sed 's/^/    /'
    fi

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
    if [ "$violations" -ne 0 ]; then
        return 1
    fi

    # ---- condition 2: a well-formed closing run record --------------------
    # The harness emits an opening record carrying the seed before the cohort
    # wait, a settled record once its draw domain is authoritative, and a closing
    # record with the achieved counts, so a run that dies mid-flight still has
    # its seed on the wire. The gate adjudicates the `phase=close` record.
    if [ -z "$run" ]; then
        echo "  $label: no RUN record"
        return 1
    fi
    seed="$(echo "$run" | grep -oE 'seed=0x[0-9a-f]+' | head -1 || true)"
    declared="$(echo "$run" | grep -oE 'sites_declared=[0-9]+' | head -1 | cut -d= -f2 || true)"
    visited="$(echo "$run" | grep -oE 'sites_visited=[0-9]+' | head -1 | cut -d= -f2 || true)"
    comp="$(echo "$run" | grep -oE 'comp=[A-Za-z]' | head -1 | cut -d= -f2 || true)"
    if [ -z "$seed" ] || [ -z "$declared" ] || [ -z "$visited" ] || [ -z "$iters" ] || [ -z "$comp" ]; then
        echo "  $label: malformed RUN record"
        echo "    $run"
        return 1
    fi

    # ---- condition: the RUN record's own component must match what was asked for --------
    # A mis-wired feature would otherwise be adjudicated under the wrong driver's rules —
    # `sites_declared` happening to discriminate by coincidence today is not a gate (m5).
    if [ "$comp" != "$COMPONENT" ]; then
        echo "  $label: RUN record reports comp=$comp, expected --component $COMPONENT"
        echo "    $run"
        return 1
    fi

    # ---- condition: a zero-iteration run proves nothing -------------------------------
    # `sites_visited < sites_declared` used to be the only thing standing between "the
    # harness genuinely ran" and "clean by construction" for a component whose census sites
    # were all ordinary-boot-traffic sites — a boot that took the PostCohort cohort-wait
    # timeout and closed with `iterations=0` still reported a full site census and was scored
    # clean (rung 2 review, B1; kernel/src/proof/sites.rs's Component C header now explains
    # why this no longer happens by construction either, via its new driver-only census
    # site — this check is belt-and-suspenders on top of that, independent of which sites any
    # future component happens to declare).
    if [ "$iters" -eq 0 ]; then
        echo "  $label: zero-iteration run — the harness never measured anything"
        echo "    $run"
        return 1
    fi

    # ---- optional mutation-window coverage guard -------------------------
    if [ -n "$REQUIRE_COV_SHORT" ]; then
        if [ "$cov_failure" -ne 0 ]; then
            echo "    $run"
            return 1
        fi
    fi

    # ---- condition 5: the vacuity guard ----------------------------------
    if [ "$visited" -lt "$declared" ]; then
        echo "  $label: vacuous run — visited $visited of $declared declared sites"
        echo "    $run"
        return 1
    fi

    local degraded
    degraded="$(echo "$run" | grep -oE 'degraded=[01]' | head -1 | cut -d= -f2 || true)"
    if [ "${degraded:-0}" != "0" ]; then
        # Not a gate failure: an ambient run is a weaker measurement, not a wrong
        # one. It is surfaced so a degraded run is never read as a penned one.
        echo "  $label: clean but DEGRADED to ambient ($seed iters=$iters sites=$visited/$declared)"
        echo "    $run"
        return 0
    fi
    if [ -n "$REQUIRE_COV_SHORT" ]; then
        echo "  $label: clean ($seed iters=$iters sites=$visited/$declared cov=$REQUIRE_COV_SHORT=$cov_value)"
    else
        echo "  $label: clean ($seed iters=$iters sites=$visited/$declared)"
    fi
    return 0
}

FAILED_BOOTS=0
for profile in "${PROFILES[@]}"; do
    echo ""
    echo "=== profile $profile — $SEEDS boot(s), component $COMPONENT, mode $MODE, window $WINDOW, disarmed $DISARM ==="
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
ENDED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
echo "boots=$TOTAL_BOOTS failed=$FAILED_BOOTS violations=$TOTAL_VIOLATIONS iters_total=$TOTAL_ITERS started=$STARTED_AT ended=$ENDED_AT"
if [ "$FAILED_BOOTS" -ne 0 ]; then
    echo "ARM64 CORE-PROOF GATE: FAILED"
    echo "Serials preserved under $OUTPUT_ROOT"
    false
else
    echo "ARM64 CORE-PROOF GATE: PASSED"
fi
