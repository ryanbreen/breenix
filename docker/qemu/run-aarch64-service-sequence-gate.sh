#!/bin/bash
# ARM64 init service-sequence soak gate for #575.
#
# The round gate for #575 is a 100-cycle run of this script per CPU profile
# (`--boots 100`). The DEFAULT is 25 boots per profile — operator directive,
# 2026-08-18 — so an unqualified local run is already a meaningful sample
# rather than the old 10-boot smoke.
# Keep the observation window well above the ~11 s service-sequence completion
# point so a wedged boot is unambiguously distinguishable from a slow one.
#
# This script is the truth about which buckets it classifies and which of them
# fail the gate; read the classify_serial function and the gate condition below
# rather than any external list, which goes stale.

set -e

BOOTS=25
PROFILE=both
IOPS=2000
BOOT_TIMEOUT=45
REBUILD=false

usage() {
    echo "Usage: $0 [--boots N] [--profile max|cortex-a72|both] [--iops N] [--timeout S] [--rebuild]"
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
        --iops)
            require_value "$@"
            IOPS="$2"
            shift 2
            ;;
        --timeout)
            require_value "$@"
            BOOT_TIMEOUT="$2"
            shift 2
            ;;
        --rebuild)
            REBUILD=true
            shift
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

case "$BOOTS" in
    ''|*[!0-9]*) echo "Error: --boots must be a positive integer"; exit 2 ;;
esac
if [ "$BOOTS" -eq 0 ]; then
    echo "Error: --boots must be a positive integer"
    exit 2
fi

case "$IOPS" in
    ''|*[!0-9]*) echo "Error: --iops must be a non-negative integer"; exit 2 ;;
esac

case "$BOOT_TIMEOUT" in
    ''|*[!0-9]*) echo "Error: --timeout must be a positive integer"; exit 2 ;;
esac
if [ "$BOOT_TIMEOUT" -eq 0 ]; then
    echo "Error: --timeout must be a positive integer"
    exit 2
fi

case "$PROFILE" in
    max|cortex-a72|both) ;;
    *) echo "Error: --profile must be max, cortex-a72, or both"; exit 2 ;;
esac

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# driven=2 proves both handoff seams ran; stage1/2 return, wake, and park fields
# expose D1/D2. stage3_elapsed_ok=1 proves no early timeout return, while
# stage3_ret=ETIMEDOUT plus rescues=0 proves the backstop did not end this wait.
# stage3_elapsed_ms is the measured duration; residual/balance prove cleanup.
# This marker is emitted from a syscall while the scheduler trace stream is live, so its line can carry a prefix.
FUTEX_HANDOFF_ORACLE_PATTERN='\[FUTEX_HANDOFF_ORACLE:aarch64:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=[0-9]+:rescues=0:queue_residual=0:balance=0\]'
CENSUS_WIDEN_ORACLE_PATTERN='\[CENSUS_WIDEN_ORACLE:aarch64:arm_target=[0-9]+:baseline_reported=0:armed_reported=1:tid=[1-9][0-9]*:shape=ready_queued_nondispatching:queued_nondispatching=[1-9][0-9]*:queued_nondispatch_ms=[1-9][0-9]*:cpu_silence_ms=[1-9][0-9]*:joined=1:retired=[01]:PASS\]'

if $REBUILD; then
    echo "Building ARM64 kernel with boot_tests feature..."
    (cd "$BREENIX_ROOT" && cargo build --release --features boot_tests \
        --target aarch64-breenix-kernel.json \
        -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
        -p kernel --bin kernel-aarch64 2>&1)
    echo "Build complete."
    echo ""
fi

KERNEL="$BREENIX_ROOT/target/aarch64-breenix-kernel/release/kernel-aarch64"
if [ ! -f "$KERNEL" ]; then
    echo "Error: No ARM64 kernel found at $KERNEL"
    echo "Build with: cargo build --release --features boot_tests --target aarch64-breenix-kernel.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64"
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
#
# R37 landmine (2026-08-22): the validation below happens only once at startup.
# In a fraction of a second, cargo hardlinks the artifact for the requested
# feature set onto the one target/aarch64-breenix-kernel/release/kernel-aarch64
# path. Thus somebody starting cargo build or cargo test during this gate can
# replace the kernel between boots without another guard check. That race cost
# r2 14 false CONTROL_FAIL boots before discovery. Do not build while any gate
# is booting; when failures suggest the kernel changed shape, check the binary
# mtime against this run's start time before treating the evidence as real.
require_boot_tests_kernel() {
    local kernel="$1"
    local marker
    local missing=""

    # A census of boot_tests-only marker literals, not a single sentinel: one
    # marker moving profile would otherwise silently disarm this guard.
    for marker in '[SCHED_STRAND_ORACLE:' '[STRAND_INJECT_ORACLE:' '[CENSUS_WIDEN_ORACLE:' '[FUTEX_HANDOFF_ORACLE:' '[CTX596_ORACLE:' '[BOOT_TESTS:'; do
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

EXT2_DISK="$BREENIX_ROOT/target/ext2-aarch64.img"
if [ ! -f "$EXT2_DISK" ]; then
    echo "Error: ext2 disk not found at $EXT2_DISK"
    exit 1
fi

RUN_STAMP="$(date -u +%Y%m%dT%H%M%SZ)-$$"
OUTPUT_DIR="${OUTPUT_DIR:-/tmp/breenix_aarch64_service_sequence_gate_$RUN_STAMP}"
mkdir -p "$OUTPUT_DIR"

QEMU_PID=""
CURRENT_DISK=""
cleanup() {
    if [ -n "$QEMU_PID" ]; then
        kill "$QEMU_PID" 2>/dev/null || true
        wait "$QEMU_PID" 2>/dev/null || true
    fi
    if [ -n "$CURRENT_DISK" ] && [ -f "$CURRENT_DISK" ]; then
        rm -f "$CURRENT_DISK"
    fi
}
trap cleanup EXIT

CLASS_BUCKET=""
CLASS_REASON=""

silent_spawn_reason() {
    local serial_file="$1"
    local spawn_record
    local line_number
    local spawn_line
    local path
    local name
    local next_line
    local last_line
    local segment
    local entry_segment
    local heartbeat_count

    grep -nF "[spawn] path='" "$serial_file" 2>/dev/null | while IFS= read -r spawn_record; do
        line_number=${spawn_record%%:*}
        spawn_line=${spawn_record#*:}
        path=$(echo "$spawn_line" | sed -n "s/.*\[spawn\] path='\([^']*\)'.*/\1/p")
        [ -n "$path" ] || continue
        name=${path##*/}

        next_line=$(awk -v start="$line_number" \
            'NR > start && index($0, "[spawn] path=") { print NR; exit }' "$serial_file")
        if [ -n "$next_line" ]; then
            last_line=$((next_line - 1))
        else
            last_line=$(awk 'END { print NR }' "$serial_file")
        fi
        segment=$(sed -n "${line_number},${last_line}p" "$serial_file")
        entry_segment=$(sed -n "${line_number},\$p" "$serial_file")

        if echo "$entry_segment" | grep -qF "create_process_with_argv [ARM64]: ENTRY - name='$name'"; then
            continue
        fi
        if echo "$segment" | grep -qE "\[spawn\] Failed|\[init\].*(failed|Failed)"; then
            continue
        fi

        heartbeat_count=$(awk -v start="$line_number" \
            'NR > start && index($0, "[heartbeat]") { count++ } END { print count + 0 }' \
            "$serial_file")
        # Five heartbeats prove the kernel stayed alive; a shorter tail may be timeout truncation.
        if [ "$heartbeat_count" -lt 5 ]; then
            continue
        fi

        echo "spawn never returned: path='$path'"
        return 0
    done || true
}

green_sequence_complete() {
    local serial_file="$1"
    local bounce_line
    local heartbeat_count

    grep -qF "[init] Boot script completed" "$serial_file" 2>/dev/null \
        && grep -qF "create_process_with_argv [ARM64]: ENTRY - name='telnetd'" "$serial_file" 2>/dev/null \
        && grep -qF "[spawn] path='/bin/bounce'" "$serial_file" 2>/dev/null || return 1

    bounce_line=$(grep -nF "[spawn] path='/bin/bounce'" "$serial_file" | head -1 | cut -d: -f1)
    [ -n "$bounce_line" ] || return 1
    heartbeat_count=$(awk -v start="$bounce_line" \
        'NR > start && index($0, "[heartbeat]") { count++ } END { print count + 0 }' \
        "$serial_file")
    [ "$heartbeat_count" -ge 5 ]
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
# files a divergent-state abort under #576 — a named bucket — which is the
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

# Same shape as instruction_abort_signatures() above, but for EL1
# ([DATA_ABORT] ... from_el0=0) data aborts: prints the distinct set of
# "far esr" field signatures (union of the [DATA_ABORT] header and the
# [FATAL_REGS] dump, deduplicated) so the caller can require a single,
# field-exact match rather than treating every EL1 data abort as one bucket.
data_abort_signatures() {
    local serial_file="$1"

    {
        grep -ahoE '\[DATA_ABORT\] FAR=0x[0-9a-f]+ ELR=0x[0-9a-f]+ ESR=0x[0-9a-f]+[^[:alnum:]].*from_el0=0' \
            "$serial_file" 2>/dev/null \
            | sed -E 's/.*FAR=(0x[0-9a-f]+) ELR=0x[0-9a-f]+ ESR=(0x[0-9a-f]+).*/\1 \2/'
        grep -ahoE 'label=DATA_ABORT[^=]*=[0-9]+ spsr=0x[0-9a-f]+ esr=0x[0-9a-f]+ far=0x[0-9a-f]+ elr=0x[0-9a-f]+' \
            "$serial_file" 2>/dev/null \
            | sed -E 's/.* esr=(0x[0-9a-f]+) far=(0x[0-9a-f]+) elr=0x[0-9a-f]+.*/\2 \1/'
    } | sort -u
}

# The ELR half of an EL1 data abort's field signature, kept separate so the
# "far esr" signature above — which #612 and #622 are both filed against — is
# not redefined. Same two record sources, same deduplication: a caller must
# require a single element, because two records that disagree about where the
# fault was taken are not one filed signature.
data_abort_elrs() {
    local serial_file="$1"

    {
        grep -ahoE '\[DATA_ABORT\] FAR=0x[0-9a-f]+ ELR=0x[0-9a-f]+ ESR=0x[0-9a-f]+[^[:alnum:]].*from_el0=0' \
            "$serial_file" 2>/dev/null \
            | sed -E 's/.*FAR=0x[0-9a-f]+ ELR=(0x[0-9a-f]+) ESR=0x[0-9a-f]+.*/\1/'
        grep -ahoE 'label=DATA_ABORT[^=]*=[0-9]+ spsr=0x[0-9a-f]+ esr=0x[0-9a-f]+ far=0x[0-9a-f]+ elr=0x[0-9a-f]+' \
            "$serial_file" 2>/dev/null \
            | sed -E 's/.* esr=0x[0-9a-f]+ far=0x[0-9a-f]+ elr=(0x[0-9a-f]+).*/\1/'
    } | sort -u
}

# Print the DISTINCT set of PC-alignment field signatures found in a serial,
# one "elr far from_el0" triple per line. A caller must require a single element:
# multiple records that disagree are not one filed fault signature.
pc_align_signatures() {
    local serial_file="$1"

    {
        grep -ahoE '\[PC_ALIGN\] ELR=0x[0-9a-f]+ FAR=0x[0-9a-f]+ from_el0=[01]' \
            "$serial_file" 2>/dev/null \
            | sed -E 's/.*ELR=(0x[0-9a-f]+) FAR=(0x[0-9a-f]+) from_el0=([01]).*/\1 \2 \3/'
    } | sort -u
}

# Print the DISTINCT set of complete panic field signatures. PanicInfo emits
# the location after "panicked at" and the message on the following line; keep
# both field values verbatim so classifier reasons preserve the actual failure.
kernel_panic_signatures() {
    local serial_file="$1"

    awk '
        /panicked at / {
            location = $0
            sub(/^.*panicked at /, "", location)
            sub(/\r$/, "", location)
            if ((getline message) > 0) {
                sub(/\r$/, "", message)
                if (length(location) != 0 && length(message) != 0) {
                    print "location=" location " message=" message
                }
            }
        }
    ' "$serial_file" | sort -u
}

# Return 0 when EarlyBoot advanced but never completed while the kernel stayed
# alive and the scheduler oracle continued sampling. This is the widened #609
# stage-boundary signature: it includes wedges that occur before any particular
# early subsystem completes. It remains a hard FAIL and is deliberately ordered
# after crash and oracle-failure attribution in classify_serial.
is_609_early_boot_stage_stall() {
    local serial_file="$1"

    grep -qF "[STAGE:early:ADVANCE]" "$serial_file" 2>/dev/null || return 1
    if grep -qF "[STAGE:early:COMPLETE" "$serial_file" 2>/dev/null; then
        return 1
    fi
    if grep -qF "[TESTS_COMPLETE:" "$serial_file" 2>/dev/null; then
        return 1
    fi
    if grep -qiE '\[(DATA|INSTRUCTION)_ABORT\]|KERNEL PANIC|panic!|soft lockup detected|Unhandled sync exception' \
        "$serial_file" 2>/dev/null; then
        return 1
    fi
    grep -qE '\[SCHED_STRAND_ORACLE:aarch64:samples=[1-9][0-9][0-9][0-9]*:checked=[1-9][0-9]*:stranded=0:' \
        "$serial_file" 2>/dev/null
}

# Consult filed signatures first: a boot that died from a filed defect before init ran
# cannot be blamed on a missing marker; the unattributed bucket is for reds nobody has filed.
classify_serial() {
    local serial_file="$1"
    local data_abort_line
    local silent_reason
    local last_line
    local quiesce_rows
    local quiesce_rows_floor
    local quiesce_walk_line
    local stranded_strand_line
    local instruction_abort_signature
    local instruction_abort_variants
    local instruction_abort_far
    local instruction_abort_elr
    local instruction_abort_esr
    local data_abort_signature
    local data_abort_variants
    local data_abort_elr
    local data_abort_elr_variants
    local boot_test_fail_line
    local pc_align_signature
    local pc_align_variants
    local pc_align_line
    local kernel_panic_signature
    local kernel_panic_variants
    local kernel_panic_marker
    local kernel_panic_location
    local kernel_panic_message

    # #596's runtime oracle is unconditional: an inline-saved context whose
    # recorded resume PC is not its inline-save x30 is a defect no matter what
    # else the boot did, so it is consulted before every other signature.
    if grep -qF "[CTX596_ORACLE:FAIL" "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="596"
        CLASS_REASON="inline-save resume-point oracle failed: $(grep -o '\[CTX596_ORACLE:FAIL:[a-z_]*' "$serial_file" | head -1)"
        return
    fi
    # EL1 (from_el0=0) data aborts are attributed BY FIELD SIGNATURE, never by
    # exception type (ruling R34; this arm used to be a catch-all for ANY
    # [DATA_ABORT] ... from_el0=0 line, bucketed as #612 regardless of FAR/ESR —
    # exactly the "new signature invisible by construction" failure the
    # instruction-abort arm below was already built to avoid for #576. #622 is
    # the proof: FAR=0x200 ESR=0x96000005 rode into the #612 bucket, which is
    # filed at the field-exact FAR=0x292 ESR=0x96000021. #596 is handled above
    # and is excluded from this arm entirely (its own oracle already returned).
    #
    # The match is on the WHOLE SET of records the serial carries, matching
    # instruction_abort_signatures()'s discipline above: a serial carrying two
    # different signatures, or one fault whose two records disagree, is
    # UNATTRIBUTED rather than guessed at — even #612, the one bucket below
    # that is a named FAIL rather than UNATTRIBUTED, only earns that name on an
    # unambiguous single-signature match.
    if grep -qF "[DATA_ABORT]" "$serial_file" 2>/dev/null && grep -qE "\[DATA_ABORT\].*from_el0=0" "$serial_file" 2>/dev/null; then
        data_abort_signature=$(data_abort_signatures "$serial_file")
        data_abort_variants=$(printf '%s' "$data_abort_signature" | grep -c . || true)
        if [ "$data_abort_variants" -eq 0 ]; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="EL1 data abort whose FAR/ESR fields could not be read from the serial"
        elif [ "$data_abort_variants" -gt 1 ]; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="EL1 data abort records disagree, so no single signature describes this boot: far/esr = $(printf '%s' "$data_abort_signature" | paste -sd '|' -)"
        elif [ "$data_abort_signature" = "0x292 0x96000021" ]; then
            CLASS_BUCKET="612"
            CLASS_REASON="EL1 data abort matching the filed #612 signature (FAR=0x292 ESR=0x96000021)"
        elif [ "$data_abort_signature" = "0x200 0x96000005" ]; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="EL1 data abort matching the filed #622 signature (FAR=0x200 ESR=0x96000005) — not #612, and not tolerated"
        elif [ "$data_abort_signature" = "0x2 0x96000005" ]; then
            # #641, ATTRIBUTION ONLY (coordinator ruling R49). This bucket does
            # not change what the gate does with the boot: 641 is in the
            # per-profile FAIL condition below, exactly as the UNATTRIBUTED
            # verdict it replaces was. It is NOT a tolerance and must never
            # become one — the only thing it buys is that a recurrence is
            # reported under the open issue it belongs to instead of as an
            # unfiled red.
            #
            # #641 is filed at FAR=0x2, ESR=0x96000005 (DFSC is the low six bits
            # of that ESR, 0x5, so a field-exact ESR match is a DFSC match by
            # construction), from_el0=0 — which this arm's guard already
            # requires — and an ELR in kernel text. The ELR is checked here, on
            # the whole record set, rather than folded into the "far esr"
            # signature #612 and #622 are filed against: a boot whose FAR/ESR
            # match but whose ELR is not one kernel-text address is a DIFFERENT
            # signature and stays UNATTRIBUTED.
            data_abort_elr=$(data_abort_elrs "$serial_file")
            data_abort_elr_variants=$(printf '%s' "$data_abort_elr" | grep -c . || true)
            if [ "$data_abort_elr_variants" -eq 1 ] && [[ "$data_abort_elr" =~ ^0xffff[0-9a-f]+$ ]]; then
                CLASS_BUCKET="641"
                CLASS_REASON="EL1 data abort matching the filed #641 signature (FAR=0x2 ESR=0x96000005 DFSC=0x5 from_el0=0, ELR=$data_abort_elr in kernel text) — ATTRIBUTED, and gate-failing exactly as the UNATTRIBUTED verdict it replaces"
            else
                CLASS_BUCKET="UNATTRIBUTED"
                CLASS_REASON="EL1 data abort carrying #641's FAR/ESR (0x2 0x96000005) without #641's single kernel-text ELR: elr = $(printf '%s' "$data_abort_elr" | paste -sd '|' -)"
            fi
        else
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="EL1 data abort matches no filed signature: far/esr = $data_abort_signature"
        fi
        return
    fi
    if grep -qF "[BLOCK_EINTR_ORACLE:FAIL" "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="575"
        CLASS_REASON="block EINTR oracle reported failure"
        return
    fi
    if grep -qF "failed to spawn service: EIO" "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="575"
        CLASS_REASON="service spawn returned EIO"
        return
    fi
    silent_reason=$(silent_spawn_reason "$serial_file")
    if [ -n "$silent_reason" ]; then
        CLASS_BUCKET="575"
        CLASS_REASON="$silent_reason"
        return
    fi
    # Instruction aborts are attributed BY FIELD SIGNATURE, never by exception
    # type. #576 is filed as exactly FAR=0x0 ELR=0x0 ESR=0x86000005, and #626
    # is filed separately as FAR=0x0 ELR=0x0 ESR=0x8600000d. Bucketing every
    # [INSTRUCTION_ABORT] as 576 made different signatures invisible by
    # construction. Anything that does not match one of these filed signatures
    # — or whose fields cannot be read at all — is UNATTRIBUTED, which this gate
    # FAILS on. Both named buckets are also gate failures.
    #
    # The match is on the WHOLE SET of records the serial carries, not on a
    # first-found record: a boot earns a named bucket only when every abort
    # record it emitted names one and the same filed signature. A serial
    # carrying two different signatures, or one fault whose two records disagree,
    # is UNATTRIBUTED — otherwise a novel signature could ride in behind a
    # matching one and the named bucket would hide it again.
    if grep -qF "[INSTRUCTION_ABORT]" "$serial_file" 2>/dev/null; then
        instruction_abort_signature=$(instruction_abort_signatures "$serial_file")
        instruction_abort_variants=$(printf '%s' "$instruction_abort_signature" | grep -c . || true)
        if [ "$instruction_abort_variants" -eq 0 ]; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="instruction abort whose FAR/ELR/ESR fields could not be read from the serial"
        elif [ "$instruction_abort_variants" -gt 1 ]; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="instruction abort records disagree, so no single signature describes this boot: far/elr/esr = $(printf '%s' "$instruction_abort_signature" | paste -sd '|' -)"
        elif [ "$instruction_abort_signature" = "0x0 0x0 0x86000005" ]; then
            CLASS_BUCKET="576"
            CLASS_REASON="instruction abort matching the filed #576 signature (FAR=0x0 ELR=0x0 ESR=0x86000005)"
        elif [ "$instruction_abort_signature" = "0x0 0x0 0x8600000d" ]; then
            CLASS_BUCKET="626"
            CLASS_REASON="instruction abort matching the filed #626 signature (FAR=0x0 ELR=0x0 ESR=0x8600000d)"
        elif instruction_abort_far=$(printf '%s' "$instruction_abort_signature" | cut -d' ' -f1) \
            && instruction_abort_elr=$(printf '%s' "$instruction_abort_signature" | cut -d' ' -f2) \
            && instruction_abort_esr=$(printf '%s' "$instruction_abort_signature" | cut -d' ' -f3) \
            && [ "$instruction_abort_far" = "$instruction_abort_elr" ] \
            && [ "$instruction_abort_far" != "0x0" ] \
            && [ "$instruction_abort_esr" = "0x8600000e" ] \
            && [[ "$instruction_abort_far" =~ ^0xffff[0-9a-f]+$ ]]; then
            # #635's field-keyed family: ESR=0x8600000e with FAR == ELR (the
            # SAME hex string in both positions, never 0x0 — #576 and #626
            # above already claimed FAR=ELR=0x0) and a canonical kernel
            # high-half address.
            #
            # The predicate above is unchanged and stays: attribution by field
            # signature is what keeps an occurrence of this shape reported
            # under the issue it belongs to rather than falling into
            # UNATTRIBUTED. Attribution is not a tolerance.
            #
            # The tolerance is gone. The non-failing exemption this bucket
            # carried was authorized only for as long as the producer was
            # unfixed; that producer is repaired at source on this branch —
            # per-CPU idle/exception stacks name their owner and the per-CPU
            # stack-top setters refuse an address belonging to another CPU,
            # thread id 0 is retired, the return-SP install follows the frame's
            # pending exception level, and the reclaimed-thread drop runs with
            # interrupts masked (docs/planning/t3g-prb/). count_635 is
            # therefore in run_profile's FAIL condition below, exactly like
            # every other named bucket, and one occurrence fails the profile it
            # happened in. Removing the tolerance is a tightening: the set of
            # runs this gate passes is strictly smaller than before.
            CLASS_BUCKET="635"
            CLASS_REASON="instruction abort matching the #635 kernel-stack-PC family (FAR=ELR=$instruction_abort_far ESR=0x8600000e) — ATTRIBUTED by field signature, and gate-failing"
        else
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="instruction abort matches no filed signature: far/elr/esr = $instruction_abort_signature"
        fi
        return
    fi
    if grep -qF "[DATA_ABORT]" "$serial_file" 2>/dev/null; then
        data_abort_line=$(grep -F "[DATA_ABORT]" "$serial_file" 2>/dev/null | head -1 | sed 's/[[:space:]]*$//')
        CLASS_BUCKET="DATA_ABORT"
        CLASS_REASON="EL1 data abort (#596): ${data_abort_line}"
        return
    fi
    # #589 is closed. A non-failing bucket keyed to a closed issue hides live reds;
    # both a strand census/injection report of stranded work and a live sibling
    # refusing exec are real defects that whoever sees them must file, not absorb.
    # The serial console emits CRLF; trim trailing whitespace before exact comparisons and reports.
    last_line=$(grep -F "CLONEVM_EXEC_TEST" "$serial_file" 2>/dev/null | tail -1 | sed 's/[[:space:]]*$//' || true)
    if [ "$last_line" = "CLONEVM_EXEC_TEST: live sibling refused exec" ]; then
        CLASS_BUCKET="CLONE_EXEC"
        CLASS_REASON="live sibling refused exec"
        return
    fi
    # A crashed boot's cleanup can strand a thread; attribute strands only if no crash came first.
    stranded_strand_line=$(grep -E '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' \
        "$serial_file" 2>/dev/null | tail -1 || true)
    if [ -n "$stranded_strand_line" ]; then
        CLASS_BUCKET="STRAND"
        CLASS_REASON="scheduler strand census reported stranded work: $stranded_strand_line"
        return
    fi
    if grep -qE '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' \
        "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="STRAND"
        CLASS_REASON="strand injection oracle reported stranded work: $(grep -E '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$serial_file" | tail -1)"
        return
    fi
    if grep -qE '\[CENSUS_WIDEN_ORACLE:[^]]*:FAIL\]' "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="STRAND"
        CLASS_REASON="census widening mutation oracle failed: $(grep -E '\[CENSUS_WIDEN_ORACLE:[^]]*:FAIL\]' "$serial_file" | tail -1)"
        return
    fi
    # Preserve prior crash and oracle attribution above. A remaining aggregate
    # boot-test failure gets its own hard-failing bucket and field signature;
    # it must never fall through to a generic missing-marker classification.
    if grep -qF "[BOOT_TESTS:FAIL" "$serial_file" 2>/dev/null \
        || grep -qE '\[TESTS_COMPLETE:[^]]*:FAILED:[1-9][0-9]*\]' "$serial_file" 2>/dev/null; then
        boot_test_fail_line=$(grep -ahoE '\[TEST:[^]]*:FAIL:[^]]*\]' \
            "$serial_file" 2>/dev/null | head -1 || true)
        CLASS_BUCKET="BOOT_TEST_FAIL"
        CLASS_REASON="boot test failure: ${boot_test_fail_line:-[TEST:<missing>:FAIL:<missing>]}"
        return
    fi
    # Preserve every existing named bucket above. A PC-alignment fault that
    # remains is attributed by the complete set of ELR/FAR/from_el0 triples,
    # never by exception type alone. #625 is filed at exactly 0x4b5/0x5/EL0;
    # it stays UNATTRIBUTED and therefore hard-failing rather than tolerated.
    if grep -qF "[PC_ALIGN]" "$serial_file" 2>/dev/null; then
        pc_align_signature=$(pc_align_signatures "$serial_file")
        pc_align_variants=$(printf '%s' "$pc_align_signature" | grep -c . || true)
        if [ "$pc_align_variants" -eq 0 ]; then
            pc_align_line=$(grep -F "[PC_ALIGN]" "$serial_file" 2>/dev/null | head -1 | sed 's/[[:space:]]*$//' || true)
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="PC alignment fault with an unreadable ELR/FAR/from_el0 signature: ${pc_align_line:-[PC_ALIGN] <fields missing>}"
        elif [ "$pc_align_variants" -gt 1 ]; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="PC alignment records disagree, so no single signature describes this boot: elr/far/from_el0 = $(printf '%s' "$pc_align_signature" | paste -sd '|' -)"
        elif [ "$pc_align_signature" = "0x4b5 0x5 1" ]; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="PC alignment fault matching filed #625 signature (ELR=0x4b5 FAR=0x5 from_el0=1) — not tolerated"
        else
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="PC alignment fault matches no filed signature: elr/far/from_el0 = $pc_align_signature"
        fi
        return
    fi
    # A panic is likewise keyed to its own location/message fields. This arm is
    # deliberately specific to KERNEL PANIC and always hard-fails. If a complete
    # signature is unavailable, report the individual evidence that was readable
    # instead of falling through to a generic missing-marker reason.
    if grep -qF "KERNEL PANIC" "$serial_file" 2>/dev/null; then
        kernel_panic_signature=$(kernel_panic_signatures "$serial_file")
        kernel_panic_variants=$(printf '%s' "$kernel_panic_signature" | grep -c . || true)
        if [ "$kernel_panic_variants" -eq 0 ]; then
            kernel_panic_marker=$(grep -F "KERNEL PANIC" "$serial_file" 2>/dev/null | head -1 | sed 's/[[:space:]]*$//' || true)
            kernel_panic_location=$(grep -F "panicked at " "$serial_file" 2>/dev/null | head -1 | sed -E 's/.*panicked at //' | sed 's/[[:space:]]*$//' || true)
            kernel_panic_message=$(awk '/panicked at / { if ((getline line) > 0) { sub(/\r$/, "", line); print line }; exit }' "$serial_file" 2>/dev/null || true)
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="kernel panic field signature unreadable: location=${kernel_panic_location:-<missing>}; message=${kernel_panic_message:-<missing>}; marker=${kernel_panic_marker:-<missing>}"
        elif [ "$kernel_panic_variants" -gt 1 ]; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="kernel panic records disagree, so no single location/message signature describes this boot: $(printf '%s' "$kernel_panic_signature" | paste -sd '|' -)"
        else
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="kernel panic: $kernel_panic_signature"
        fi
        return
    fi
    # Deliberately LAST of the attributing arms: every abort signature and both
    # strand arms have already been consulted, so a boot can only reach #609 by
    # having crashed nowhere and stranded nothing. This arm names the shape; it
    # does not excuse it — bucket 609 is a gate FAIL (R33).
    if is_609_early_boot_stage_stall "$serial_file"; then
        CLASS_BUCKET="609"
        CLASS_REASON="EarlyBoot stage advanced and never completed (#609, UNTOLERATED); census: $(grep -ahoE '\[SCHED_STRAND_ORACLE:[^]]*\]' "$serial_file" | tail -1)"
        return
    fi
    if ! grep -qF "[BLOCK_EINTR_ORACLE:" "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="UNATTRIBUTED"
        CLASS_REASON="oracle marker absent"
        return
    fi
    # Anti-vacuity: a boot that never armed the #596 oracle cannot be scored
    # against it, so it is never GREEN by omission.
    if ! grep -qF "[CTX596_ORACLE:ARMED" "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="UNATTRIBUTED"
        CLASS_REASON="#596 inline-save resume-point oracle never armed"
        return
    fi
    if grep -qF "[init] Boot script completed" "$serial_file" 2>/dev/null \
        && grep -qF "create_process_with_argv [ARM64]: ENTRY - name='telnetd'" "$serial_file" 2>/dev/null \
        && grep -qF "[spawn] path='/bin/bounce'" "$serial_file" 2>/dev/null; then
        if ! grep -qF "[INIT_GROUP_REFUSAL:aarch64:phase=quiesce:probe1=-22:probe2=-22:expected=-22]" "$serial_file" 2>/dev/null; then
            CLASS_BUCKET="P5B"
            CLASS_REASON="init-group quiesce refusal marker missing"
            return
        fi
        quiesce_walk_line=$(grep -E '^\[INIT_GROUP_WALK:aarch64:rows=[0-9]+:init_tgid_rows=1:foreign_tgid_rows=0:refused=4:verdict=PASS\]$' "$serial_file" 2>/dev/null | tail -1 || true)
        if [ -z "$quiesce_walk_line" ]; then
            CLASS_BUCKET="P5B"
            CLASS_REASON="init-group quiesce walk marker missing"
            return
        fi
        quiesce_rows=$(echo "$quiesce_walk_line" | sed -n 's/^\[INIT_GROUP_WALK:aarch64:rows=\([0-9][0-9]*\):.*/\1/p')
        # The preserved GREEN service-sequence exhibit has rows=11.  A floor of
        # 8 leaves three rows of headroom for a legitimately shorter service set
        # while making the vacuous rows=1 case a hard failure.
        quiesce_rows_floor=8
        if [ -z "$quiesce_rows" ] || [ "$quiesce_rows" -lt "$quiesce_rows_floor" ]; then
            CLASS_BUCKET="P5B"
            CLASS_REASON="init-group quiesce walk rows ${quiesce_rows:-<missing>} below floor $quiesce_rows_floor"
            return
        fi
        if grep -qE '\[INIT_GROUP_WALK:.*verdict=FAIL' "$serial_file" 2>/dev/null; then
            CLASS_BUCKET="P5B"
            CLASS_REASON="init-group walk reported verdict=FAIL"
            return
        fi
        if grep -qF "[INIT_GROUP_CHILD_RAN]" "$serial_file" 2>/dev/null; then
            CLASS_BUCKET="P5B"
            CLASS_REASON="refused init-group child ran"
            return
        fi
        if ! grep -qF "[SCHED_STRAND_ORACLE:" "$serial_file" 2>/dev/null \
            || ! grep -qF "[STRAND_INJECT_ORACLE:" "$serial_file" 2>/dev/null; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="scheduler strand oracle marker absent"
            return
        fi
        if ! grep -qE "$CENSUS_WIDEN_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
            CLASS_BUCKET="UNATTRIBUTED"
            CLASS_REASON="census widening mutation oracle marker absent or failed"
            return
        fi
        CLASS_BUCKET="GREEN"
        CLASS_REASON="all service-sequence and P5b markers observed"
        return
    fi

    last_line=$(grep -vF "[heartbeat]" "$serial_file" 2>/dev/null | awk 'NF { line = $0 } END { print line }' | sed 's/[[:space:]]*$//')
    CLASS_BUCKET="UNATTRIBUTED"
    CLASS_REASON="last non-heartbeat serial line: ${last_line:-<none>}"
}

TOTAL_575=0
TOTAL_576=0
TOTAL_626=0
TOTAL_635=0
TOTAL_641=0
TOTAL_DATA_ABORT=0
TOTAL_CLONE_EXEC=0
TOTAL_STRAND=0
TOTAL_BOOT_TEST_FAIL=0
TOTAL_596=0
TOTAL_612=0
TOTAL_609=0
TOTAL_P5B=0
TOTAL_GREEN=0
TOTAL_UNATTRIBUTED=0
TOTAL_DIVERGENCE_BOOTS=0
TOTAL_DIVERGENCE_LINES=0
TOTAL_REFUSAL_BOOTS=0
TOTAL_REFUSAL_LINES=0
TOTAL_RESUME_PC_REFUSAL_BOOTS=0
TOTAL_RESUME_PC_REFUSAL_LINES=0
TOTAL_STACK_ALIEN_BOOTS=0
TOTAL_STACK_ALIEN_LINES=0
TOTAL_BOOTS=0
PROFILE_COUNT=0
ANY_GATE_FAILURE=0

print_census() {
    local label="$1"
    local count_575="$2"
    local count_576="$3"
    local count_626="$4"
    local count_635="$5"
    local count_641="$6"
    local count_data_abort="$7"
    local count_clone_exec="$8"
    local count_strand="$9"
    local count_boot_test_fail="${10}"
    local count_596="${11}"
    local count_612="${12}"
    local count_609="${13}"
    local count_p5b="${14}"
    local count_green="${15}"
    local count_unattributed="${16}"
    local count_boots="${17}"
    local divergence_boots="${18}"
    local divergence_lines="${19}"
    local refusal_boots="${20}"
    local refusal_lines="${21}"
    local resume_pc_refusal_boots="${22}"
    local resume_pc_refusal_lines="${23}"
    local stack_alien_boots="${24}"
    local stack_alien_lines="${25}"
    local green_rate

    green_rate=$(awk -v green="$count_green" -v boots="$count_boots" \
        'BEGIN { printf "%.1f", (boots == 0 ? 0 : green * 100 / boots) }')
    echo ""
    echo "$label census"
    printf '  %-13s %d\n' "575" "$count_575"
    printf '  %-13s %d\n' "576" "$count_576"
    printf '  %-13s %d\n' "626" "$count_626"
    printf '  %-13s %d\n' "635" "$count_635"
    printf '  %-13s %d\n' "641" "$count_641"
    printf '  %-13s %d\n' "DATA_ABORT" "$count_data_abort"
    printf '  %-13s %d\n' "CLONE_EXEC" "$count_clone_exec"
    printf '  %-13s %d\n' "STRAND" "$count_strand"
    printf '  %-13s %d\n' "BOOT_TEST_FAIL" "$count_boot_test_fail"
    printf '  %-13s %d\n' "596" "$count_596"
    printf '  %-13s %d\n' "612" "$count_612"
    printf '  %-13s %d\n' "609" "$count_609"
    printf '  %-13s %d\n' "P5B" "$count_p5b"
    printf '  %-13s %d\n' "GREEN" "$count_green"
    printf '  %-13s %d\n' "UNATTRIBUTED" "$count_unattributed"
    echo "  GREEN rate: $count_green/$count_boots ($green_rate%) — census-only: every non-GREEN bucket is gate-failing, with no exceptions, including the open #576, #626, #635 and #641 defects"
    # Reported, never gated: the #596 mechanism counter. A nonzero divergence
    # count with bucket 596 at zero is the production evidence that an
    # inline-saved context really is ERET-dispatched carrying a stale ELR and
    # that the repair neutralises it (coordinator ruling R20).
    echo "  CTX596 divergence: $divergence_lines marker line(s) across $divergence_boots/$count_boots boot(s) — reported, not gated"
    # Reported, never gated: a refusal is the resume-PC validation doing its
    # job. Nonzero refusals together with zero #576-shape boots are production
    # confirmation of that guard and name a producer to chase, not a regression.
    echo "  RET dispatch refused: $refusal_lines marker line(s) across $refusal_boots/$count_boots boot(s) — reported, not gated"
    # GATED, not merely reported. This gate builds no oracle and no injection
    # feature, so every [RESUME_PC_REFUSED:] record it can see was emitted by a
    # production dispatch. The guard that emits the record was earned by the
    # previous PR, which unified every resume-PC consumer on one admission test;
    # a refusal here therefore means a resume PC that failed that admission
    # actually occurred in production — the EL0 arm of the same fault family as
    # the filed #633 and #637 faces. That is a defect to file, not a number to
    # watch, so a non-zero count fails the profile via run_profile's FAIL
    # condition below.
    echo "  Resume PC refused: $resume_pc_refusal_lines marker line(s) across $resume_pc_refusal_boots/$count_boots boot(s) — gate-failing"
    # GATED, on exactly the argument above. [PERCPU_STACK_ALIEN: is emitted only
    # when a per-CPU exception-stack top is not attributable to the CPU asking
    # for it — either the producer declining to choose it or the setter
    # declining to install it, both funnelled through one emitter. This gate
    # builds no oracle and no injection feature (percpu_stack_custody_oracle is
    # a separate cargo feature that boot_tests does not imply), so every record
    # it can see came from a production dispatch, and the #635 acceptance
    # battery showed one such record standing immediately in front of a fatal
    # whole-context-corrupt abort. Same standard as the resume-PC refusal: a
    # defect to file, not a number to watch.
    echo "  Per-CPU stack alien: $stack_alien_lines marker line(s) across $stack_alien_boots/$count_boots boot(s) — gate-failing"
}

run_profile() {
    local cpu_profile="$1"
    local profile_dir="$OUTPUT_DIR/$cpu_profile"
    local census_file="$profile_dir/census.tsv"
    local count_575=0
    local count_576=0
    local count_626=0
    local count_635=0
    local count_641=0
    local count_data_abort=0
    local count_clone_exec=0
    local count_strand=0
    local count_boot_test_fail=0
    local count_596=0
    local count_612=0
    local count_609=0
    local count_p5b=0
    local count_green=0
    local count_unattributed=0
    local divergence_boots=0
    local divergence_lines=0
    local refusal_boots=0
    local refusal_lines=0
    local resume_pc_refusal_boots=0
    local resume_pc_refusal_lines=0
    local stack_alien_boots=0
    local stack_alien_lines=0
    local boot_divergence
    local boot_refusals
    local boot_resume_pc_refusals
    local boot_stack_aliens
    local boot
    local serial_file
    local writable_disk
    local drive_opts
    local qemu_status
    local boot_end
    local boot_seconds
    local boot_start
    local sleep_seconds
    local census_sum

    mkdir -p "$profile_dir"
    printf 'boot\tbucket\tend\tseconds\tctx596_divergence\treason\tserial\tret_dispatch_refusals\tresume_pc_refusals\tpercpu_stack_aliens\n' > "$census_file"
    echo ""
    echo "Profile $cpu_profile: running $BOOTS sequential boots"

    for boot in $(seq 1 "$BOOTS"); do
        serial_file="$profile_dir/serial-$boot.txt"
        writable_disk="$profile_dir/ext2-writable-$boot.img"
        : > "$serial_file"
        cp "$EXT2_DISK" "$writable_disk"
        CURRENT_DISK="$writable_disk"

        drive_opts="if=none,id=ext2,format=raw,file=$writable_disk"
        if [ "$IOPS" -ne 0 ]; then
            drive_opts="$drive_opts,throttling.iops-total=$IOPS"
        fi

        qemu-system-aarch64 \
            -M virt,gic-version=3 -cpu "$cpu_profile" -m 512 -smp 4 \
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
        QEMU_PID=$!

        boot_end="timeout"
        boot_start=$SECONDS
        while :; do
            boot_seconds=$((SECONDS - boot_start))
            if [ "$boot_seconds" -ge "$BOOT_TIMEOUT" ]; then
                kill "$QEMU_PID" 2>/dev/null || true
                break
            fi
            sleep_seconds=$((BOOT_TIMEOUT - boot_seconds))
            if [ "$sleep_seconds" -gt 2 ]; then
                sleep_seconds=2
            fi
            sleep "$sleep_seconds"
            boot_seconds=$((SECONDS - boot_start))

            if grep -qF "[BLOCK_EINTR_ORACLE:FAIL" "$serial_file" 2>/dev/null; then
                boot_end="early"
                kill "$QEMU_PID" 2>/dev/null || true
                break
            fi
            if grep -qF "[INSTRUCTION_ABORT]" "$serial_file" 2>/dev/null; then
                boot_end="early"
                kill "$QEMU_PID" 2>/dev/null || true
                break
            fi
            if grep -qF "[CTX596_ORACLE:FAIL" "$serial_file" 2>/dev/null; then
                boot_end="early"
                kill "$QEMU_PID" 2>/dev/null || true
                break
            fi
            if grep -qE "\[DATA_ABORT\]" "$serial_file" 2>/dev/null; then
                boot_end="early"
                kill "$QEMU_PID" 2>/dev/null || true
                break
            fi
            if green_sequence_complete "$serial_file"; then
                boot_end="early"
                kill "$QEMU_PID" 2>/dev/null || true
                break
            fi
            if [ "$boot_seconds" -ge "$BOOT_TIMEOUT" ]; then
                boot_seconds=$((SECONDS - boot_start))
                kill "$QEMU_PID" 2>/dev/null || true
                break
            fi
        done

        set +e
        wait "$QEMU_PID"
        qemu_status=$?
        set -e
        QEMU_PID=""
        rm -f "$writable_disk"
        CURRENT_DISK=""

        if ! grep -qE "$FUTEX_HANDOFF_ORACLE_PATTERN" "$serial_file" 2>/dev/null; then
            ANY_GATE_FAILURE=1
            echo "  Boot $boot/$BOOTS: futex handoff oracle marker missing or failed"
        fi

        boot_divergence=$(grep -cF "[CTX596_ELR_DIVERGENCE]" "$serial_file" 2>/dev/null | tr -d ' ')
        boot_divergence=${boot_divergence:-0}
        divergence_lines=$((divergence_lines + boot_divergence))
        if [ "$boot_divergence" -ne 0 ]; then
            divergence_boots=$((divergence_boots + 1))
        fi
        boot_refusals=$(grep -cF "[RET_DISPATCH_REFUSED:" "$serial_file" 2>/dev/null | tr -d ' ')
        boot_refusals=${boot_refusals:-0}
        refusal_lines=$((refusal_lines + boot_refusals))
        if [ "$boot_refusals" -ne 0 ]; then
            refusal_boots=$((refusal_boots + 1))
        fi
        # Counted per boot and GATED: this counts the refusal arm that replaced
        # four unequal resume-PC admission tests. Every record this gate can see
        # comes from a production dispatch, so the count feeds the per-profile
        # FAIL condition below rather than a watch list.
        boot_resume_pc_refusals=$(grep -cF "[RESUME_PC_REFUSED:" "$serial_file" 2>/dev/null | tr -d ' ')
        boot_resume_pc_refusals=${boot_resume_pc_refusals:-0}
        resume_pc_refusal_lines=$((resume_pc_refusal_lines + boot_resume_pc_refusals))
        if [ "$boot_resume_pc_refusals" -ne 0 ]; then
            resume_pc_refusal_boots=$((resume_pc_refusal_boots + 1))
        fi
        # Counted per boot and GATED, for the same reason: a production install
        # of a per-CPU stack top the installing CPU does not own.
        boot_stack_aliens=$(grep -cF "[PERCPU_STACK_ALIEN:" "$serial_file" 2>/dev/null | tr -d ' ')
        boot_stack_aliens=${boot_stack_aliens:-0}
        stack_alien_lines=$((stack_alien_lines + boot_stack_aliens))
        if [ "$boot_stack_aliens" -ne 0 ]; then
            stack_alien_boots=$((stack_alien_boots + 1))
        fi

        classify_serial "$serial_file"
        case "$CLASS_BUCKET" in
            575) count_575=$((count_575 + 1)) ;;
            576) count_576=$((count_576 + 1)) ;;
            626) count_626=$((count_626 + 1)) ;;
            635) count_635=$((count_635 + 1)) ;;
            641) count_641=$((count_641 + 1)) ;;
            DATA_ABORT) count_data_abort=$((count_data_abort + 1)) ;;
            CLONE_EXEC) count_clone_exec=$((count_clone_exec + 1)) ;;
            STRAND) count_strand=$((count_strand + 1)) ;;
            BOOT_TEST_FAIL) count_boot_test_fail=$((count_boot_test_fail + 1)) ;;
            596) count_596=$((count_596 + 1)) ;;
            612) count_612=$((count_612 + 1)) ;;
            609) count_609=$((count_609 + 1)) ;;
            P5B) count_p5b=$((count_p5b + 1)) ;;
            GREEN) count_green=$((count_green + 1)) ;;
            UNATTRIBUTED) count_unattributed=$((count_unattributed + 1)) ;;
            *)
                echo "Internal error: unknown bucket '$CLASS_BUCKET' for $serial_file"
                exit 1
                ;;
        esac
        printf '%s\t%s\t%s\t%s\t%s\t%s (qemu_status=%s)\t%s\t%s\t%s\t%s\n' \
            "$boot" "$CLASS_BUCKET" "$boot_end" "$boot_seconds" "$boot_divergence" \
            "$CLASS_REASON" "$qemu_status" "$serial_file" "$boot_refusals" "$boot_resume_pc_refusals" "$boot_stack_aliens" >> "$census_file"
        echo "  Boot $boot/$BOOTS: $CLASS_BUCKET — $CLASS_REASON [$boot_end, ${boot_seconds}s, ctx596_divergence=$boot_divergence, ret_dispatch_refusals=$boot_refusals, resume_pc_refusals=$boot_resume_pc_refusals, percpu_stack_aliens=$boot_stack_aliens]"
    done

    census_sum=$((count_575 + count_576 + count_626 + count_635 + count_641 + count_data_abort + count_clone_exec + count_strand + count_boot_test_fail + count_596 + count_612 + count_609 + count_p5b + count_green + count_unattributed))
    if [ "$census_sum" -ne "$BOOTS" ]; then
        echo "FATAL: $cpu_profile bucket census sums to $census_sum, expected $BOOTS"
        exit 1
    fi

    print_census "Profile $cpu_profile" "$count_575" "$count_576" "$count_626" "$count_635" "$count_641" "$count_data_abort" "$count_clone_exec" \
        "$count_strand" "$count_boot_test_fail" "$count_596" "$count_612" "$count_609" "$count_p5b" "$count_green" "$count_unattributed" "$BOOTS" \
        "$divergence_boots" "$divergence_lines" "$refusal_boots" "$refusal_lines" \
        "$resume_pc_refusal_boots" "$resume_pc_refusal_lines" \
        "$stack_alien_boots" "$stack_alien_lines"

    # #589 is closed; its CLONE_EXEC and STRAND shapes now fail this gate. The
    # GREEN rate stays census-only reporting; open #576 and #626 remain named
    # defect attributions, but neither is a tolerance. Its GREEN denominator is
    # also the P5b whole-boot-walk denominator.
    # #609 joined this condition under R33: its rate pre-adjudication is retired,
    # so a single boot carrying the shape fails the profile immediately instead of
    # being deferred to a run-wide ceiling.
    # #635 is IN this condition. Its bucket keeps the field-keyed classifier arm
    # in classify_serial — that is attribution, and attribution stays — but it
    # has lost the non-failing exemption it carried while its producer was
    # unfixed. The producer is repaired at source on this branch (per-CPU
    # idle/exception stack ownership with refusing setters, tid 0 retired, the
    # return-SP install following the pending exception level, and the
    # reclaimed-thread drop under masked interrupts; docs/planning/t3g-prb/), so
    # there is no rate left to tolerate: one boot carrying the shape fails the
    # profile it happened in, and its serial is preserved by the failure report
    # below. Removing the tolerance is a TIGHTENING — the set of runs this gate
    # passes is strictly smaller than before.
    # #641 is IN this condition (R49): its bucket is an ATTRIBUTION, not a
    # tolerance. Before the bucket existed the signature scored UNATTRIBUTED and
    # failed the profile; it fails the profile now for the same reason, under
    # the name of the open issue it belongs to. Nothing else about the condition
    # changed when it was added.
    # resume_pc_refusal_lines is IN this condition. This gate builds no oracle
    # or injection feature, so a [RESUME_PC_REFUSED:] record here can only have
    # come from a production dispatch whose resume PC failed admission — the EL0
    # arm of the same fault family as the filed #633 and #637 faces. It is a
    # defect to file, not a number to watch.
    if [ "$count_575" -ne 0 ] || [ "$count_576" -ne 0 ] || [ "$count_626" -ne 0 ] || [ "$count_635" -ne 0 ] || [ "$count_641" -ne 0 ] || [ "$count_data_abort" -ne 0 ] || [ "$count_clone_exec" -ne 0 ] || [ "$count_strand" -ne 0 ] || [ "$count_boot_test_fail" -ne 0 ] || [ "$count_596" -ne 0 ] || [ "$count_612" -ne 0 ] || [ "$count_609" -ne 0 ] || [ "$count_p5b" -ne 0 ] || [ "$count_unattributed" -ne 0 ] || [ "$resume_pc_refusal_lines" -ne 0 ] || [ "$stack_alien_lines" -ne 0 ]; then
        ANY_GATE_FAILURE=1
        echo "Profile $cpu_profile gate: FAILED (575=$count_575, 576=$count_576, 626=$count_626, 635=$count_635, 641=$count_641, DATA_ABORT=$count_data_abort, CLONE_EXEC=$count_clone_exec, STRAND=$count_strand, BOOT_TEST_FAIL=$count_boot_test_fail, 596=$count_596, 612=$count_612, 609=$count_609, P5B=$count_p5b, UNATTRIBUTED=$count_unattributed, RESUME_PC_REFUSED=$resume_pc_refusal_lines, PERCPU_STACK_ALIEN=$stack_alien_lines)"
    else
        echo "Profile $cpu_profile gate: PASSED (575=0, 576=0, 626=0, 635=0, 641=0, DATA_ABORT=0, CLONE_EXEC=0, STRAND=0, BOOT_TEST_FAIL=0, 596=0, 612=0, 609=0, P5B=0, UNATTRIBUTED=0, RESUME_PC_REFUSED=0, PERCPU_STACK_ALIEN=0)"
    fi

    TOTAL_575=$((TOTAL_575 + count_575))
    TOTAL_576=$((TOTAL_576 + count_576))
    TOTAL_626=$((TOTAL_626 + count_626))
    TOTAL_635=$((TOTAL_635 + count_635))
    TOTAL_641=$((TOTAL_641 + count_641))
    TOTAL_DATA_ABORT=$((TOTAL_DATA_ABORT + count_data_abort))
    TOTAL_CLONE_EXEC=$((TOTAL_CLONE_EXEC + count_clone_exec))
    TOTAL_STRAND=$((TOTAL_STRAND + count_strand))
    TOTAL_BOOT_TEST_FAIL=$((TOTAL_BOOT_TEST_FAIL + count_boot_test_fail))
    TOTAL_596=$((TOTAL_596 + count_596))
    TOTAL_612=$((TOTAL_612 + count_612))
    TOTAL_609=$((TOTAL_609 + count_609))
    TOTAL_P5B=$((TOTAL_P5B + count_p5b))
    TOTAL_DIVERGENCE_BOOTS=$((TOTAL_DIVERGENCE_BOOTS + divergence_boots))
    TOTAL_DIVERGENCE_LINES=$((TOTAL_DIVERGENCE_LINES + divergence_lines))
    TOTAL_REFUSAL_BOOTS=$((TOTAL_REFUSAL_BOOTS + refusal_boots))
    TOTAL_REFUSAL_LINES=$((TOTAL_REFUSAL_LINES + refusal_lines))
    TOTAL_RESUME_PC_REFUSAL_BOOTS=$((TOTAL_RESUME_PC_REFUSAL_BOOTS + resume_pc_refusal_boots))
    TOTAL_RESUME_PC_REFUSAL_LINES=$((TOTAL_RESUME_PC_REFUSAL_LINES + resume_pc_refusal_lines))
    TOTAL_STACK_ALIEN_BOOTS=$((TOTAL_STACK_ALIEN_BOOTS + stack_alien_boots))
    TOTAL_STACK_ALIEN_LINES=$((TOTAL_STACK_ALIEN_LINES + stack_alien_lines))
    TOTAL_GREEN=$((TOTAL_GREEN + count_green))
    TOTAL_UNATTRIBUTED=$((TOTAL_UNATTRIBUTED + count_unattributed))
    TOTAL_BOOTS=$((TOTAL_BOOTS + BOOTS))
    PROFILE_COUNT=$((PROFILE_COUNT + 1))
}

echo "========================================="
echo "ARM64 #575 Service Sequence Gate"
echo "========================================="
echo "Kernel: $KERNEL"
echo "ext2 disk: $EXT2_DISK"
echo "Boots per profile: $BOOTS"
echo "Profile selection: $PROFILE"
echo "Block IOPS throttle: $IOPS"
echo "Per-boot timeout: ${BOOT_TIMEOUT}s"
echo "Output: $OUTPUT_DIR"

case "$PROFILE" in
    max) run_profile max ;;
    cortex-a72) run_profile cortex-a72 ;;
    both)
        run_profile max
        run_profile cortex-a72
        ;;
esac

TOTAL_SUM=$((TOTAL_575 + TOTAL_576 + TOTAL_626 + TOTAL_635 + TOTAL_641 + TOTAL_DATA_ABORT + TOTAL_CLONE_EXEC + TOTAL_STRAND + TOTAL_BOOT_TEST_FAIL + TOTAL_596 + TOTAL_612 + TOTAL_609 + TOTAL_P5B + TOTAL_GREEN + TOTAL_UNATTRIBUTED))
EXPECTED_TOTAL=$((BOOTS * PROFILE_COUNT))
if [ "$TOTAL_SUM" -ne "$EXPECTED_TOTAL" ] || [ "$TOTAL_BOOTS" -ne "$EXPECTED_TOTAL" ]; then
    echo "FATAL: total bucket census sums to $TOTAL_SUM for $TOTAL_BOOTS recorded boots; expected $EXPECTED_TOTAL"
    exit 1
fi

print_census "Total" "$TOTAL_575" "$TOTAL_576" "$TOTAL_626" "$TOTAL_635" "$TOTAL_641" "$TOTAL_DATA_ABORT" "$TOTAL_CLONE_EXEC" \
    "$TOTAL_STRAND" "$TOTAL_BOOT_TEST_FAIL" "$TOTAL_596" "$TOTAL_612" "$TOTAL_609" "$TOTAL_P5B" "$TOTAL_GREEN" "$TOTAL_UNATTRIBUTED" "$TOTAL_BOOTS" \
    "$TOTAL_DIVERGENCE_BOOTS" "$TOTAL_DIVERGENCE_LINES" "$TOTAL_REFUSAL_BOOTS" "$TOTAL_REFUSAL_LINES" \
    "$TOTAL_RESUME_PC_REFUSAL_BOOTS" "$TOTAL_RESUME_PC_REFUSAL_LINES" \
    "$TOTAL_STACK_ALIEN_BOOTS" "$TOTAL_STACK_ALIEN_LINES"

# The #609 run-wide rate ceiling that used to live here is DELETED (R33) and
# stays deleted (R37). A rate ceiling is a tolerance: it let up to
# ceil(0.06 * boots) boots carry the shape and still pass. The justification
# recorded here previously — that the mechanism was "falsified by its own forced
# arm" and that "the class did not occur once in 290 non-forcing boots on main" —
# is RETRACTED and must not be re-cited: the forced arm was unjoined and could
# not emit the signature on a healthy or a broken kernel alike, and the class was
# afterwards reproduced ~1.3% over ~560 boots on this same -cpu cortex-a72 -smp 4
# IOPS-2000 setup (7 wedges, 6 captured live under GDB).
# The real justification is the fix: #609 is root-caused (an ARM64_STACK_BITMAP
# holder preempted mid-critical-section, orphaning a lock that CPU 0 then spins on
# with preemption disabled while idle peers spin on it with all interrupts masked)
# and repaired at source on this branch — irqsave guard type on the bitmap,
# reclamation hoisted out of the idle loop's masked window, and a placement rule
# that stops parking work on a CPU that cannot dispatch it. A fixed defect has no
# rate left to tolerate: every occurrence now fails the profile it happened in via
# count_609 in run_profile's FAIL condition, and its serial is preserved by the
# failure report below. Removing the tolerance is a tightening: the set of runs
# this gate passes is strictly smaller than before.

if [ "$ANY_GATE_FAILURE" -ne 0 ]; then
    echo ""
    echo "ARM64 #575 SERVICE SEQUENCE GATE: FAILED"
    echo "Non-GREEN boots:"
    for census_file in "$OUTPUT_DIR"/*/census.tsv; do
        awk -F '\t' 'NR > 1 && $2 != "GREEN" { printf "  %s: %s — %s [%s, %ss]\n", $7, $2, $6, $3, $4 }' "$census_file"
    done
    echo "Preserved serials:"
    find "$OUTPUT_DIR" -type f -name 'serial-*.txt' -print | sort | sed 's/^/  /'
    exit 1
fi

echo ""
echo "ARM64 #575 SERVICE SEQUENCE GATE: PASSED"
echo "Preserved serials: $OUTPUT_DIR"
exit 0
