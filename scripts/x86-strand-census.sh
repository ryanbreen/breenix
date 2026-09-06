#!/usr/bin/env bash
# x86 strand census: consume the kernel's newest dispatch-ledger snapshot.
#
# #568 round 2 found a blocking poll thread that was saved and then stopped
# running. #775 removed the formatted save/restore records from the dispatch
# path and put the same state transitions in a fixed atomic ledger. Three
# contexts emit that ledger at most once per second from ordinary thread
# context with interrupts enabled -- the scheduler's idle loop, the loopback
# pump, and the `kstrandd` census kthread -- and the syscall completion path
# emits one final snapshot outside the limiter. The newest snapshot in the
# capture is the source of truth, so a wedged userspace thread does not have
# to resume or exit to make its state observable.
# claim-lint:ok: #775 rulings R125 and R137 define the emission contexts.
#
# Each snapshot carries `seq`, `tick`, `ms`, the distinct ever-saved count, the
# stranded count, and up to 16 stranded TIDs. This script recovers their names
# from the existing `Added thread N <name>` records.
#
# INPUT CONTRACT
#   * Snapshots are written on COM2, the kernel-log channel
#     (kernel/src/task/dispatch_strand_census.rs uses `log_serial_println!`;
#     kernel/src/serial.rs:17-19 reserves COM1 for user I/O). The KERNEL serial
#     capture MUST therefore be among the arguments; the `Added thread` name
#     records are on the same channel. Passing the COM1 capture as well is
#     harmless, and is what all three in-repo callers do.
#   * Every argument must belong to ONE boot. Argument ORDER DOES NOT MATTER:
#     the snapshot judged is the one with the highest `seq`, not the last one in
#     concatenation order. `seq` is 1-based and unique within a boot, so two
#     boots concatenated repeat a seq and are rejected (exit 2) instead of being
#     silently mixed. Byte-identical duplicate markers -- the same capture
#     passed twice -- are collapsed before that check. The AGE line below is
#     order-independent for the same reason: the completion marker and the
#     snapshot that follows it are located by position within ONE capture, not
#     by position in a concatenation of all of them.
#     claim-lint:ok: the ordering, two-boot and age-ordering arms are covered
#     by tests/x86_gate_verdict_test.rs.
#   * Every marker is validated. Malformed ones are counted and skipped, so one
#     truncated trailing marker cannot discard an otherwise readable red
#     reading; the highest-seq VALID snapshot still decides.
#     claim-lint:ok: both truncation shapes are covered by
#     tests/x86_gate_verdict_test.rs.
#
# WHAT THE VERDICT DOES AND DOES NOT SAY
#   A snapshot is the ledger's state at one instant. A listed thread was saved
#   blocked and had not been restored AS OF THAT SNAPSHOT; it may have resumed
#   afterwards. The emitted sentence says exactly that, and the snapshot's
#   `seq` and `tick` are printed with it.
#   claim-lint:ok: the emission contract is the one #775 ruling R137 sets and
#   the age arms are covered by tests/x86_gate_verdict_test.rs.
#
#   The emission is rate-LIMITED, not periodic. `kstrandd` sleeps on the
#   scheduler timer, so the cadence does not need the CPU to idle, but the
#   snapshot is published only once that kthread is DISPATCHED after its timer
#   wake -- the wake-to-dispatch latency #766 measures. Under load that latency
#   is seconds: the two committed round-4 gate captures carry census holes of
#   19939 ms and 17888 ms with `kstrandd` alive and having published at 1 Hz
#   right up to each hole. Anything else that stops it running -- a wedge
#   holding the scheduler lock, a lost timer wake -- leaves the newest snapshot
#   stale in the same way. The capture carries no end-of-boot timestamp, so
#   staleness at the END of the boot is still not derivable; what IS derivable
#   is staleness at the completion marker, and that is asserted (see AGE
#   below). The observed gaps are printed too.
#   claim-lint:ok: both holes are re-derivable from the `ms=` fields of
#   docs/planning/green-program/sockets/serials/775/round4/gate-green/
#   boot{1,2}/serial_kernel.txt.
#
# AGE AT THE COMPLETION MARKER
#   The completion site emits a snapshot immediately after the kernel prints
#   `USERSPACE TEST COMPLETE`, so a capture that reaches that point carries a
#   kernel timestamp for a known late instant. The age reported is that
#   timestamp minus the ms of the newest CADENCE snapshot before it, i.e. how
#   stale the reading a consumer would have judged was when the userspace
#   phase ended. On a capture with no completion marker there is no such
#   reference and the line says so instead of inventing one. A capture that
#   DOES carry the marker but no valid snapshot after it is a truncated
#   capture, not an unmeasurable one: it exits 2 rather than skipping the
#   assertion -- but ONLY when the reading it carries is otherwise clean. A
#   truncated capture whose newest valid snapshot is RED still exits 1 and
#   names the threads; see PRECEDENCE under Exit below.
#
#   THE BOUND IS DERIVED, not chosen, and this file holds it in exactly ONE
#   place: the `stale_limit_ms` assignment in the awk BEGIN block below. #766
#   measured the x86 wake-to-dispatch overrun this cadence rides on -- min
#   84 ms, p50 426.5 ms, p90 2592 ms, max 10318 ms over 324 re-derivable
#   trials, recorded in
#   docs/planning/green-program/sockets/693-RCA-2026-09-02.md -- and the bound
#   is that measured maximum plus margin. It tightens when #766 lands, which is
#   why no second copy of the VALUE exists anywhere: this script's own age line
#   prints it, and scripts/x86-gate-verdict.sh's rc=4 sentence, the tests and
#   the docs all read the number back out of what is printed here rather than
#   restating it -- finding F4.
#   claim-lint:ok: the bound, its derivation, its single-copy property and all
#   3 age arms are covered by tests/x86_gate_verdict_test.rs and
#   tests/dispatch_strand_census_structure.rs.
#
# Usage:  scripts/x86-strand-census.sh <serial-log> [<serial-log> ...]
# Output: one line per listed stranded thread, a provenance/cadence line, an
#         age line, overflow diagnostics if present, then a STRAND_CENSUS line.
# Exit:   0 when the highest-seq valid snapshot says stranded=0, the ledger did
#           not overflow, the census was fresh at the completion marker (or no
#           age is measurable), and the capture is not truncated at the marker;
#         1 when it says stranded>0;
#         2 when no valid snapshot exists, or the inputs carry snapshots from
#           more than one boot, or the capture carries the completion marker
#           with no valid snapshot after it (a truncated capture: the age
#           assertion is not silently skipped);
#         3 when the kernel ledger overflowed, so the snapshot is incomplete and
#           carries no verdict either way (R134 item 2: not a clean census);
#         4 when stranded=0 but the newest cadence snapshot was older at the
#           completion marker than the bound printed with the age line, so the
#           clean reading is stale rather than clean (R137, bound per AGE above).
#
#         PRECEDENCE IS 1 > 3 > 4 > 2, AND THE CODE BELOW TAKES THE EXITS IN
#         THAT ORDER. A RED READING IS NEVER MASKED: stranded>0 exits 1 even
#         when the ledger overflowed, even when the reading was stale at the
#         completion marker, and even when the capture is truncated there.
#         Each of those three says the reading is WORSE than it looks, never
#         better, so none of them may downgrade a named strand to "census
#         unavailable" -- finding F1, which is what the round-5 order did.
#         The truncated-at-the-marker rc=2 is therefore the LAST arm, reached
#         only when the newest valid snapshot is clean, unoverflowed and not
#         stale.
#         The two UNREADABLE rc=2 classes -- no valid snapshot at all, and
#         snapshots from more than one boot -- still short-circuit ahead of all
#         of them, because no verdict can be computed from those inputs at all.
# claim-lint:ok: the 5 exit classes and the 1-outranks-2 precedence are covered
# by tests/x86_gate_verdict_test.rs.

set -euo pipefail

if [[ $# -eq 0 ]]; then
    echo "usage: $0 <serial-log> [<serial-log> ...]" >&2
    exit 2
fi

for serial_log in "$@"; do
    [[ -r "$serial_log" ]] || { echo "strand census: serial log is not readable: $serial_log" >&2; exit 2; }
done

# The captures are handed to awk as OPERANDS, not concatenated through `cat`.
# The completion marker and the snapshot that follows it are then located by
# position WITHIN one capture (`FNR`), so which order the caller passes the
# files in cannot change the age line -- the R4-6 defect in the streaming form.
# It also makes `lines=` the true total line count either way.
awk '
BEGIN {
    census_re = "\\[DISPATCH_STRAND_CENSUS:[^]]*\\]"
    # Derived from #766, not chosen: max measured wake-to-dispatch overrun
    # 10318 ms (324 trials, docs/planning/green-program/sockets/
    # 693-RCA-2026-09-02.md) plus margin. Tightens when #766 lands. This
    # assignment is the single copy of the value in the repository outside
    # captured tool output; consumers read it back from what is printed.
    stale_limit_ms = 15000
    marker_present = 0
    file_complete = 0
    completion_seq = 0
    truncated_at_marker = 0
    # The eight fields this tool reads, then any number of further
    # name=digits fields. That tail is what PR-1 of the critical-path logging
    # drain appends: ten DispatchLogFact totals, which belong to the
    # dispatch-path publication census and not to the strand verdict this
    # script computes. Accepting rather than requiring them is deliberate. The
    # committed round-4 captures of 775 under
    # docs/planning/green-program/sockets/serials/775/ carry the eight-field
    # form and are replayed verbatim by tests/x86_gate_verdict_test.rs, so a
    # shape check that demanded eighteen fields would score those real
    # captures malformed. What holds the ten new fields on live bytes is the
    # DISPATCH_FACT_ORACLE pin in docker/qemu/run-x86-boot-tests.sh, not this
    # regex.
    #
    # NOTE: this comment sits INSIDE the single-quoted awk program, so it
    # carries no apostrophe and no backtick. One apostrophe here terminates
    # the program string and what is left prints 0 lines and exits 0; the
    # first draft of this very comment did exactly that, and
    # tests/x86_gate_verdict_test.rs caught it.
    valid_re = "^\\[DISPATCH_STRAND_CENSUS:seq=[0-9]+:tick=[0-9]+:ms=[0-9]+:saved=[0-9]+:stranded=[0-9]+:tids=(-|[0-9]+(,[0-9]+)*):tid_overflow=[0-9]+:ledger_overflow=[0-9]+(:[a-z_]+=[0-9]+)*\\]$"
    best_seq = -1
    valid = 0
    malformed = 0
    duplicate_boot = 0
}

function field(marker, key,   data, count, parts, i, equals, name) {
    data = marker
    sub(/^\[/, "", data); sub(/\]$/, "", data)
    count = split(data, parts, ":")
    for (i = 2; i <= count; i++) {
        equals = index(parts[i], "=")
        name = substr(parts[i], 1, equals - 1)
        if (name == key) return substr(parts[i], equals + 1)
    }
    return ""
}

function consistent(marker,   listed, tids, spare) {
    listed = 0
    if (field(marker, "tids") != "-") listed = split(field(marker, "tids"), tids, ",")
    return (listed + (field(marker, "tid_overflow") + 0) == (field(marker, "stranded") + 0))
}

# Per-capture state. `marker_present` is detected INDEPENDENTLY of the snapshot
# parse, so the presence of the marker and the presence of a snapshot after it
# are two separate facts and neither stands in for the other; `file_complete`
# resets at each capture so the completion snapshot is the first valid snapshot
# after the marker IN THAT capture.
FNR == 1 { file_complete = 0 }
/USERSPACE TEST COMPLETE/ { marker_present = 1; file_complete = 1 }

/Added thread [0-9]+ / {
    line = $0
    sub(/.*Added thread /, "", line)
    tid = line; sub(/ .*/, "", tid)
    rest = line; sub(/^[0-9]+ /, "", rest)
    if (substr(rest, 1, 1) == "\047") {
        sub(/^\047/, "", rest); sub(/\047.*/, "", rest)
        names[tid] = rest
    }
}
{
    rest = $0
    while (match(rest, census_re)) {
        marker = substr(rest, RSTART, RLENGTH)
        rest = substr(rest, RSTART + RLENGTH)

        if (marker !~ valid_re || !consistent(marker)) {
            malformed++
            if (malformed == 1) first_malformed = marker
            continue
        }

        seq = field(marker, "seq") + 0
        if (seq in seen) {
            # Same capture handed in twice collapses; a genuinely different
            # snapshot sharing a seq means a second boot is in the input.
            if (seen[seq] != marker) duplicate_boot = 1
            continue
        }
        seen[seq] = marker
        ms[seq] = field(marker, "ms") + 0
        valid++
        if (file_complete && completion_seq == 0) completion_seq = seq
        if (seq > best_seq) { best_seq = seq; best = marker }
    }
}
END {
    # The two UNREADABLE inputs: neither yields a verdict, so they precede the
    # whole precedence chain rather than sitting inside it.
    if (duplicate_boot) {
        print "strand census: the inputs carry census snapshots from more than one boot (repeated seq); pass the capture set of a single boot" > "/dev/stderr"
        exit 2
    }
    if (valid == 0) {
        if (malformed > 0)
            printf "strand census: no valid DISPATCH_STRAND_CENSUS snapshot found (%d malformed marker(s), first: %s)\n", malformed, first_malformed > "/dev/stderr"
        else
            print "strand census: no DISPATCH_STRAND_CENSUS line found" > "/dev/stderr"
        exit 2
    }

    # Cadence, over the valid snapshots that are actually present.
    max_gap = -1; last_gap = -1; previous = -1
    for (seq = 0; seq <= best_seq; seq++) {
        if (!(seq in seen)) continue
        if (previous >= 0) {
            gap = ms[seq] - ms[previous]
            if (gap > max_gap) max_gap = gap
            last_gap = gap
        }
        previous = seq
    }

    saved = field(best, "saved") + 0
    stranded = field(best, "stranded") + 0
    tid_overflow = field(best, "tid_overflow") + 0
    ledger_overflow = field(best, "ledger_overflow") + 0
    tick = field(best, "tick")
    at_ms = field(best, "ms")

    listed = 0
    if (field(best, "tids") != "-") listed = split(field(best, "tids"), stranded_tids, ",")

    for (i = 1; i <= listed; i++) {
        tid = stranded_tids[i]
        name = (tid in names) ? names[tid] : "?"
        printf "strand census: thread %s (%s) saved blocked and not restored as of the latest snapshot (seq %d, tick %s)\n", tid, name, best_seq, tick
    }
    if (tid_overflow > 0)
        printf "strand census: %d additional stranded thread(s) omitted by the kernel TID-list bound\n", tid_overflow

    printf "strand census: latest snapshot seq=%d tick=%s at %s ms; %d valid snapshot(s)", best_seq, tick, at_ms, valid
    if (last_gap >= 0) printf ", previous %d ms earlier, largest gap %d ms", last_gap, max_gap
    else printf ", no earlier snapshot to measure cadence against"
    if (malformed > 0) printf ", %d malformed marker(s) skipped", malformed
    printf "\n"

    # AGE AT THE COMPLETION MARKER. The completion snapshot is the one the
    # syscall completion site emits immediately after `USERSPACE TEST COMPLETE`,
    # so it is a kernel timestamp for a known late point in the boot -- the only
    # such reference a capture carries. Age is that timestamp minus the ms of
    # the newest CADENCE snapshot that preceded it, i.e. how stale the ledger
    # reading was at the end of the userspace phase.
    age_measured = 0
    age_ms = -1
    if (completion_seq > 0) {
        before = -1
        for (seq = 0; seq < completion_seq; seq++) if (seq in seen) before = seq
        if (before >= 0) {
            age_measured = 1
            age_ms = ms[completion_seq] - ms[before]
            printf "strand census: age at the completion marker: %d ms (newest cadence snapshot seq=%d at %d ms, completion snapshot seq=%d at %d ms, bound %d ms)\n", age_ms, before, ms[before], completion_seq, ms[completion_seq], stale_limit_ms
        } else {
            printf "strand census: age at the completion marker: not measurable -- the completion snapshot seq=%d is the first valid snapshot in the capture\n", completion_seq
        }
    } else if (marker_present) {
        # TRUNCATED: the marker is here, the snapshot that should follow it is
        # not. Saying "no marker" here would be false (finding R4-5), and the
        # capture is not unmeasurable by nature, it is cut short. The exit this
        # sets up is taken LAST, so it can mask no red reading (finding F1).
        truncated_at_marker = 1
        printf "strand census: age at the completion marker: not measurable -- this capture carries the USERSPACE TEST COMPLETE marker but no valid snapshot follows it in that capture, so it is TRUNCATED and the newest reading (seq=%d at %s ms) carries no freshness evidence\n", best_seq, at_ms
    } else {
        printf "strand census: age at the completion marker: not measurable -- this capture carries no USERSPACE TEST COMPLETE, so it has no kernel timestamp for a known late point; newest snapshot seq=%d at %s ms\n", best_seq, at_ms
    }

    # An overflowed ledger is REPORTED whichever exit is taken below, because it
    # qualifies the snapshot the verdict is read from either way.
    if (ledger_overflow > 0)
        printf "strand census: kernel ledger overflowed (%d event(s)); the snapshot is incomplete and carries no verdict\n", ledger_overflow

    # PRECEDENCE 1 > 3 > 4 > 2, in this order, and this is the whole of it.
    # A RED READING IS NEVER MASKED (finding F1): a named strand outranks an
    # overflowed ledger, a stale age and a truncated capture alike, because
    # each of those three makes the reading worse than it looks, never better.
    # claim-lint:ok: the order and its anti-masking arms are covered by
    # tests/x86_gate_verdict_test.rs.
    if (stranded > 0) {
        printf "STRAND_CENSUS: threads_saved_blocked=%d stranded=%d lines=%d\n", saved, stranded, NR
        exit 1
    }

    if (ledger_overflow > 0) {
        printf "STRAND_CENSUS: INCOMPLETE ledger_overflow=%d threads_saved_blocked=%d stranded=%d lines=%d\n", ledger_overflow, saved, stranded, NR
        exit 3
    }

    # A stale reading is checked only on the clean arm, deliberately: its whole
    # purpose is to stop a PASS being issued off a snapshot that stopped being
    # refreshed before the boot finished.
    if (age_measured && age_ms > stale_limit_ms) {
        printf "strand census: the newest cadence snapshot was %d ms old at the completion marker, over the %d ms bound; the census cadence stopped before the userspace phase ended, so stranded=0 here is not evidence of anything\n", age_ms, stale_limit_ms
        printf "STRAND_CENSUS: STALE age_ms=%d bound_ms=%d threads_saved_blocked=%d stranded=%d lines=%d\n", age_ms, stale_limit_ms, saved, stranded, NR
        exit 4
    }

    # LAST: a capture that carries the completion marker but no valid snapshot
    # after it is TRUNCATED, not unmeasurable -- the age assertion has 0
    # snapshots to run on, so the bound cannot be applied and a PASS here would
    # have no evidence under it (finding R4-5). Reached on an otherwise clean,
    # unoverflowed, unstale reading, so it cannot mask a red one.
    if (truncated_at_marker) {
        printf "strand census: census incomplete at completion marker: the capture carries USERSPACE TEST COMPLETE but no valid snapshot follows it in that capture, so the age at the marker cannot be measured and the newest (clean) reading seq=%d carries no freshness evidence\n", best_seq > "/dev/stderr"
        exit 2
    }

    printf "STRAND_CENSUS: threads_saved_blocked=%d stranded=%d lines=%d\n", saved, stranded, NR
    exit 0
}
' "$@"
