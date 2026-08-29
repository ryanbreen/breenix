#!/usr/bin/env bash
# x86 strand census: name every thread that was saved blocked in a kernel wait
# and never restored.
#
# #568 round 2 review turned up a failure the gate could not see. When the
# blocking `poll()` lost its wake, the polling thread was saved blocked and
# never scheduled again -- so it emitted no verdict, no non-zero exit and no
# marker. The boot went on for another 1600 lines and the only trace was an
# ABSENT oracle line. A gate that only reads emitted markers cannot fail on a
# thread that was silenced; this census reads the context-switch record itself,
# which is written whether or not the thread ever speaks again.
#
# The rule is exactly one fact, taken from the kernel's own bookkeeping:
#
#   a thread whose LAST "Saved kernel context for blocked thread N" is not
#   followed by any "Restored kernel context for thread N", and which never
#   reported an exit, was never resumed.
#
# Threads that exit are excluded: exiting is a legitimate way never to be
# restored. Nothing else is excluded -- there is no name list here, because a
# name list is what goes stale.
#
# Usage:  scripts/x86-strand-census.sh <serial-log> [<serial-log> ...]
# Output: one line per stranded thread, then a STRAND_CENSUS summary line.
# Exit:   0 when no thread was stranded, 1 when at least one was.

set -euo pipefail

if [[ $# -eq 0 ]]; then
    echo "usage: $0 <serial-log> [<serial-log> ...]" >&2
    exit 2
fi

for serial_log in "$@"; do
    [[ -r "$serial_log" ]] || { echo "strand census: serial log is not readable: $serial_log" >&2; exit 2; }
done

cat -- "$@" | awk '
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
/Saved kernel context for blocked thread [0-9]+/ {
    tid = $0; sub(/.*Saved kernel context for blocked thread /, "", tid); sub(/[^0-9].*/, "", tid)
    saved[tid] = NR
}
/Restored kernel context for thread [0-9]+/ {
    tid = $0; sub(/.*Restored kernel context for thread /, "", tid); sub(/[^0-9].*/, "", tid)
    restored[tid] = NR
}
/\(thread [0-9]+\) exited with code/ {
    tid = $0; sub(/.*\(thread /, "", tid); sub(/\).*/, "", tid)
    exited[tid] = 1
}
END {
    count = 0
    saved_n = 0
    for (tid in saved) {
        saved_n++
        if (tid in exited) continue
        r = (tid in restored) ? restored[tid] : -1
        if (r < saved[tid]) {
            nm = (tid in names) ? names[tid] : "?"
            printf "strand census: thread %s (%s) saved blocked at line %d and never restored (%d further lines followed)\n", tid, nm, saved[tid], NR - saved[tid]
            count++
        }
    }
    printf "STRAND_CENSUS: threads_saved_blocked=%d stranded=%d lines=%d\n", saved_n, count, NR
    exit (count > 0) ? 1 : 0
}
'
