#!/usr/bin/env bash
# Validate an x86 userspace run. The caller must set EXPECTED_EXITS to the
# expected number of userspace process exits for the selected boot profile.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALLOWLIST_PATH="$SCRIPT_DIR/x86-gate-allowlist.txt"

fail() {
    echo "x86 userspace gate: FAIL - $1"
    exit 1
}

if [[ $# -eq 0 ]]; then
    fail "usage: EXPECTED_EXITS=<count> $0 <serial-log> [<serial-log> ...]"
fi

if [[ ! "${EXPECTED_EXITS:-}" =~ ^[0-9]+$ ]] || (( 10#$EXPECTED_EXITS < 1 )); then
    fail "EXPECTED_EXITS must be set to the expected number of userspace process exits for this profile"
fi
expected_exits=$((10#$EXPECTED_EXITS))

for serial_log in "$@"; do
    [[ -r "$serial_log" ]] || fail "serial log is not readable: $serial_log"
done

[[ -r "$ALLOWLIST_PATH" ]] || fail "allowlist is not readable: $ALLOWLIST_PATH"

# Run the strand census first. The kernel emits a ledger snapshot from the
# scheduler's idle loop and from the loopback pump, at most once per second, so
# a saved-blocked thread can be NAMED even when that userspace thread never runs
# again. The consumer judges the highest-seq snapshot because it carries the
# newest ledger state, and the completion path emits a final one.
# claim-lint:ok: the 3 emission sites are pinned by
# tests/dispatch_strand_census_structure.rs.
#
# The emission is rate-LIMITED, not guaranteed-periodic: the idle loop runs
# whenever no thread is runnable, so a wedge that idles publishes at cadence,
# while a wedge that spins a CPU can stop the cadence and leave the newest
# snapshot stale. The census prints the observed gaps for that reason, and
# reports what the snapshot supports -- "not restored as of the latest snapshot"
# -- not "never restored".
# claim-lint:ok: #775 ruling R134 defines the idle-loop and pump sources; the
# cadence and its failure mode are measured in
# docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md.
#
# No snapshot means the kernel never reached the heartbeat, or failed before
# its first emission. That is census unavailability, not evidence of a strand:
# continue so the existing ordered checks name the first observed cause. This
# preserves run-x86-gate.sh's #702-vs-strand distinction.
# claim-lint:ok: #775 ruling R125 defines rc=2 as census unavailable.
#
# rc=3 is an OVERFLOWED ledger: the snapshot is incomplete, so `stranded=0` in
# it is not evidence of anything. It is reported loudly and treated as census
# unavailability -- never as a clean census.
# claim-lint:ok: #775 ruling R134 item 2 forbids passing on an overflowed ledger.
strand_output=""
strand_rc=0
strand_output="$("$SCRIPT_DIR/x86-strand-census.sh" "$@" 2>&1)" || strand_rc=$?
printf '%s\n' "$strand_output"
case "$strand_rc" in
    0) ;;
    1) fail "a thread was saved blocked in a kernel wait and was still not restored at the latest census snapshot (see the strand census above)" ;;
    2) echo "x86 userspace gate: census unavailable; continuing with ordered first-cause checks" ;;
    3) echo "x86 userspace gate: STRAND CENSUS INCOMPLETE - the kernel ledger overflowed, so this boot has NO usable strand evidence in either direction; continuing with ordered first-cause checks" ;;
    *) fail "strand census returned unexpected status $strand_rc" ;;
esac

# #693: the kernel's own lost-readiness report, checked before the terminal
# markers for the same reason the strand census is: a boot that lost a readiness
# publication should be named by that, not by whatever a program downstream of
# it then failed to print.
#
# This is a failure check with no matching presence check, deliberately. The
# ordinary companion line [POLL_TCP_TIMEOUT] is only emitted by a blocking poll
# of at least 120 ms on a connected TCP fd, and whether any x86 boot profile
# contains one depends on #697 (see kernel/src/main.rs). Requiring it here would
# assert a property of the profile that this script cannot know; requiring the
# ABSENCE of a contradiction is sound on any profile, including one that does
# not poll a TCP fd.
if grep -hFq '[POLL_TCP_READY_LOST]' "$@"; then
    printf '%s\n' "$(grep -hF '[POLL_TCP_READY_LOST]' "$@" | head -1)"
    fail "the kernel reported a lost TCP readiness publication (#693): a blocking poll() returned without POLLIN although bytes were published inside its window and are still buffered"
fi

if ! grep -hFq 'USERSPACE TEST COMPLETE' "$@"; then
    fail "USERSPACE TEST COMPLETE was absent; boot did not finish"
fi

if ! grep -hq 'TEST_TALLY:' "$@"; then
    fail "TEST_TALLY was absent; kernel is stale or userspace did not finish"
fi

tally_line="$(grep -h 'TEST_TALLY:' "$@" | tail -n 1)"
parsed_tally="$(
    printf '%s\n' "$tally_line" \
        | sed -n 's/.*TEST_TALLY: exited=\([0-9][0-9]*\) nonzero=\([0-9][0-9]*\) failed=\[\([^]]*\)\].*/\1|\2|\3/p'
)"
[[ -n "$parsed_tally" ]] || fail "last TEST_TALLY line is malformed: $tally_line"

IFS='|' read -r exited_text nonzero_text failed_field <<< "$parsed_tally"
exited=$((10#$exited_text))
nonzero=$((10#$nonzero_text))
(( nonzero <= exited )) || fail "tally reports nonzero=$nonzero greater than exited=$exited"
(( exited >= expected_exits )) \
    || fail "tally reports exited=$exited below the expected floor EXPECTED_EXITS=$expected_exits; a test program never ran or never exited"

pass_marker=false
failure_marker=false
if grep -hFq '🏁 TEST RUNNER: All tests passed' "$@"; then
    pass_marker=true
fi
if grep -hFq 'TEST RUNNER: FAILED' "$@"; then
    failure_marker=true
fi

if (( nonzero == 0 )); then
    $pass_marker || fail "nonzero=0 but the all-tests-passed marker is absent"
    ! $failure_marker || fail "nonzero=0 but a TEST RUNNER: FAILED marker is present"
else
    $failure_marker || fail "nonzero=$nonzero but the TEST RUNNER: FAILED marker is absent"
    ! $pass_marker || fail "nonzero=$nonzero but the all-tests-passed marker is present"
fi

failure_names=()
if [[ -n "$failed_field" ]]; then
    [[ "$failed_field" != *...* ]] \
        || fail "failure list was truncated; not every failing process can be reviewed"

    IFS=',' read -r -a failure_items <<< "$failed_field"
    failure_item_count=${#failure_items[@]}
    for ((index = 0; index < failure_item_count; index++)); do
        failure_item="${failure_items[$index]}"
        if [[ ! "$failure_item" =~ ^([^,:[:space:]]+):(-?[0-9]+)$ ]]; then
            fail "malformed failure entry in last tally: $failure_item"
        fi
        failure_names+=("${BASH_REMATCH[1]}")
    done
fi

failure_count=${#failure_names[@]}
(( failure_count == nonzero )) \
    || fail "nonzero=$nonzero but failed=[...] contains $failure_count named failures"

trim_whitespace() {
    local value="$1"
    value="${value#"${value%%[![:space:]]*}"}"
    value="${value%"${value##*[![:space:]]}"}"
    printf '%s' "$value"
}

allowlist_names=()
while IFS= read -r raw_line || [[ -n "$raw_line" ]]; do
    line="$(trim_whitespace "$raw_line")"
    [[ -z "$line" || "$line" == \#* ]] && continue
    [[ "$line" == *\#* ]] \
        || fail "allowlist entry lacks required # <issue reference>: $line"

    allowlist_name="$(trim_whitespace "${line%%#*}")"
    issue_reference="$(trim_whitespace "${line#*#}")"
    [[ -n "$allowlist_name" && -n "$issue_reference" ]] \
        || fail "malformed allowlist entry; expected <test-name> # <issue reference>: $line"
    [[ "$allowlist_name" != *[[:space:],:]* ]] \
        || fail "allowlist test name contains unsupported whitespace or punctuation: $allowlist_name"

    allowlist_count=${#allowlist_names[@]}
    for ((index = 0; index < allowlist_count; index++)); do
        [[ "${allowlist_names[$index]}" != "$allowlist_name" ]] \
            || fail "duplicate allowlist entry: $allowlist_name"
    done
    allowlist_names+=("$allowlist_name")
done < "$ALLOWLIST_PATH"

allowlist_count=${#allowlist_names[@]}
for ((failure_index = 0; failure_index < failure_count; failure_index++)); do
    failure_name="${failure_names[$failure_index]}"
    found=false
    for ((allowlist_index = 0; allowlist_index < allowlist_count; allowlist_index++)); do
        if [[ "${allowlist_names[$allowlist_index]}" == "$failure_name" ]]; then
            found=true
            break
        fi
    done
    $found || fail "failing process is not allowlisted: $failure_name"
done

for ((allowlist_index = 0; allowlist_index < allowlist_count; allowlist_index++)); do
    allowlist_name="${allowlist_names[$allowlist_index]}"
    found=false
    for ((failure_index = 0; failure_index < failure_count; failure_index++)); do
        if [[ "${failure_names[$failure_index]}" == "$allowlist_name" ]]; then
            found=true
            break
        fi
    done
    $found || fail "allowlisted process is no longer failing; remove its entry: $allowlist_name"
done

echo "x86 userspace gate: PASS - exited=$exited expected>=$expected_exits nonzero=$nonzero allowlist=$allowlist_count"
