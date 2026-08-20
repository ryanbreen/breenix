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

# Return 0 when this serial carries the filed #609 signature: the network:early
# subsystem kthread was created and then never dispatched.
#
# #609 is pre-adjudicated as an attributed non-green (coordinator ruling R30) at
# its filed ~3% rate, so it earns a NAMED bucket instead of UNATTRIBUTED — but it
# earns it only on the FIELD signature, and the rate ceiling enforced after the
# last profile is what stops the attribution becoming an unlimited excuse. Every
# clause below is a shape, never a name list, and every one of them must hold;
# anything that misses one falls through to UNATTRIBUTED, which fails this gate.
#
#   * memory:early ran to COMPLETE, so the early stage really was executing and
#     this is not a boot that died before the test framework started;
#   * the network:early kthread emitted NOTHING — zero [SUBSYSTEM:network:...]
#     lines and zero [TEST:network:...] lines. A dispatched-then-wedged kthread
#     prints its own :START first, so total silence is the discriminator between
#     "never got a first instruction" and "ran and hung", and it is counted as a
#     line census over both marker forms rather than against the name of any
#     particular network test;
#   * the stage consequently never completed — no [STAGE:early:COMPLETE — so the
#     join really is still outstanding;
#   * no abort, panic or lockup of ANY kind. #609 is a stall, not a crash. The
#     classifier has already consulted every abort signature by the time this
#     runs; the clause is repeated here so the arm still cannot absorb a crash if
#     it is ever reordered;
#   * the kernel stayed alive to the wall clock: the strand census kept sampling
#     into the hundreds and never saw a strand. That clean census is itself part
#     of the filed signature — #609 records that the census cannot see this class
#     of lost dispatch (worst_dwell_ms=0, ~2 threads examined per sample), so a
#     clean census here is evidence of the blind spot, not of health.
is_609_network_early_stall() {
    local serial_file="$1"

    grep -qF "[SUBSYSTEM:memory:early:COMPLETE:" "$serial_file" 2>/dev/null || return 1
    if grep -qE '\[(TEST|SUBSYSTEM):network:' "$serial_file" 2>/dev/null; then
        return 1
    fi
    if grep -qF "[STAGE:early:COMPLETE" "$serial_file" 2>/dev/null; then
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

    # #596's runtime oracle is unconditional: an inline-saved context whose
    # recorded resume PC is not its inline-save x30 is a defect no matter what
    # else the boot did, so it is consulted before every other signature.
    if grep -qF "[CTX596_ORACLE:FAIL" "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="596"
        CLASS_REASON="inline-save resume-point oracle failed: $(grep -o '\[CTX596_ORACLE:FAIL:[a-z_]*' "$serial_file" | head -1)"
        return
    fi
    if grep -qE "\[DATA_ABORT\].*from_el0=0" "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="596"
        CLASS_REASON="EL1 data abort: $(grep -E "\[DATA_ABORT\].*from_el0=0" "$serial_file" | head -1 | sed 's/[[:space:]]*$//')"
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
    # type. #576 is filed as exactly FAR=0x0 ELR=0x0 ESR=0x86000005; bucketing
    # every [INSTRUCTION_ABORT] as 576 made a different signature invisible by
    # construction, which is how a FAR=ELR=0x80000000 regression rode into a
    # tolerated bucket. Anything that does not match a filed signature — or whose
    # fields cannot be read at all — is UNATTRIBUTED, which this gate FAILS on.
    #
    # The match is on the WHOLE SET of records the serial carries, not on a
    # first-found record: a boot earns the tolerated #576 bucket only when every
    # abort record it emitted names one and the same filed signature. A serial
    # carrying two different signatures, or one fault whose two records disagree,
    # is UNATTRIBUTED — otherwise a novel signature could ride in behind a
    # matching one and the tolerated bucket would hide it again.
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
    # The serial console emits CRLF; trim trailing whitespace before exact comparisons and reports.
    last_line=$(grep -F "CLONEVM_EXEC_TEST" "$serial_file" 2>/dev/null | tail -1 | sed 's/[[:space:]]*$//' || true)
    if [ "$last_line" = "CLONEVM_EXEC_TEST: live sibling refused exec" ]; then
        CLASS_BUCKET="589"
        CLASS_REASON="live sibling refused exec"
        return
    fi
    # A crashed boot's cleanup can strand a thread; attribute strands to #589 only if no crash came first.
    stranded_strand_line=$(grep -E '\[SCHED_STRAND_ORACLE:[^]]*:stranded=[1-9][0-9]*:' \
        "$serial_file" 2>/dev/null | tail -1 || true)
    if [ -n "$stranded_strand_line" ]; then
        CLASS_BUCKET="589"
        CLASS_REASON="scheduler strand census reported stranded work: $stranded_strand_line"
        return
    fi
    if grep -qE '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' \
        "$serial_file" 2>/dev/null; then
        CLASS_BUCKET="589"
        CLASS_REASON="strand injection oracle reported stranded work: $(grep -E '\[STRAND_INJECT_ORACLE:[^]]*:stranded=[1-9][0-9]*\]' "$serial_file" | tail -1)"
        return
    fi
    # Deliberately LAST of the attributing arms: every abort signature and both
    # strand arms have already been consulted, so a boot can only reach #609 by
    # having crashed nowhere and stranded nothing.
    if is_609_network_early_stall "$serial_file"; then
        CLASS_BUCKET="609"
        CLASS_REASON="network:early subsystem kthread never dispatched after memory:early completed (#609); census: $(grep -ahoE '\[SCHED_STRAND_ORACLE:[^]]*\]' "$serial_file" | tail -1)"
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
TOTAL_DATA_ABORT=0
TOTAL_589=0
TOTAL_596=0
TOTAL_609=0
TOTAL_P5B=0
TOTAL_GREEN=0
TOTAL_UNATTRIBUTED=0
TOTAL_DIVERGENCE_BOOTS=0
TOTAL_DIVERGENCE_LINES=0
TOTAL_BOOTS=0
PROFILE_COUNT=0
ANY_GATE_FAILURE=0

print_census() {
    local label="$1"
    local count_575="$2"
    local count_576="$3"
    local count_data_abort="$4"
    local count_589="$5"
    local count_596="$6"
    local count_609="$7"
    local count_p5b="$8"
    local count_green="$9"
    local count_unattributed="${10}"
    local count_boots="${11}"
    local divergence_boots="${12}"
    local divergence_lines="${13}"
    local green_rate

    green_rate=$(awk -v green="$count_green" -v boots="$count_boots" \
        'BEGIN { printf "%.1f", (boots == 0 ? 0 : green * 100 / boots) }')
    echo ""
    echo "$label census"
    printf '  %-13s %d\n' "575" "$count_575"
    printf '  %-13s %d\n' "576" "$count_576"
    printf '  %-13s %d\n' "DATA_ABORT" "$count_data_abort"
    printf '  %-13s %d\n' "589" "$count_589"
    printf '  %-13s %d\n' "596" "$count_596"
    printf '  %-13s %d\n' "609" "$count_609"
    printf '  %-13s %d\n' "P5B" "$count_p5b"
    printf '  %-13s %d\n' "GREEN" "$count_green"
    printf '  %-13s %d\n' "UNATTRIBUTED" "$count_unattributed"
    echo "  GREEN rate: $count_green/$count_boots ($green_rate%) — not a gate today: #589 and #576 are open and intercept boots"
    # Reported, never gated: the #596 mechanism counter. A nonzero divergence
    # count with bucket 596 at zero is the production evidence that an
    # inline-saved context really is ERET-dispatched carrying a stale ELR and
    # that the repair neutralises it (coordinator ruling R20).
    echo "  CTX596 divergence: $divergence_lines marker line(s) across $divergence_boots/$count_boots boot(s) — reported, not gated"
}

run_profile() {
    local cpu_profile="$1"
    local profile_dir="$OUTPUT_DIR/$cpu_profile"
    local census_file="$profile_dir/census.tsv"
    local count_575=0
    local count_576=0
    local count_data_abort=0
    local count_589=0
    local count_596=0
    local count_609=0
    local count_p5b=0
    local count_green=0
    local count_unattributed=0
    local divergence_boots=0
    local divergence_lines=0
    local boot_divergence
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
    printf 'boot\tbucket\tend\tseconds\tctx596_divergence\treason\tserial\n' > "$census_file"
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

        classify_serial "$serial_file"
        case "$CLASS_BUCKET" in
            575) count_575=$((count_575 + 1)) ;;
            576) count_576=$((count_576 + 1)) ;;
            DATA_ABORT) count_data_abort=$((count_data_abort + 1)) ;;
            589) count_589=$((count_589 + 1)) ;;
            596) count_596=$((count_596 + 1)) ;;
            609) count_609=$((count_609 + 1)) ;;
            P5B) count_p5b=$((count_p5b + 1)) ;;
            GREEN) count_green=$((count_green + 1)) ;;
            UNATTRIBUTED) count_unattributed=$((count_unattributed + 1)) ;;
            *)
                echo "Internal error: unknown bucket '$CLASS_BUCKET' for $serial_file"
                exit 1
                ;;
        esac
        printf '%s\t%s\t%s\t%s\t%s\t%s (qemu_status=%s)\t%s\n' \
            "$boot" "$CLASS_BUCKET" "$boot_end" "$boot_seconds" "$boot_divergence" \
            "$CLASS_REASON" "$qemu_status" "$serial_file" >> "$census_file"
        echo "  Boot $boot/$BOOTS: $CLASS_BUCKET — $CLASS_REASON [$boot_end, ${boot_seconds}s, ctx596_divergence=$boot_divergence]"
    done

    census_sum=$((count_575 + count_576 + count_data_abort + count_589 + count_596 + count_609 + count_p5b + count_green + count_unattributed))
    if [ "$census_sum" -ne "$BOOTS" ]; then
        echo "FATAL: $cpu_profile bucket census sums to $census_sum, expected $BOOTS"
        exit 1
    fi

    print_census "Profile $cpu_profile" "$count_575" "$count_576" "$count_data_abort" "$count_589" \
        "$count_596" "$count_609" "$count_p5b" "$count_green" "$count_unattributed" "$BOOTS" \
        "$divergence_boots" "$divergence_lines"

    # The GREEN rate is census-only because open #589 and #576 intercept boots;
    # its GREEN denominator is now also the P5b whole-boot-walk denominator.
    # #609 is not in this per-profile condition because its pre-adjudication is a
    # RATE, and a rate is only meaningful over the whole run; it is enforced once,
    # against every boot the run produced, after the last profile finishes.
    if [ "$count_575" -ne 0 ] || [ "$count_data_abort" -ne 0 ] || [ "$count_596" -ne 0 ] || [ "$count_p5b" -ne 0 ] || [ "$count_unattributed" -ne 0 ]; then
        ANY_GATE_FAILURE=1
        echo "Profile $cpu_profile gate: FAILED (575=$count_575, DATA_ABORT=$count_data_abort, 596=$count_596, P5B=$count_p5b, UNATTRIBUTED=$count_unattributed)"
    else
        echo "Profile $cpu_profile gate: PASSED (575=0, DATA_ABORT=0, 596=0, P5B=0, UNATTRIBUTED=0; 609=$count_609 pending the run-wide rate ceiling)"
    fi

    TOTAL_575=$((TOTAL_575 + count_575))
    TOTAL_576=$((TOTAL_576 + count_576))
    TOTAL_DATA_ABORT=$((TOTAL_DATA_ABORT + count_data_abort))
    TOTAL_589=$((TOTAL_589 + count_589))
    TOTAL_596=$((TOTAL_596 + count_596))
    TOTAL_609=$((TOTAL_609 + count_609))
    TOTAL_P5B=$((TOTAL_P5B + count_p5b))
    TOTAL_DIVERGENCE_BOOTS=$((TOTAL_DIVERGENCE_BOOTS + divergence_boots))
    TOTAL_DIVERGENCE_LINES=$((TOTAL_DIVERGENCE_LINES + divergence_lines))
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

TOTAL_SUM=$((TOTAL_575 + TOTAL_576 + TOTAL_DATA_ABORT + TOTAL_589 + TOTAL_596 + TOTAL_609 + TOTAL_P5B + TOTAL_GREEN + TOTAL_UNATTRIBUTED))
EXPECTED_TOTAL=$((BOOTS * PROFILE_COUNT))
if [ "$TOTAL_SUM" -ne "$EXPECTED_TOTAL" ] || [ "$TOTAL_BOOTS" -ne "$EXPECTED_TOTAL" ]; then
    echo "FATAL: total bucket census sums to $TOTAL_SUM for $TOTAL_BOOTS recorded boots; expected $EXPECTED_TOTAL"
    exit 1
fi

print_census "Total" "$TOTAL_575" "$TOTAL_576" "$TOTAL_DATA_ABORT" "$TOTAL_589" \
    "$TOTAL_596" "$TOTAL_609" "$TOTAL_P5B" "$TOTAL_GREEN" "$TOTAL_UNATTRIBUTED" "$TOTAL_BOOTS" \
    "$TOTAL_DIVERGENCE_BOOTS" "$TOTAL_DIVERGENCE_LINES"

# #609's pre-adjudication is a bounded attribution, never a blanket excuse:
# coordinator ruling R30 tolerates it "at its ~3% rate" and says a materially
# higher rate is a NEW defect to investigate, not a bucket to grow. This gate
# therefore enforces the rate itself rather than trusting a reader to notice.
#
# The trip point is twice the filed rate, with a floor of one boot so a short run
# is never failed by a single occurrence:
#
#   ceiling = max(1, ceil(0.06 * total boots))  ->  3 at the default 50 boots.
#
# At the filed p=0.03 a 50-boot run exceeds three #609 boots about 6% of the
# time, so crossing the line means a materially higher rate rather than ordinary
# binomial variance. Exceeding the ceiling FAILS this gate — the boots stay
# attributed, but the run stops being covered by the pre-adjudication.
TOTAL_609_CEILING=$(awk -v boots="$TOTAL_BOOTS" \
    'BEGIN { ceiling = int(boots * 6 / 100); if ((boots * 6) % 100 != 0) ceiling++; if (ceiling < 1) ceiling = 1; print ceiling }')
if [ "$TOTAL_609" -gt "$TOTAL_609_CEILING" ]; then
    ANY_GATE_FAILURE=1
    echo ""
    echo "#609 RATE CEILING EXCEEDED: $TOTAL_609 of $TOTAL_BOOTS boots carry the #609 stall signature, ceiling $TOTAL_609_CEILING (twice the filed ~3% rate)."
    echo "  The pre-adjudication covers #609 at its filed rate only. This run is materially above it and must be investigated as new."
fi

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
