#!/usr/bin/env bash
# x86 strand census: consume the kernel's newest dispatch-ledger snapshot.
#
# #568 round 2 found a blocking poll thread that was saved and then stopped
# running. #775 removed the formatted save/restore records from the dispatch
# path and put the same state transitions in a fixed atomic ledger. Under R125,
# the scheduler's idle loop and the loopback pump emit that ledger at most once
# per second from ordinary thread context with interrupts enabled; the
# completion path emits a final snapshot. The newest snapshot in the capture is
# the source of truth, so a wedged userspace thread does not have to resume or
# exit to make its state observable.
# claim-lint:ok: #775 ruling R125 defines the replacement source and cadence.
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
#     passed twice -- are collapsed before that check.
#     claim-lint:ok: the ordering and two-boot arms are covered by
#     tests/x86_gate_verdict_test.rs.
#   * Every marker is validated. Malformed ones are counted and skipped, so one
#     truncated trailing marker cannot discard an otherwise readable red
#     reading; the highest-seq VALID snapshot still decides.
#     claim-lint:ok: both truncation shapes are covered by
#     tests/x86_gate_verdict_test.rs.
#
# WHAT THE VERDICT DOES AND DOES NOT SAY
#   A snapshot is the ledger's state at one instant. A listed thread was saved
#   blocked and had not been restored AS OF THAT SNAPSHOT; it may have resumed
#   afterwards. The emitted sentence says exactly that, and the snapshot's `seq`
#   and `tick` are printed with it. Cadence holds only while the idle loop
#   runs: a wedge that spins a CPU instead of idling leaves the newest
#   snapshot stale, and the capture carries no end-of-boot timestamp, so how old
#   the reading was when the boot ended is not derivable from the log. The
#   observed inter-snapshot gaps are printed for that reason.
#
# Usage:  scripts/x86-strand-census.sh <serial-log> [<serial-log> ...]
# Output: one line per listed stranded thread, a provenance/cadence line,
#         overflow diagnostics if present, then a STRAND_CENSUS summary line.
# Exit:   0 when the highest-seq valid snapshot says stranded=0;
#         1 when it says stranded>0;
#         2 when no valid snapshot exists, or the inputs carry snapshots from
#           more than one boot;
#         3 when the kernel ledger overflowed, so the snapshot is incomplete and
#           carries no verdict either way (R134 item 2: not a clean census).
# claim-lint:ok: the 4 exit classes are covered by tests/x86_gate_verdict_test.rs.

set -euo pipefail

if [[ $# -eq 0 ]]; then
    echo "usage: $0 <serial-log> [<serial-log> ...]" >&2
    exit 2
fi

for serial_log in "$@"; do
    [[ -r "$serial_log" ]] || { echo "strand census: serial log is not readable: $serial_log" >&2; exit 2; }
done

cat -- "$@" | awk '
BEGIN {
    census_re = "\\[DISPATCH_STRAND_CENSUS:[^]]*\\]"
    valid_re = "^\\[DISPATCH_STRAND_CENSUS:seq=[0-9]+:tick=[0-9]+:ms=[0-9]+:saved=[0-9]+:stranded=[0-9]+:tids=(-|[0-9]+(,[0-9]+)*):tid_overflow=[0-9]+:ledger_overflow=[0-9]+\\]$"
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
        if (seq > best_seq) { best_seq = seq; best = marker }
    }
}
END {
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

    if (ledger_overflow > 0) {
        printf "strand census: kernel ledger overflowed (%d event(s)); the snapshot is incomplete and carries no verdict\n", ledger_overflow
        printf "STRAND_CENSUS: INCOMPLETE ledger_overflow=%d threads_saved_blocked=%d stranded=%d lines=%d\n", ledger_overflow, saved, stranded, NR
        exit 3
    }

    printf "STRAND_CENSUS: threads_saved_blocked=%d stranded=%d lines=%d\n", saved, stranded, NR
    exit (stranded > 0) ? 1 : 0
}
'
