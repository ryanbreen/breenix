#!/usr/bin/env bash
# x86 strand census: consume the kernel's latest dispatch-ledger snapshot.
#
# #568 round 2 found a blocking poll thread that was saved and then stopped
# running. #775 removed the formatted save/restore records from the dispatch
# path and put the same state transitions in a fixed atomic ledger. Under R125,
# existing idle housekeeping emits that ledger about once per second after
# enable_and_hlt returns with interrupts enabled; the completion path emits a
# final snapshot. The last snapshot in the serial is the source of truth, so a
# wedged userspace thread does not have to resume or exit to make its state
# observable.
# claim-lint:ok: #775 ruling R125 defines the replacement source and cadence.
#
# Each snapshot carries the distinct ever-saved count, the stranded count, and
# up to 16 stranded TIDs. This script recovers their names from the existing
# `Added thread N <name>` records. A bounded-list overflow is reported beside
# the named prefix; a ledger overflow is reported as an incomplete observation.
#
# Usage:  scripts/x86-strand-census.sh <serial-log> [<serial-log> ...]
# Output: one line per listed stranded thread, overflow diagnostics if present,
#         then a STRAND_CENSUS summary line.
# Exit:   0 when the last snapshot says stranded=0;
#         1 when the last snapshot says stranded>0;
#         2 when no usable census snapshot exists.

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
    valid_re = "^\\[DISPATCH_STRAND_CENSUS:saved=[0-9]+:stranded=[0-9]+:tids=(-|[0-9]+(,[0-9]+)*):tid_overflow=[0-9]+:ledger_overflow=[0-9]+\\]$"
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
        latest = substr(rest, RSTART, RLENGTH)
        snapshots++
        rest = substr(rest, RSTART + RLENGTH)
    }
}
END {
    if (snapshots == 0) {
        print "strand census: no DISPATCH_STRAND_CENSUS line found" > "/dev/stderr"
        exit 2
    }
    if (latest !~ valid_re) {
        print "strand census: last DISPATCH_STRAND_CENSUS line is malformed: " latest > "/dev/stderr"
        exit 2
    }

    data = latest
    sub(/^\[/, "", data); sub(/\]$/, "", data)
    field_count = split(data, fields, ":")
    for (field = 2; field <= field_count; field++) {
        equals = index(fields[field], "=")
        key = substr(fields[field], 1, equals - 1)
        value[key] = substr(fields[field], equals + 1)
    }

    listed = 0
    if (value["tids"] != "-") {
        listed = split(value["tids"], stranded_tids, ",")
    }
    if (listed + value["tid_overflow"] != value["stranded"]) {
        print "strand census: last snapshot has an inconsistent stranded TID list" > "/dev/stderr"
        exit 2
    }

    for (i = 1; i <= listed; i++) {
        tid = stranded_tids[i]
        name = (tid in names) ? names[tid] : "?"
        printf "strand census: thread %s (%s) saved blocked and never restored\n", tid, name
    }
    if (value["tid_overflow"] > 0) {
        printf "strand census: %d additional stranded thread(s) omitted by the kernel TID-list bound\n", value["tid_overflow"]
    }
    if (value["ledger_overflow"] > 0) {
        printf "strand census: kernel ledger overflowed (%d event(s)); snapshot is incomplete\n", value["ledger_overflow"]
    }

    printf "STRAND_CENSUS: threads_saved_blocked=%d stranded=%d lines=%d\n", value["saved"], value["stranded"], NR
    exit (value["stranded"] > 0) ? 1 : 0
}
'
