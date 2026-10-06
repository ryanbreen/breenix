#!/usr/bin/env bash
# Validate an x86 userspace run. The caller must set EXPECTED_EXITS to the
# expected number of userspace process exits for the selected boot profile.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

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

# Publish the parseable nonzero records before the ordered first-cause checks.
exit_contracts_ok=true
python3 "$SCRIPT_DIR/x86-gate-exits.py" "$@" || exit_contracts_ok=false

python3 "$SCRIPT_DIR/score-softirq-deferral.py" "$@" || exit $?

# Run the strand census first. The kernel emits a ledger snapshot from three
# rate-limited contexts -- the scheduler's idle loop, the loopback pump and the
# `kstrandd` census kthread -- at most once per second between them, so a
# saved-blocked thread can be NAMED even when that userspace thread never runs
# again. The consumer judges the highest-seq snapshot because it carries the
# newest ledger state, and the completion path emits a final one.
# claim-lint:ok: the 4 emission sites are pinned by
# tests/dispatch_strand_census_structure.rs.
#
# The emission is rate-LIMITED, not guaranteed-periodic. `kstrandd` sleeps on
# the scheduler timer, so the cadence no longer needs the CPU to idle, but
# anything that stops that kthread running still leaves the newest snapshot
# stale. The census prints the observed gaps and the age of the newest cadence
# snapshot at the completion marker for that reason, and reports what the
# snapshot supports -- "not restored as of the latest snapshot" -- not
# "never restored".
# claim-lint:ok: #775 rulings R134 and R137 define the three cadence sources;
# the cadence and its failure mode are measured in
# docs/planning/green-program/sockets/775-CENSUS-EQUIVALENCE-2026-09-04.md.
#
# rc=4 is a STALE clean reading: stranded=0, but the snapshot it came from was
# already older than the census's bound when the userspace phase ended. That is
# not a pass, because the ledger stopped being published before the boot
# finished. The bound is DERIVED from #766's measured x86 wake-to-dispatch
# overrun (max 10318 ms over 324 trials) plus margin, not chosen; it tightens
# when #766 lands. scripts/x86-strand-census.sh's AGE header carries the
# derivation and the disclosed cost, and its `stale_limit_ms` assignment is the
# ONLY copy of the value: the sentence below reads the number back out of the
# census's own STALE summary line rather than restating it, so this script and
# the census can never disagree about which bound was applied -- finding F4.
# claim-lint:ok: #775 ruling R137 defines the age bound and R140 derives it
# from the distribution in
# docs/planning/green-program/sockets/693-RCA-2026-09-02.md.
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
#
# rc=0 with NO summary line is a census that did not RUN, and it used to read as
# a clean one. `x86-strand-census.sh` prints a `STRAND_CENSUS:` line on each of
# its exit-0 paths, so an exit 0 without one means the tool did not reach its
# END block. Round 5 produced that state by accident -- an apostrophe inside a
# comment in the single-quoted awk program terminates the program string, and
# the resulting shell printed nothing and exited 0. The gate scored 6 of its 19
# verdict tests green against that broken tool, this one among them, so the
# check is not hypothetical.
# claim-lint:ok: the arm is covered by
# a_census_that_exits_zero_without_a_summary_line_is_not_a_pass in
# tests/x86_gate_verdict_test.rs.
case "$strand_rc" in
    0)
        case "$strand_output" in
            *"STRAND_CENSUS:"*) ;;
            *) fail "the strand census exited 0 without printing a STRAND_CENSUS summary line, so this boot carries no census reading at all: the tool did not run to completion" ;;
        esac
        ;;
    1) fail "a thread was saved blocked in a kernel wait and was still not restored at the latest census snapshot (see the strand census above)" ;;
    2) echo "x86 userspace gate: census unavailable; continuing with ordered first-cause checks" ;;
    3) echo "x86 userspace gate: STRAND CENSUS INCOMPLETE - the kernel ledger overflowed, so this boot has NO usable strand evidence in either direction; continuing with ordered first-cause checks" ;;
    4)
        stale_summary="$(printf '%s\n' "$strand_output" | grep -F 'STRAND_CENSUS: STALE ' | tail -n 1 || true)"
        stale_bound="${stale_summary##*bound_ms=}"
        stale_bound="${stale_bound%% *}"
        [[ "$stale_bound" =~ ^[0-9]+$ ]] \
            || fail "the strand census reported a stale reading (rc=4) but printed no parseable bound_ms, so the bound it applied cannot be named: ${stale_summary:-<no STRAND_CENSUS: STALE line>}"
        fail "the strand census read stranded=0 from a snapshot that was already more than $stale_bound ms stale at the completion marker, so the clean reading is stale rather than clean (see the age line above)"
        ;;
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

$exit_contracts_ok || fail "one or more process exits violate their asserted contracts (see every TEST_EXIT above)"

echo "x86 userspace gate: PASS - exited=$exited expected>=$expected_exits nonzero=$nonzero (all statuses match their contracts)"
