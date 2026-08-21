#!/bin/bash
# This script IS the #609 arm-A placement oracle. It compares:
#   A. production placement followed by a symmetric pin: --expect lost
#   B. ordinary production placement with peer rescue intact: control leg
# A blind soak at #609's filed ~3% rate is not an oracle; this field-signature
# census exists so the placement result and the instrument control are judged directly.

set -eu

EXPECT=""
BOOTS=10
PROFILE="cortex-a72"
STARVED=false
BOOT_TIMEOUT=45
LABEL="arm"
IOPS=2000

usage() {
    echo "Usage: $0 --expect lost|clean [--boots N] [--profile max|cortex-a72]"
    echo "                                 [--starved] [--timeout S] [--label NAME] [--iops N]"
}

require_value() {
    if [ "$#" -lt 2 ]; then
        echo "Error: $1 requires a value"
        usage
        exit 2
    fi
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --expect)
            require_value "$@"
            EXPECT="$2"
            shift 2
            ;;
        --boots)
            require_value "$@"
            BOOTS="$2"
            shift 2
            ;;
        --profile)
            require_value "$@"
            PROFILE="$2"
            shift 2
            ;;
        --starved)
            STARVED=true
            shift
            ;;
        --timeout)
            require_value "$@"
            BOOT_TIMEOUT="$2"
            shift 2
            ;;
        --label)
            require_value "$@"
            LABEL="$2"
            shift 2
            ;;
        --iops)
            require_value "$@"
            IOPS="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Error: unknown option $1"
            usage
            exit 2
            ;;
    esac
done

case "$EXPECT" in
    lost|clean) ;;
    *) echo "Error: --expect must be lost or clean"; usage; exit 2 ;;
esac

case "$BOOTS" in
    ''|*[!0-9]*) echo "Error: --boots must be a positive integer"; exit 2 ;;
esac
if [ "$BOOTS" -eq 0 ]; then
    echo "Error: --boots must be a positive integer"
    exit 2
fi

case "$PROFILE" in
    max|cortex-a72) ;;
    *) echo "Error: --profile must be max or cortex-a72"; exit 2 ;;
esac

case "$BOOT_TIMEOUT" in
    ''|*[!0-9]*) echo "Error: --timeout must be a positive integer"; exit 2 ;;
esac
if [ "$BOOT_TIMEOUT" -eq 0 ]; then
    echo "Error: --timeout must be a positive integer"
    exit 2
fi

case "$IOPS" in
    ''|*[!0-9]*) echo "Error: --iops must be a non-negative integer"; exit 2 ;;
esac

case "$LABEL" in
    ''|*[!A-Za-z0-9._-]*)
        echo "Error: --label may contain only letters, digits, dot, underscore, and dash"
        exit 2
        ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"

if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found at $KERNEL"
    echo "Build with: cargo build --release --features arm_a_609 --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

"$BREENIX_ROOT/scripts/check-kernel-no-neon.sh" "$KERNEL"

# Durable feature-profile guard, and it is the twin of the #528 guard above: this
# gate pins markers that ONLY a `--features boot_tests` kernel emits, so a kernel
# built in any other profile fails every single boot on "marker missing" and the
# run reads exactly like a kernel regression.
#
# That is not hypothetical, and it is not a mistake anyone makes visibly. `cargo`
# keeps one cached artifact per feature set and hardlinks the requested one into
# this single output path in about 0.06 s with no recompilation and no output
# worth reading. So ANY `cargo test` run in the same session silently replaces
# this binary — `cargo test --test kernel_no_neon_guard` builds the kernel with
# NO features by design — and the very next gate boots the wrong kernel. It was
# found exactly that way: a local acceptance battery ran the structural suites
# and then this gate, and produced 0/6 on the strict gate and 21 consecutive
# "futex handoff oracle marker missing" boots here, all of them against a
# production kernel that had never been asked to emit those markers.
#
# Refusing to boot is the only honest response. Fifty boots of an attributable-
# looking false red is worse than no run at all.
require_boot_tests_kernel() {
    local kernel="$1"
    local marker
    local missing=""

    # A census of boot_tests-only marker literals, not a single sentinel: one
    # marker moving profile would otherwise silently disarm this guard.
    for marker in '[SCHED_STRAND_ORACLE:' '[STRAND_INJECT_ORACLE:' '[FUTEX_HANDOFF_ORACLE:' '[CTX596_ORACLE:' '[BOOT_TESTS:'; do
        if ! grep -aqF "$marker" "$kernel" 2>/dev/null; then
            missing="$missing $marker"
        fi
    done

    if [ -n "$missing" ]; then
        echo "Error: $kernel was not built with --features boot_tests."
        echo "  Missing boot_tests-only marker literal(s):$missing"
        echo "  This gate pins those markers, so every boot would fail on 'marker missing'."
        echo "  Rebuild with:"
        echo "    cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
        echo "  NOTE: any 'cargo test' in this session rebuilds the kernel WITHOUT boot_tests and"
        echo "  silently swaps this binary in a fraction of a second. Build after testing, not before."
        exit 1
    fi
}

require_boot_tests_kernel "$KERNEL"

if ! grep -aqF '[ARMA609:ARMED]' "$KERNEL" 2>/dev/null; then
    echo "Error: this runner requires a kernel built with --features arm_a_609."
    echo "  Missing kernel marker literal: [ARMA609:ARMED]"
    echo "  Rebuild with:"
    echo "    cargo build --release --features arm_a_609 --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
    exit 1
fi

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

RUN_STAMP="$(date -u +%Y%m%dT%H%M%SZ)-$$"
OUTPUT_DIR="/tmp/breenix_arma609_${LABEL}_${RUN_STAMP}"
mkdir -p "$OUTPUT_DIR"
CENSUS_FILE="$OUTPUT_DIR/census.tsv"
printf 'boot\tclass\treason\tarmed\tlega_cpu\tlega_joined\tlegb_joined\tlate\tseconds\tserial-path\n' > "$CENSUS_FILE"

QEMU_PID=""
CURRENT_DISK=""
HOG_PIDS=""
cleanup() {
    local pid

    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    if [ -n "$CURRENT_DISK" ] && [ -f "$CURRENT_DISK" ]; then
        rm -f "$CURRENT_DISK"
    fi
    for pid in $HOG_PIDS; do
        kill "$pid" 2>/dev/null || true
    done
    for pid in $HOG_PIDS; do
        wait "$pid" 2>/dev/null || true
    done
}
trap cleanup EXIT

start_cpu_hogs() {
    local hog

    for hog in 1 2 3 4 5 6 7 8 9 10; do
        nice -n 19 sh -c 'while :; do :; done' &
        HOG_PIDS="$HOG_PIDS $!"
    done
}

launch_qemu() {
    local serial_file="$1"
    local drive_opts="$2"

    if $STARVED; then
        nice -n 19 qemu-system-aarch64 \
            -M virt,gic-version=3 -cpu "$PROFILE" -m 512 -smp 4 \
            -kernel "$KERNEL" \
            -display none -no-reboot \
            -device virtio-gpu-device \
            -device virtio-keyboard-device \
            -device virtio-tablet-device \
            -device virtio-blk-device,drive=ext2 \
            -drive "$drive_opts" \
            -device virtio-net-device,netdev=net0 \
            -netdev user,id=net0 \
            -serial file:"$serial_file" &
    else
        qemu-system-aarch64 \
            -M virt,gic-version=3 -cpu "$PROFILE" -m 512 -smp 4 \
            -kernel "$KERNEL" \
            -display none -no-reboot \
            -device virtio-gpu-device \
            -device virtio-keyboard-device \
            -device virtio-tablet-device \
            -device virtio-blk-device,drive=ext2 \
            -drive "$drive_opts" \
            -device virtio-net-device,netdev=net0 \
            -netdev user,id=net0 \
            -serial file:"$serial_file" &
    fi
    QEMU_PID=$!
}

crash_detected() {
    local serial_file="$1"

    grep -qE '\[(DATA|INSTRUCTION)_ABORT\]|KERNEL PANIC|panic!|soft lockup detected|Unhandled sync exception' \
        "$serial_file" 2>/dev/null
}

# Print the DISTINCT set of EL1 instruction-abort field signatures found in a
# serial, one "far elr esr" per line, sorted and deduplicated. Prints nothing
# when no abort record can be parsed at all.
#
# Both record sources are read and UNIONED rather than ranked, because neither is
# authoritative and they can disagree:
#
#   * the "[INSTRUCTION_ABORT] FAR=... ELR=... ESR=..." header, which a heartbeat
#     line spliced into it can render unparseable;
#   * the "[FATAL_REGS] label=INSTRUCTION_ABORT ... esr=... far=... elr=..." dump,
#     which survives that splice (and vice versa).
#
# Preferring one source over the other silently picks a winner when they differ.
# That is not a theoretical concern: partition arm A's serial-10 carries header
# FAR=0x0 ELR=0x0 (byte-identical to #576's filed signature) and FATAL_REGS
# far=0x0 elr=0x4ba for the SAME fault on the SAME CPU, so a header-first reader
# files a divergent-state abort under #576 — a tolerated bucket — which is the
# exact "new signature invisible by construction" failure this classifier exists
# to remove. Two records of one fault that disagree describe a CPU state that
# changed between them; that is not the filed single-shot signature and the
# caller must not attribute it. Taking the union, and letting the caller require
# a single-element set, makes the disagreement itself disqualifying.
instruction_abort_signatures() {
    local serial_file="$1"

    {
        grep -ahoE '\[INSTRUCTION_ABORT\] FAR=0x[0-9a-f]+ ELR=0x[0-9a-f]+ ESR=0x[0-9a-f]+' \
            "$serial_file" 2>/dev/null \
            | sed -E 's/.*FAR=(0x[0-9a-f]+) ELR=(0x[0-9a-f]+) ESR=(0x[0-9a-f]+).*/\1 \2 \3/'
        grep -ahoE 'label=INSTRUCTION_ABORT[^=]*=[0-9]+ spsr=0x[0-9a-f]+ esr=0x[0-9a-f]+ far=0x[0-9a-f]+ elr=0x[0-9a-f]+' \
            "$serial_file" 2>/dev/null \
            | sed -E 's/.* esr=(0x[0-9a-f]+) far=(0x[0-9a-f]+) elr=(0x[0-9a-f]+).*/\2 \3 \1/'
    } | sort -u
}

CLASS=""
CLASS_REASON=""
LEGA_CPU="-"
LEGA_JOINED="-"
LEGB_JOINED="-"
LATE="-"
classify_serial() {
    local serial_file="$1"
    local crash_line
    local instruction_abort_signature
    local instruction_abort_variants
    local oracle_fail_line
    local last_line
    local lega_line
    local legb_line
    local late_line

    lega_line=$(grep -ahoE '\[ARMA609:LEGA:tid=[0-9]+:cpu=[0-9]+:joined=[01]:waited_ms=[0-9]+:body_ran=[0-9]+\]' \
        "$serial_file" 2>/dev/null | tail -1)
    legb_line=$(grep -ahoE '\[ARMA609:LEGB:tid=[0-9]+:cpu=-1:joined=[01]:waited_ms=[0-9]+:body_ran=[0-9]+\]' \
        "$serial_file" 2>/dev/null | tail -1)
    late_line=$(grep -ahoE '\[ARMA609:LATE:[^]]*\]' "$serial_file" 2>/dev/null | tail -1)

    LEGA_CPU=$(printf '%s\n' "$lega_line" | sed -nE 's/.*:cpu=([0-9]+):joined=.*/\1/p')
    LEGA_JOINED=$(printf '%s\n' "$lega_line" | sed -nE 's/.*:joined=([01]):waited_ms=.*/\1/p')
    LEGB_JOINED=$(printf '%s\n' "$legb_line" | sed -nE 's/.*:joined=([01]):waited_ms=.*/\1/p')
    LATE=$(printf '%s\n' "$late_line" | sed -nE 's/^\[ARMA609:LATE:([^]]*)\]$/\1/p')
    LEGA_CPU=${LEGA_CPU:--}
    LEGA_JOINED=${LEGA_JOINED:--}
    LEGB_JOINED=${LEGB_JOINED:--}
    LATE=${LATE:--}

    if crash_detected "$serial_file"; then
        if grep -qF '[INSTRUCTION_ABORT]' "$serial_file" 2>/dev/null; then
            instruction_abort_signature=$(instruction_abort_signatures "$serial_file")
            instruction_abort_variants=$(printf '%s' "$instruction_abort_signature" | grep -c . || true)
            if [ "$instruction_abort_variants" -eq 1 ] \
                && [ "$instruction_abort_signature" = '0x0 0x0 0x86000005' ]; then
                CLASS="CRASH:576"
                CLASS_REASON="far/elr/esr = $instruction_abort_signature"
            elif [ "$instruction_abort_variants" -eq 0 ]; then
                CLASS="CRASH:UNATTRIBUTED"
                CLASS_REASON="far/elr/esr = <unreadable>"
            elif [ "$instruction_abort_variants" -gt 1 ]; then
                CLASS="CRASH:UNATTRIBUTED"
                CLASS_REASON="far/elr/esr = $(printf '%s' "$instruction_abort_signature" | paste -sd '|' -)"
            else
                CLASS="CRASH:UNATTRIBUTED"
                CLASS_REASON="far/elr/esr = $instruction_abort_signature"
            fi
        else
            crash_line=$(grep -E '\[(DATA|INSTRUCTION)_ABORT\]|KERNEL PANIC|panic!|soft lockup detected|Unhandled sync exception' \
                "$serial_file" 2>/dev/null | head -1 | sed 's/[[:space:]]*$//')
            CLASS="CRASH:UNATTRIBUTED"
            CLASS_REASON="${crash_line:-<unreadable crash line>}"
        fi
        return
    fi

    # This arm exists even though green boots end at [BOOT_TESTS:PASS]: the
    # block EINTR oracle fires later in the service sequence, so green boots
    # normally never reach it, but every stall and every crash-free hang this
    # runner leaves alive to the wall clock does. A #575 oracle failure must
    # never be absorbed by a bucket that is about something else.
    if grep -qF '[BLOCK_EINTR_ORACLE:FAIL' "$serial_file" 2>/dev/null; then
        oracle_fail_line=$(grep -F '[BLOCK_EINTR_ORACLE:FAIL' "$serial_file" 2>/dev/null \
            | head -1 | sed 's/[[:space:]]*$//')
        CLASS="ORACLE_FAIL"
        CLASS_REASON="block EINTR oracle failure: $oracle_fail_line"
        return
    fi

    if [ "$LEGB_JOINED" = "-" ] || [ "$LEGB_JOINED" = "0" ]; then
        CLASS="CONTROL_FAIL"
        if grep -qF '[ARMA609:LEGB:SPAWN_ERROR]' "$serial_file" 2>/dev/null; then
            CLASS_REASON="leg B spawn error"
        elif [ "$LEGB_JOINED" = "0" ]; then
            CLASS_REASON="leg B joined=0"
        else
            CLASS_REASON="[ARMA609:LEGB:] result line absent"
        fi
        return
    fi

    if [ "$LEGA_JOINED" = "0" ]; then
        CLASS="LOST"
        CLASS_REASON="leg B joined=1; leg A joined=0"
        return
    fi

    if [ "$LEGA_JOINED" = "1" ] && grep -qF '[BOOT_TESTS:PASS]' "$serial_file" 2>/dev/null; then
        CLASS="GREEN"
        CLASS_REASON="leg B joined=1; leg A joined=1; [BOOT_TESTS:PASS]"
        return
    fi

    last_line=$(grep -vF '[heartbeat]' "$serial_file" 2>/dev/null \
        | awk 'NF { line = $0 } END { print line }' \
        | sed 's/[[:space:]]*$//')
    CLASS="OTHER"
    CLASS_REASON="${last_line:-<no non-heartbeat serial line>}"
}

COUNT_CONTROL_FAIL=0
COUNT_LOST=0
COUNT_CRASH_576=0
COUNT_CRASH_UNATTRIBUTED=0
COUNT_ORACLE_FAIL=0
COUNT_GREEN=0
COUNT_OTHER=0
COUNT_ARMED=0
NONCONFORMING=""

echo "========================================="
echo "ARM64 #609 arm A — placement oracle"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo "Expectation: $EXPECT"
echo "Boots: $BOOTS"
echo "CPU profile: $PROFILE"
echo "Starved: $STARVED"
echo "Block IOPS throttle: $IOPS"
echo "Per-boot timeout: ${BOOT_TIMEOUT}s"
echo "Output: $OUTPUT_DIR"

if $STARVED; then
    echo "Starting 10 nice -n 19 CPU hogs; QEMU will also run at nice -n 19."
    start_cpu_hogs
fi

for boot in $(seq 1 "$BOOTS"); do
    SERIAL_FILE="$OUTPUT_DIR/serial-$boot.txt"
    WRITABLE_DISK="$OUTPUT_DIR/ext2-writable-$boot.img"
    : > "$SERIAL_FILE"
    cp "$EXT2_DISK" "$WRITABLE_DISK"
    CURRENT_DISK="$WRITABLE_DISK"

    DRIVE_OPTS="if=none,id=ext2,format=raw,file=$WRITABLE_DISK"
    if [ "$IOPS" -ne 0 ]; then
        DRIVE_OPTS="$DRIVE_OPTS,throttling.iops-total=$IOPS"
    fi

    launch_qemu "$SERIAL_FILE" "$DRIVE_OPTS"
    BOOT_END="timeout"
    BOOT_START=$SECONDS
    BOOT_SECONDS=0
    while :; do
        BOOT_SECONDS=$((SECONDS - BOOT_START))
        if [ "$BOOT_SECONDS" -ge "$BOOT_TIMEOUT" ]; then
            kill "$QEMU_PID" 2>/dev/null || true
            break
        fi

        SLEEP_SECONDS=$((BOOT_TIMEOUT - BOOT_SECONDS))
        if [ "$SLEEP_SECONDS" -gt 2 ]; then
            SLEEP_SECONDS=2
        fi
        sleep "$SLEEP_SECONDS"
        BOOT_SECONDS=$((SECONDS - BOOT_START))

        if crash_detected "$SERIAL_FILE"; then
            BOOT_END="early"
            kill "$QEMU_PID" 2>/dev/null || true
            break
        fi
        if grep -qF '[BLOCK_EINTR_ORACLE:FAIL' "$SERIAL_FILE" 2>/dev/null; then
            BOOT_END="early"
            kill "$QEMU_PID" 2>/dev/null || true
            break
        fi
        if grep -qF '[BOOT_TESTS:PASS]' "$SERIAL_FILE" 2>/dev/null; then
            BOOT_END="early"
            kill "$QEMU_PID" 2>/dev/null || true
            break
        fi
        if [ "$BOOT_SECONDS" -ge "$BOOT_TIMEOUT" ]; then
            kill "$QEMU_PID" 2>/dev/null || true
            break
        fi
    done

    set +e
    wait "$QEMU_PID"
    QEMU_STATUS=$?
    set -e
    QEMU_PID=""
    rm -f "$WRITABLE_DISK"
    CURRENT_DISK=""

    classify_serial "$SERIAL_FILE"

    ARMED=false
    if grep -qF '[ARMA609:ARMED]' "$SERIAL_FILE" 2>/dev/null; then
        ARMED=true
        COUNT_ARMED=$((COUNT_ARMED + 1))
    fi

    case "$CLASS" in
        CONTROL_FAIL) COUNT_CONTROL_FAIL=$((COUNT_CONTROL_FAIL + 1)) ;;
        LOST) COUNT_LOST=$((COUNT_LOST + 1)) ;;
        CRASH:576) COUNT_CRASH_576=$((COUNT_CRASH_576 + 1)) ;;
        CRASH:UNATTRIBUTED) COUNT_CRASH_UNATTRIBUTED=$((COUNT_CRASH_UNATTRIBUTED + 1)) ;;
        ORACLE_FAIL) COUNT_ORACLE_FAIL=$((COUNT_ORACLE_FAIL + 1)) ;;
        GREEN) COUNT_GREEN=$((COUNT_GREEN + 1)) ;;
        OTHER) COUNT_OTHER=$((COUNT_OTHER + 1)) ;;
        *) echo "Internal error: unknown class '$CLASS' for $SERIAL_FILE"; exit 1 ;;
    esac

    TSV_REASON=$(printf '%s' "$CLASS_REASON" | tr '\t\r\n' '   ')
    TSV_LATE=$(printf '%s' "$LATE" | tr '\t\r\n' '   ')
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$boot" "$CLASS" "$TSV_REASON" "$ARMED" "$LEGA_CPU" "$LEGA_JOINED" \
        "$LEGB_JOINED" "$TSV_LATE" "$BOOT_SECONDS" "$SERIAL_FILE" >> "$CENSUS_FILE"
    printf '  Boot %s/%s: %s — %s [armed=%s, lega_cpu=%s, lega_joined=%s, legb_joined=%s, late=%s, seconds=%s, end=%s, qemu_status=%s, serial=%s]\n' \
        "$boot" "$BOOTS" "$CLASS" "$CLASS_REASON" "$ARMED" "$LEGA_CPU" "$LEGA_JOINED" \
        "$LEGB_JOINED" "$LATE" "$BOOT_SECONDS" "$BOOT_END" "$QEMU_STATUS" "$SERIAL_FILE"

    NONCONFORMING_BOOT=false
    if [ "$EXPECT" = "lost" ]; then
        if [ "$CLASS" != "LOST" ] || [ "$ARMED" != "true" ]; then
            NONCONFORMING_BOOT=true
        fi
    else
        if [ "$ARMED" != "true" ]; then
            NONCONFORMING_BOOT=true
        else
            case "$CLASS" in
                CONTROL_FAIL|LOST|CRASH:UNATTRIBUTED|ORACLE_FAIL|OTHER) NONCONFORMING_BOOT=true ;;
            esac
        fi
    fi
    if $NONCONFORMING_BOOT; then
        NONCONFORMING="$NONCONFORMING
  Boot $boot: $CLASS (armed=$ARMED) — $SERIAL_FILE"
    fi
done

COUNT_CRASH=$((COUNT_CRASH_576 + COUNT_CRASH_UNATTRIBUTED))
CENSUS_SUM=$((COUNT_CONTROL_FAIL + COUNT_LOST + COUNT_CRASH + COUNT_ORACLE_FAIL + COUNT_GREEN + COUNT_OTHER))
if [ "$CENSUS_SUM" -ne "$BOOTS" ]; then
    echo "FATAL: class census sums to $CENSUS_SUM, expected $BOOTS"
    echo "Output directory: $OUTPUT_DIR"
    exit 1
fi

LOST_RATE=$(awk -v lost="$COUNT_LOST" -v boots="$BOOTS" \
    'BEGIN { printf "%.1f", (boots == 0 ? 0 : lost * 100 / boots) }')

echo ""
echo "#609 arm-A placement oracle census"
printf '  %-22s %d\n' "CONTROL_FAIL" "$COUNT_CONTROL_FAIL"
printf '  %-22s %d\n' "LOST" "$COUNT_LOST"
printf '  %-22s %d\n' "CRASH:576" "$COUNT_CRASH_576"
printf '  %-22s %d\n' "CRASH:UNATTRIBUTED" "$COUNT_CRASH_UNATTRIBUTED"
printf '  %-22s %d\n' "CRASH total" "$COUNT_CRASH"
printf '  %-22s %d\n' "ORACLE_FAIL" "$COUNT_ORACLE_FAIL"
printf '  %-22s %d\n' "GREEN" "$COUNT_GREEN"
printf '  %-22s %d\n' "OTHER" "$COUNT_OTHER"
printf '  %-22s %s/%s\n' "armed" "$COUNT_ARMED" "$BOOTS"
echo "  Observed lost rate: $COUNT_LOST/$BOOTS ($LOST_RATE%)"

VERDICT=FAIL
if [ "$EXPECT" = "lost" ]; then
    if [ "$COUNT_LOST" -eq "$BOOTS" ] \
        && [ "$COUNT_ARMED" -eq "$BOOTS" ] \
        && [ "$COUNT_CONTROL_FAIL" -eq 0 ] \
        && [ "$COUNT_CRASH" -eq 0 ] \
        && [ "$COUNT_ORACLE_FAIL" -eq 0 ] \
        && [ "$COUNT_OTHER" -eq 0 ]; then
        VERDICT=PASS
    fi
    echo "Expectation lost requires LOST=boots, armed=boots, CONTROL_FAIL=0, CRASH=0, ORACLE_FAIL=0, and OTHER=0."
else
    if [ "$COUNT_GREEN" -eq "$BOOTS" ] \
        && [ "$COUNT_ARMED" -eq "$BOOTS" ] \
        && [ "$COUNT_LOST" -eq 0 ] \
        && [ "$COUNT_CONTROL_FAIL" -eq 0 ] \
        && [ "$COUNT_CRASH_UNATTRIBUTED" -eq 0 ] \
        && [ "$COUNT_ORACLE_FAIL" -eq 0 ] \
        && [ "$COUNT_OTHER" -eq 0 ]; then
        VERDICT=PASS
    fi
    echo "Expectation clean requires GREEN=boots, armed=boots, LOST=0, CONTROL_FAIL=0, CRASH:UNATTRIBUTED=0, ORACLE_FAIL=0, and OTHER=0."
    echo "CRASH:576 is reported and tolerated as the pre-adjudicated EL1 NULL-PC signature (FAR=0x0 ELR=0x0 ESR=0x86000005)."
fi

echo "Verdict: $VERDICT"
if [ "$VERDICT" = "FAIL" ]; then
    echo "Non-conforming boots:$NONCONFORMING"
fi
echo "Output directory: $OUTPUT_DIR"

if [ "$VERDICT" = "PASS" ]; then
    exit 0
fi
exit 1
