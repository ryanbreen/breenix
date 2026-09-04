#!/usr/bin/env bash
# x86 strand census: consume the kernel's end-of-test dispatch census.
#
# #568 round 2 review turned up a failure the gate could not see. When the
# blocking `poll()` lost its wake, the polling thread was saved blocked and
# never scheduled again.  The old host census recovered that fact from the
# formatted save/restore records emitted by the context-switch path.  #775
# removes those interrupt-path records; the kernel now maintains the same
# save/restore/exit state in a fixed atomic ledger and emits one compact line
# after the userspace test battery ends.
# claim-lint:ok: #568 field failure and #775 migration definition.
#
# The rule is exactly one fact, taken from the kernel's own bookkeeping:
#
#   a thread which has ever been saved blocked, whose LAST save/restore event is
#   save, and which never reported an exit, was never resumed.
# claim-lint:ok: #775 preserves scripts/x86-strand-census.sh's pre-migration predicate.
#
# Threads that exit are excluded.  An overflow is a data error, not a green:
# without a ledger slot the gate cannot apply the definition honestly.
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

line_count=$(cat -- "$@" | wc -l | tr -d ' ')
census_lines=$(grep -hE '\[DISPATCH_STRAND_CENSUS:threads_saved_blocked=[0-9]+:stranded=[0-9]+:overflow=[0-9]+\]' -- "$@" || true)
census_count=$(printf '%s\n' "$census_lines" | awk 'NF { count++ } END { print count + 0 }')

if [[ "$census_count" -ne 1 ]]; then
    echo "strand census: expected exactly one DISPATCH_STRAND_CENSUS line, found $census_count" >&2
    exit 2
fi

threads_saved_blocked=$(printf '%s\n' "$census_lines" | sed -E 's/.*threads_saved_blocked=([0-9]+).*/\1/')
stranded=$(printf '%s\n' "$census_lines" | sed -E 's/.*:stranded=([0-9]+).*/\1/')
overflow=$(printf '%s\n' "$census_lines" | sed -E 's/.*:overflow=([0-9]+).*/\1/')

if [[ "$overflow" -ne 0 ]]; then
    echo "strand census: kernel ledger overflowed ($overflow event(s)); result is incomplete" >&2
    exit 2
fi

printf 'STRAND_CENSUS: threads_saved_blocked=%d stranded=%d lines=%d\n' \
    "$threads_saved_blocked" "$stranded" "$line_count"
[[ "$stranded" -eq 0 ]]
