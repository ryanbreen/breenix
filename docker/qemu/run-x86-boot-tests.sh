#!/bin/bash
# Build and execute the x86_64 frame/page-table custody injection gates.
# This script deliberately does not treat [BOOT_TESTS:PASS] as test evidence:
# advance_stage_marker_only
# emits it unconditionally alongside [TESTS_COMPLETE:0/0]. The removed
# KERNEL_POST_TESTS_COMPLETE marker is likewise never used as a gate.
# The 900-second poll bound allows the x86 boot-test registry to run after the
# userspace programs; a shorter bound scores a slow-but-healthy boot as failed.
# http_test's live external fetches are bounded in-process by a receive
# deadline. A connect-phase failure prints an explicit SKIP marker and the boot
# continues; a mid-stream stall is an honest FAIL that appears in the tally as
# a nonzero http_test exit. A quiet boot with no marker remains a gate failure.
# This gate never retries a hung run: a blanket retry could swallow exactly the
# recv-wake regression this gate exists to catch.

set -euo pipefail
# errtrace: without this, the ERR trap below is not inherited into shell
# functions, and report_gate_failure is itself invoked from that trap.
set -E

# Every assertion below this point (the explicit passed-flag check and the ~40
# bare `test ... -eq N` marker-count assertions) is deliberately a
# set -e abort point: on a genuine boot regression, the assertion SHOULD
# kill the script. What must never happen is a silent kill: `set -e`
# does not print anything on its own, so without this trap the script
# dies with no verdict/FAIL text and no serial pointer, and the only way
# to learn why is to dig through the raw serial by hand. This trap is the
# fix: it fires on every uncaught nonzero exit under set -e (bash routes
# them through ERR the same way set -e decides to abort), prints an
# explicit FAIL line naming the failing command and line, tails the
# current run's serial output for diagnosis, and then exits nonzero
# deliberately so the caller still sees a real failure.
report_gate_failure() {
    local exit_code=$?
    local line_no="$1"
    local failing_cmd="$2"
    # #717: many assertions below are shaped `test "$(cmd | awk ...)" -eq N`
    # (or `VAR=$(cmd | awk ...)` feeding one). That command substitution
    # runs in its own subshell; under `set -o pipefail`, a zero-match `grep`
    # earlier in such a pipeline fails the whole pipeline even though the
    # final command (commonly `awk`) is fine, so this ERR trap fires INSIDE
    # that subshell first, misattributing the failure to the pipeline's
    # last command. `exit` there only ends the subshell, not the script --
    # the parent's `test`/assignment then receives this handler's own
    # printed text (or nothing) as its "value" instead of a real count,
    # which always fails that parent statement's own check too, re-firing
    # this trap a SECOND time at the top level with a different, but this
    # time correctly-attributed, $LINENO/failing-command pair. A plain
    # shell-variable guard can't dedupe this: the subshell's variable
    # changes never propagate back to the parent. `$BASH_SUBSHELL` does
    # survive that boundary as a readable fact (it counts subshell nesting
    # depth), so use it: stay silent when running inside a subshell -- the
    # top-level re-fire this always triggers is the one worth reporting --
    # and print (and exit the whole script) only from depth 0.
    if [ "$BASH_SUBSHELL" -gt 0 ]; then
        exit "$exit_code"
    fi
    echo "x86 frame-custody gate${i:+ run $i}: FAIL (set -e abort at ${BASH_SOURCE[0]}:${line_no}, exit ${exit_code})"
    echo "  failing command: ${failing_cmd}"
    if [ -n "${OUTPUT_DIR:-}" ] && compgen -G "$OUTPUT_DIR/serial_*.txt" >/dev/null 2>&1; then
        echo "--- serial tail (last 200 lines per file, $OUTPUT_DIR) ---"
        tail -n 200 "$OUTPUT_DIR"/serial_*.txt
    fi
    exit "$exit_code"
}
trap 'report_gate_failure "$LINENO" "$BASH_COMMAND"' ERR

COUNT="${1:-1}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BREENIX_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
# #826/#834/#865/R181: this gate's qemu-system-x86_64 boot(s) run behind the
# host-wide lock in lib/qemu-host-lock.sh (one lock domain per QEMU binary --
# see that file's own ARCHITECTURE AWARENESS comment), so at most one
# qemu-system-x86_64 is active on this host for each boot's duration. #865
# is this lock's own report of the failure mode on the beast x86 host:
# several TCG lanes running concurrently starve each other and hit the
# gate's own timing ceilings on a healthy guest.
# shellcheck source=lib/qemu-host-lock.sh
source "$SCRIPT_DIR/lib/qemu-host-lock.sh"
# #827/#865: per-boot host-side facts (wall-clock window, host QEMU count
# and load average at start/kill, QEMU's own CPU time, the guest's last
# heartbeat, and which bound ended the boot) -- see that file's own header
# for why a starved boot and a wedged boot could not be told apart before.
# shellcheck source=lib/gate-boot-facts.sh
source "$SCRIPT_DIR/lib/gate-boot-facts.sh"
# #797: concurrent lanes sharing one host (e.g. the beast Incus container) each
# invoking this script hardcode the identical /tmp/breenix_x86_boot_tests_$i
# path, so one lane's rm -rf/mkdir can clobber another lane's in-flight run and
# a poll loop can read back the wrong lane's serial as its own. Defaulting to
# /tmp keeps every existing caller byte-identical; a concurrent-lane launcher
# sets this to a per-clone directory instead.
# claim-lint:ok: #797, diff-empty against origin/main -- see
# docs/planning/green-program/gates/GATE-TMP-BASEDIR-2026-09-05.md
BREENIX_GATE_TMP="${BREENIX_GATE_TMP:-/tmp}"
# Must be absolute: OUTPUT_DIR (below, post-cd) is built from this value
# after `cd "$BREENIX_ROOT"`, but the ERR trap above is installed before that
# cd and can read OUTPUT_DIR pre-cd on an early failure -- a relative value
# would resolve differently in each place (review finding F6 on #797).
#
# The rejection below is `echo` + bare `false` rather than a bare `exit 1`
# (#802/#805 idiom, widened to this gate): the ERR trap is already installed
# above, so a `false` here fires it and the gate's own
# "x86 frame-custody gate: FAIL (...)" line prints before the process ends,
# instead of a silent `exit`, which the trap does not catch.
case "$BREENIX_GATE_TMP" in
    /*) ;;
    *) echo "x86 frame-custody gate preflight: BREENIX_GATE_TMP must be an absolute path, got: $BREENIX_GATE_TMP" >&2
       false ;;
esac
# The x86 serial console carries the scheduler's single-character trace stream
# on the same port as kernel and userspace output, so any marker line can carry
# a prefix. The markers are self-delimiting (`[...]` or a unique sentence), so
# a substring match is still exact; do not re-anchor these.
FRAME_CUSTODY_PATTERN='\[FRAME_CUSTODY_COUNTERS:x86:double=1:stale=1:never=1:untracked=1:duplicate=3:contended=[1-9][0-9]*\]'
# The x86 failed-exec release oracle records one three-table hierarchy and retires it:
# recorded rises by the hierarchy, returned by the hierarchy plus its root, and undecided deliberately does not move.
PT_CUSTODY_LITERAL='[PT_CUSTODY_COUNTERS:x86:recorded=14:no_proof=0:no_arch=0:terminated=1:undecided=1:retired=2:returned=14:lost=0:requeued=0]'
# One kernel-stack slot returns per child death across this 64-child cohort, so kstack_returns == children.
PT_COHORT_LITERAL='[PT_RETIRE_COHORT:x86:children=64:retired=65:returned=642:recorded=577:lost=0:no_arch=0:undecided=0:mid_retire=0:kstack_returns=64:balance=0]'
PT_EXEC_COHORT_LITERAL='[PT_EXEC_COHORT:x86:children=16:superseded=3:roots=64:returned=640:recorded=576:lost=0:leaf_recorded=192:leaf_released=192:leaf_returned=192:custody_refused=0:decref_unregistered=0:undecided=0:mid_retire=0:no_arch=0:balance=0]' # The returned and recorded table-frame fields are pinned from the measured run.
# KernelStack::drop now unmaps the x86 stack VA range and releases its 128 frames; exactly one stack is allocated and dropped in this window, so 149 - 128 * 1 = 21.
# kstack_frames_released proves that arithmetic in-kernel: the oracle asserts 149 - stack_residual == kstack_frames_released, preventing silent drift.
EXEC_DETACH_ORACLE_LITERAL='[EXEC_DETACH_ORACLE:x86:bodies=2:fail_preserved=2:sibling_refused=2:success_detached=2:fresh_root=2:tgid_self=2:custody_balance=0:leaf_residual=16:stack_residual=21:kstack_frames_released=128:old_group_reached_pre=2:old_group_missed_post=2:self_group_reached_post=2]'
CLONE_ADMISSION_ORACLE_LITERAL='[CLONE_ADMISSION_ORACLE:x86:admitted=1:refused=2:creating_refused=1:published_admitted=2:balance=0]'
# Every field is a delta the oracle drives itself in the same run except
# reserved_collisions, which is the absolute boot-wide count of ordinary
# allocations that landed on the reserved init PID and must be zero.
# construct_residual is the counted frame residue of the two construction-failure arms read off a measured green run, and it is architecture-specific (4 on x86, 2 on aarch64) because the two page-table constructors record different table-frame counts.
INIT_DESIGNATION_ORACLE_LITERAL='[INIT_DESIGNATION_ORACLE:x86:construct_failed=2:construct_undecided=2:construct_residual=4:refused=4:accepted=1:published=1:retired=1:held_error_removals=1:reparented=1:reparent_skipped=1:ordinary_allocated=5:reserved_collisions=0:designation_balance=0]'
INIT_GROUP_REFUSAL_ORACLE_LITERAL='[INIT_GROUP_REFUSAL_ORACLE:x86:none_probes=3:none_refusals=0:init_refused=1:alias_refused=1:alias_pid_refused=0:nonit_probes=2:nonit_refusals=0:rows_delta=0:refusal_counter_delta=0:designation_residual=0:balance=0]'
# driven=2 proves both handoff seams ran; stage1/2 return, wake, and park fields
# expose D1/D2. stage3_elapsed_ok=1 proves the interval the oracle measured
# reached the full requested duration -- since #627 that interval is anchored
# to the same clock read the kernel used to compute the deadline, not to a
# later oracle-internal read, so this bit can no longer read 0 on a wait that
# was never actually short. arm_delay_us is that retired gap, kept visible.
# stage3_ret=ETIMEDOUT plus rescues=0 proves the backstop did not end this wait.
# stage3_elapsed_ms is the measured duration; residual/balance prove cleanup.
# claim-lint:ok: #627 -- provable by construction from program order (futex.rs
# reads base_ns before its deadline check; record_arm's own clock read comes
# after), not by boot sampling: see kernel/src/syscall/futex_oracle.rs::record_arm
# and validate_futex_oracle_record_arm_anchor in tests/teardown_structure.rs.
# This marker is emitted from a syscall while the scheduler trace stream is live, so its line can carry a prefix.
FUTEX_HANDOFF_ORACLE_PATTERN='\[FUTEX_HANDOFF_ORACLE:x86:driven=2:stage1_ret=EAGAIN:stage1_wake=0:stage1_parked=0:stage2_ret=0:stage2_wake=1:stage2_parked=0:stage3_ret=ETIMEDOUT:stage3_elapsed_ok=1:stage3_elapsed_ms=[0-9]+:arm_delay_us=[0-9]+:rescues=0:queue_residual=0:balance=0\]'
# Absolute frame counts are boot-state dependent, so pin every delta exactly,
# including the three-table recorded_pre hierarchy cost and computed tables_returned=4;
# the in-kernel oracle asserts used_after == used_before, and a skipped/cfg'd-out block fails this gate.
EXEC_FAILED_RELEASE_ORACLE_PATTERN='\[EXEC_FAILED_RELEASE_ORACLE:x86:used_before=[0-9]+:used_after=[0-9]+:recorded_pre=3:leaf_recorded=1:leaf_released=1:leaf_returned=1:tables_returned=4:roots_retired=1:undecided=0:live_refused=0\]'
EXEC_FAILED_RELEASE_PROD_LITERAL='[EXEC_FAILED_RELEASE_PROD:x86:plain_err=true:plain_kept=true:argv_err=true:argv_kept=true:name_kept=true:balance=0:undecided=0:mid_retire=0:lost=0:custody_refused=0:decref_unregistered=0:double=0:stale=0:untracked=0:root_slot_refused=0]'
# Creation/fork/slot/frame/refusal/classifier/balance fields are oracle-driven and exact.
# live_checks is nonzero because every allocation evaluates the guard; pub_pooled and pub_sched_owned are nonzero boot-wide totals whose exact values depend on process creation, while the oracle asserts they are equal and both publication residuals are zero.
# sched_publications is a nonzero boot-wide driver for sched_pm_held_production=0.
# frame_used_delta is boot-state dependent because of heap growth during the stress; the oracle asserts it is strictly less than 128 frames, one x86 kernel stack's worth.
KSTACK_OWNER_ORACLE_PATTERN='\[KSTACK_OWNER_ORACLE:x86:creation_rows=1000:creation_owned=1000:one_owner=1000:two_owner=0:zero_owner=0:fork_rows=2:fork_owned=2:slot_returns_exact_one=2:slot_alloc_delta=[1-9][0-9]*:slot_free_delta=[1-9][0-9]*:slot_balance=-?[0-9]+:cohort_enrolled=1000:cohort_returned=1000:cohort_double_return=0:foreign_alloc_delta=[0-9]+:foreign_returned=[0-9]+:frames_mapped_delta=[1-9][0-9]*:frames_released_delta=[1-9][0-9]*:frame_balance=-?[0-9]+:frame_used_delta=[0-9]+:frame_used_bounded=1:live_checks=[1-9][0-9]*:live_refusals_production=0:live_refusals_injected=1:drop_refused_live=0:pte_overwrite_refusals=0:pub_pooled=[1-9][0-9]*:pub_sched_owned=[1-9][0-9]*:pub_row_residual=0:pub_unowned=0:classifier_sched_owned=1:classifier_row_residual=1:classifier_unowned=1:classifier_not_pooled=1:sched_publications=[1-9][0-9]*:sched_pm_held_production=0:sched_pm_held_injected=1:reconciliation_diff=-?[0-9]+:reconciliation_skew_bound=[0-9]+:balance=0\]'
# P6a PR-2 gate extras (b)/(f)/(g). Every field is a delta the oracle drives
# itself inside one run, so the whole line is a literal: two fixture rows, one
# joined by retirement (retire_second) and one by the reap (reap_second), the
# gauge back at its entry value (resident_delta=0) and no tombstone left behind
# (tombstone_rows=0 is absolute, not a delta). This is the ONLY evidence that
# gate (g)'s retire-second arm ever executes on x86, and until this pin existed
# deleting run_x86_tombstone_join_gate() from main.rs left this gate green.
TOMBSTONE_JOIN_ORACLE_LITERAL='[TOMBSTONE_JOIN_ORACLE:x86:retire_second=1:reap_second=1:removed=2:resident_delta=0:tombstone_rows=0:PASS]'
# P6a PR-2 review finding B2, re-derived for #653 — the x86 retention claim,
# measured on production rows instead of on the oracle's fixture, in two samples
# that mean different things and are pinned separately.
#
# #653 is FIXED, and these pins are what the fix moved. The production
# deferred-reclaim claim used to be a bare boolean released only by the tail of
# the function that took it; the first timer preemption inside an owned pass
# discarded that release (x86's dispatcher restarts `idle_loop` rather than
# resuming the mid-drain continuation), so the claim latched true and every later
# drain refused for the rest of the boot. Eighteen receipts stayed queued, no
# retirement ever completed, and the four rows the live reaps claimed could never
# join. The claim window is now non-preemptible, so production reclamation runs
# to completion and BOTH samples below assert a drained system rather than
# narrating a strand.
#
# The join oracle stages two fixture rows of its own before any user process
# exists and removes both (TOMBSTONE_JOIN_ORACLE above reports
# resident_delta=0).
readonly TOMBSTONE_FIXTURE_REMOVALS=2
#
# PRODUCTION_REAPED_ROWS is NOT a literal (#697). It used to be pinned at 4,
# and 63e5f8e0 (PR #765, the #707 regression test tcp_cloexec_exec_test)
# added a RING3_SMOKE-roster process that forks a peer and waitpid()s it --
# one more production row reaped through the live `complete_wait` path --
# without touching this pin, which turned the conservation assertion below
# (CENSUS_RESIDENT + CENSUS_REMOVED - TOMBSTONE_FIXTURE_REMOVALS ==
# PRODUCTION_REAPED_ROWS) red on every boot: a frozen literal pinned the
# roster's SHAPE at a moment in time instead of the roster ITSELF, the
# #549/#551/#527-r1 census-not-literal lesson.
# claim-lint:ok: "red on every boot" is this round's own measurement, not a
# restatement of #697 (#697's own body records main as green here --
# "Main emits removed=6 exactly" -- and never mentions #731): both of the 2
# unpatched-main boots run this round at 509802e5 fail at this exact
# assertion (docs/planning/green-program/gates/serials/697-2026-09-02/
# main-unpatched/boot1-gate.txt:426-427, main-unpatched/boot2-gate.txt:407-408,
# both a FAIL at :548 on this same `test` line); the branch's minimum possible
# CENSUS_REMOVED value never satisfies the old literal, which is why every
# boot that reaches this assertion fails it.
#
# It is now derived from the roster kernel_main's FIRST RING3_SMOKE block
# launches: that block in kernel/src/main.rs loads its userspace test
# binaries by name between the "canonical list of test binaries is in
# boot::test_list::TEST_BINARIES" comment and the without_interrupts() call
# that creates them -- the get_test_binary("...") argument on each line in
# between is the roster. kernel_main has a SECOND, live RING3_SMOKE block
# further down (same #[cfg], a single get_test_binary("hello_time") call
# that creates smoke_hello_time) that sits past this awk window's end
# anchor and so is not scanned -- it is not dead, only outside the window,
# and missing it is safe: hello_time.rs is the same source the first
# block's roster already names, has 0 fork() call sites, and would
# contribute 0 to this pin whichever block created its process. Each
# roster name is a userspace/programs/src/<name>.rs
# source file, and every `fork()` call site introduced by `match` or by an
# assignment (`=`), through zero or more `mod::` path segments, is one child
# the program later reaps through a blocking waitpid() -- this recognises
# every fork idiom in the tree today (R98): `match fork() { ... }`
# (loopback_wake_test, bare), `let x = match fork() { ... }`
# (tcp_cloexec_exec_test, assign+match), `let x = process::fork();`
# (tcp_blocking_test, assign only, no roster program uses this shape yet),
# `match libbreenix::process::fork() { ... }` (bsh, fully-qualified path,
# no roster program uses this shape yet), and
# `match fork().unwrap_or_else(..) { ... }` (sigkill_teardown_test, chained
# call on the match scrutinee, no roster program uses this shape yet).
# loopback_wake_test forks its reader/peer/load/watchdog children (4 call
# sites) and tcp_cloexec_exec_test forks its one exec peer (1 call site),
# which is exactly how this pin was 4 before #765 added the second forking
# program to the roster and is 5 with it in. A roster addition that does not
# fork contributes 0 and changes nothing here; one that forks+waitpids, in
# any of the five idioms above, moves this count with it. The assertion
# stays live either way: a kernel defect that phantom-reaps an extra row, or
# fails to reap one the roster expects, still mismatches this derived total
# and fails it.
# claim-lint:ok: #697 -- "every ... call site ... is one child" is this
# derivation's own definition (it is what the grep loop immediately below
# counts, not an empirical claim about the kernel), and measured directly:
# 2 of 16 current roster files match (loopback_wake_test.rs: 4 sites,
# tcp_cloexec_exec_test.rs: 1 site), summing to the 5 this pin now derives;
# widening the pattern to the five idioms above leaves that sum unchanged
# (verified against every one of the 16 roster files plus, for the three
# newly-recognised idioms, tcp_blocking_test.rs=2, bsh.rs=3,
# sigkill_teardown_test.rs=4 -- none of which are on today's roster).
# `|| true`: under `set -o pipefail`, a roster-comment rename or removal in
# kernel/src/main.rs makes the middle `grep -oE` exit 1 with no match, which
# would otherwise abort the script AT THIS ASSIGNMENT -- the ERR trap still
# fires and still prints a FAIL, but it names the awk/grep/sed pipeline
# rather than the `test -n "$RING3_SMOKE_ROSTER"` assertion below that is
# actually making the claim (same shape as the PCI_CENSUS_LINE guard
# further down). Let the assignment succeed with an empty value and let the
# explicit `test -n` name the real failure.
RING3_SMOKE_ROSTER=$(awk \
    '/canonical list of test binaries is in boot::test_list::TEST_BINARIES/,/without_interrupts\(\|\| \{/' \
    "$BREENIX_ROOT/kernel/src/main.rs" \
    | grep -oE 'get_test_binary\("[a-zA-Z0-9_]+"\)' \
    | sed -E 's/.*"([a-zA-Z0-9_]+)".*/\1/') || true
test -n "$RING3_SMOKE_ROSTER"
PRODUCTION_REAPED_ROWS=0
for _ring3_smoke_name in $RING3_SMOKE_ROSTER; do
    _ring3_smoke_src="$BREENIX_ROOT/userspace/programs/src/${_ring3_smoke_name}.rs"
    test -f "$_ring3_smoke_src"
    # `|| true`: 14 of 16 current roster programs never fork (measured --
    # only loopback_wake_test.rs and tcp_cloexec_exec_test.rs match), so a
    # zero-match grep here is the expected case, not a failure -- see the
    # PCI_CENSUS_LINE comment below for why an unguarded pipefail would
    # otherwise abort the script at this assignment instead of at a legible
    # `test -n`/`-eq` assertion.
    # Pattern: a `fork()` call reached via `match` or an assignment (`=`),
    # through zero or more `mod::` path segments -- recognises `match
    # fork() {`, `match process::fork() {`, `match libbreenix::process::
    # fork() {`, `let x = process::fork();`, and `match
    # fork().unwrap_or_else(..) {` alike (R98; see the comment above this
    # loop for which roster/non-roster file uses which idiom).
    _ring3_smoke_forks=$(grep -cE '(match|=) *([a-zA-Z_]+::)*fork\(\)' "$_ring3_smoke_src") || true
    PRODUCTION_REAPED_ROWS=$(( PRODUCTION_REAPED_ROWS + _ring3_smoke_forks ))
done
unset _ring3_smoke_name _ring3_smoke_src _ring3_smoke_forks
readonly PRODUCTION_REAPED_ROWS
test "$PRODUCTION_REAPED_ROWS" -ge 1
# This value is not otherwise echoed anywhere in this gate's output, so a
# reader attributing a future census-assertion FAIL from the log alone
# (rather than by re-running this derivation by hand) needs it on the
# record (review finding F5).
echo "  RING3_SMOKE fork census: PRODUCTION_REAPED_ROWS=$PRODUCTION_REAPED_ROWS"
#
# (1) End of the userspace phase, emitted from the `sys_exit` arm entered when no
# userspace thread remains. This sample used to be pinnable as an exact literal
# only BECAUSE reclamation was dead: `resident=4:removed=2` was the frozen state
# of a system that had stopped retiring. With the drain live, the split between
# "still a tombstone" and "already joined" at that instant depends on how many
# retirements the drain happened to complete while the workload was still
# running, and pinning either half would pin a race. What is timing-safe is the
# conservation law: every production row the reaps claimed is either still
# resident or already removed, so
#   resident + (removed - TOMBSTONE_FIXTURE_REMOVALS) == PRODUCTION_REAPED_ROWS
# is invariant across every scheduling of the same workload. It is asserted below
# on the LAST `[TOMBSTONE_CENSUS:` line, which is this sample: the other two
# census emitters on this arch (kernel_main's and the strand oracle's once-only
# report) both fire earlier, and the total emission count is pinned so a deleted
# sample cannot pass by leaving a stale earlier line in its place.
readonly TOMBSTONE_CENSUS_EMISSIONS=3
# (2) Quiesce, emitted from the idle loop once the deferred-reclaim queues are
# empty or a 2000 ms backstop elapses, whichever comes first. Before #653 it was
# always the backstop, and `pending` was nonzero because production reclamation
# was dead by then; the field was a bounded attribution field for exactly that
# reason. It is now an exact zero, and it is the load-bearing half of this pin:
# `pending=0:parked=0:resident=0` is the return-to-zero evidence x86 has never
# had. `removed` is the two fixture rows plus every production row
# PRODUCTION_REAPED_ROWS derives (see its own comment above), because
# a row can only be removed by the join after it has been both reaped and
# retired. The reap-second/retire-second SPLIT is deliberately not pinned: with
# the drain running during the userspace phase, a row may retire before or after
# its reap and the arm that completes the join differs accordingly. Their SUM is
# invariant and is asserted below.
#
# `pending` is NOT pinned to zero, and the reason is a measurement rather than a
# concession. A boot on the fixed kernel drains seventeen of the eighteen
# receipts #653 used to leak and leaves exactly one, because a page-table root
# cannot be retired while it is the root the CPU currently has installed — and
# after the last userspace thread exits, nothing on this uniprocessor profile
# ever loads another one. That receipt is not a leak the drain could have taken;
# refusing it is the root proof working. What must be pinned is that NOTHING
# RETIRABLE is left behind, which is what `pend_selectable=0` and the
# depth-conservation check below assert, and which the strand would have
# violated with seventeen selectable receipts.
readonly TOMBSTONE_JOINED_REMOVALS=$(( TOMBSTONE_FIXTURE_REMOVALS + PRODUCTION_REAPED_ROWS ))
TOMBSTONE_QUIESCE_PATTERN="\\[TOMBSTONE_QUIESCE:resident=0:removed=${TOMBSTONE_JOINED_REMOVALS}:reap_second=[0-9]+:retire_second=[0-9]+:abandoned_unqueued=1:pending=[0-9]+:parked=0\\]"
KSTACK_QUIESCE_LEAK_PATTERN='\[KSTACK_QUIESCE_LEAK:baseline_outstanding=[0-9]+:outstanding=[0-9]+:leaked=0\]'
# (3) #653 delta (3) — the drain's own refusal counters, which existed before the
# fix and were printed by nothing: a whole-boot loss of production reclamation
# was inferable only three phases later from a tombstone census read alongside a
# queue depth. `context_violations=0` is exact (a drain entered under the process
# manager or inside a scheduler scope is a defect, not a rate). `nested=1` is
# exact and is the whole point of `injected=1`: the fix keeps the nested refusal
# as its fail-closed residual for a fatal fault inside an owned pass, so the arm
# must still be provably live, and the boot-test injection drives it once on
# purpose. A `nested=0:injected=0` line would mean the refusal path no longer
# executes at all and this pin would be vacuous. `selection_capped` is the
# production selection cap firing; its exact value is a property of how many
# receipts happened to be ready per pass rather than of the claim, so it is
# shape-pinned rather than value-pinned.
#
# The four `pend_*` fields attribute the settled queue depth term by term, using
# the same lock-free predicate the drain uses to choose a receipt.
# `pend_selectable=0` is the load-bearing one: a receipt the drain could have
# taken and did not is the #653 signature, and the strand would print seventeen
# of them here. The other three are the roots that are legitimately unretirable
# at quiesce and are left shape-pinned, because which term holds a root is a
# property of what the CPU happens to have installed.
RECLAIM_DRAIN_PATTERN='\[RECLAIM_DRAIN:nested=1:context_violations=0:selection_capped=[0-9]+:injected=1:pend_epoch=[0-9]+:pend_hw=[0-9]+:pend_shadow=[0-9]+:pend_selectable=0\]'
SCHED_STRAND_ORACLE_PATTERN='\[SCHED_STRAND_ORACLE:x86:samples=[1-9][0-9]*:checked=[1-9][0-9]*:stranded=0:running_shape=[0-9]+:ready_shape=[0-9]+:resolved_production=[0-9]+:resolved_exercised=[0-9]+:worst_dwell_ms=[0-9]+:overflow=[0-9]+:worst_nonprogress_ms=[0-9]+:nonprogress=[0-9]+:queued_on_nondispatching_cpu=[0-9]+:worst_queued_nondispatch_ms=[0-9]+:worst_cpu_scheduler_silence_ms=[0-9]+:worst_silence_cpu=[0-9]+\]'
CENSUS_WIDEN_ORACLE_LITERAL='[CENSUS_WIDEN_ORACLE:x86:arm=none:reason=uniprocessor_no_dispatching_peer:baseline_reported=0:axes=6:SKIP]'
# #796. This gate boots -smp 1, where two threads do not contend for the
# process-manager lock: the dispatch path refuses to switch while it is held, so
# the holder is not preempted and no second thread runs. The oracle says so in
# its own marker rather than printing a PASS it did not earn; the aarch64 strict
# gate pins the armed pattern, which reddens on origin/main. Pinning the SKIP
# line keeps the emitter alive on this arch -- deleting the oracle would
# otherwise leave this gate green.
FCNTL_PM_CONTENTION_ORACLE_LITERAL='[FCNTL_PM_CONTENTION_ORACLE:x86:arm=none:reason=uniprocessor_no_pm_contention_peer:online_cpus=1:SKIP]'
# #812. x86's irq_exit() runs do_softirq() only at preempt_count 0 and the
# oracle's holder is preempt-disabled, so the aarch64 race has no way to fire
# here; the oracle says so in its own marker rather than printing a PASS it did
# not earn. Pinning the SKIP line keeps the emitter alive on this arch --
# deleting the oracle would otherwise leave this gate green.
IRQ_HOLD_ORACLE_LITERAL='[IRQ_HOLD_ORACLE:x86:arm=none:reason=irq_exit_gates_softirq_on_preempt_count:online_cpus=1:SKIP]'
# #821. Unlike the two SKIPs above, this oracle has a real arm on x86: the
# defect it measures is worse here than on aarch64, because `manager()`
# performs no mask operation on this architecture, so the boot thread's
# driver-test window holds PROCESS_MANAGER with IF=1 and an input interrupt
# taken on top of it, on a machine that boots -smp 1, would wait for a lock its
# own CPU owns.
#
# `pm_blocking_acquires=0` is the property: a production counter that each of
# the 3 blocking PROCESS_MANAGER accessors bumps while the input IRQ entry's
# no-blocking scope is open. `fg_unset_before=1` is the precondition, since the
# deferring branch is only live while no foreground pgrp is set.
# `pm_held_during_entry=1` and `irqs_enabled_before=1` are the pair that says
# the injection really ran under a held, unmasked lock -- and on this
# uniprocessor profile the entry's mere completion under that hold is the
# reading, which is why the trailing arm is pinned to `local_hold`. The oracle
# reports `handler_would_block` and refuses to drive that injection when its
# own detector says the entry still reaches a blocking acquisition, so the
# unrepaired kernel reddens this gate instead of hanging the boot.
#
# `entry_us` is deliberately pinned loosely here: this profile's monotonic
# clock is TSC-backed only when the TSC is calibrated, and a millisecond-
# resolution fallback would make a tight pin a flake rather than a reading.
# The aarch64 gate carries the timed reading; this one carries the
# completed-under-hold reading.
TTY_IRQ_PM_ORACLE_PATTERN='\[TTY_IRQ_PM_ORACLE:x86:fg_unset_before=1:pm_blocking_acquires=0:deferred=2:pgrp_set_by_entry=0:processed=2:buffered=2:irqs_enabled_before=1:pm_held_during_entry=1:entry_us=[0-9]+:adopted=1:adopted_pgrp=821:restored=1:PASS:local_hold\]'
# #822's oracle, on the same entry and the console's own foreground_pgrp
# mutex. `fg_lock_touches=0` is the property, `fg_blocking_acquires=0` its
# blocking subset, and `sig_calls=1:sig_pid=822:sig_num=2` is the injected
# Ctrl+C still reaching the signal path with the right pgrp while a thread on
# this CPU holds that mutex with IF=1. `arm=local_hold` is what says the
# injection was really driven under the hold rather than refused: the x86 arm
# refuses when the entry still reaches a blocking acquisition, because this
# machine boots uniprocessor and the injection would take the boot with it.
# `entry_us` is pinned loosely here for the reason the pattern above gives.
TTY_IRQ_FG_ORACLE_PATTERN='\[TTY_IRQ_FG_ORACLE:x86:fg_known=822:target_absent=1:fg_lock_touches=0:fg_blocking_acquires=0:snapshot_reads=[1-9][0-9]*:processed=2:buffered=[1-9][0-9]*:irqs_enabled_before=1:fg_busy_probe=1:entry_us=[0-9]+:sig_calls=1:sig_pid=822:sig_num=2:snapshot_agrees=1:restored=1:PASS:local_hold\]'
# The boot-test oracle deliberately drives the detector exactly once; the forbidden exact marker is separately pinned absent below.
CREATION_LOCK_ORDER_INJECTED_LITERAL='[CREATION_LOCK_ORDER:INJECTED:PM_HELD]'
CREATION_LOCK_ORDER_VIOLATION_LITERAL='[CREATION_LOCK_ORDER:VIOLATION:PM_HELD]'
# #767 timer-scale oracle (kernel/src/time_test.rs, report_timer_scale()).
# The kernel prints exactly one of these per boot, carrying the two bracketing
# tick reads and the millisecond read taken between them. In THIS profile it is
# emitted by run_x86_timer_scale_gate(), dispatched first in the boot_tests gate
# list in kernel/src/main.rs: a measured 900 s boot of this profile emitted 0 of
# these lines in 1 of 1 such boots when the only call site was
# test_timer_resolution() in
# kernel_main_continue(), which the test userspace preempts before it is
# reached. test_timer_resolution() is the shipped (zero-feature) profile's call
# site, and is what run-x86-prod-profile-boot-test.sh pins. ms_per_tick=5 is x86's own
# 1000 / PIT_HZ with PIT_HZ = 200; a nonzero ticks_before is the anti-vacuity
# term (a tick counter still at 0 satisfies any scale factor, so it is not
# scored as a pass); in_range=1 is the conversion claim itself. Pinning the
# emission count at 1 as well as the PASS line means a FAIL emission cannot
# hide behind a later PASS, and a deleted call site cannot pass this gate by
# silence.
TIMER_SCALE_ORACLE_PREFIX='[TIMER_SCALE_ORACLE:'
TIMER_SCALE_ORACLE_PASS_PATTERN='\[TIMER_SCALE_ORACLE:x86:ms_per_tick=5:ticks_before=[1-9][0-9]*:ms=[1-9][0-9]*:ticks_after=[0-9]+:ticks_nonzero=1:in_range=1:PASS\]'
# failure-trace-capture PR-2: TIMER_TICK ring-depth self-check
# (kernel/src/tracing/providers/irq.rs). PR-2 fix round (2026-09-05,
# PR-2-2026-09-05.md section 9) replaced the original span_ms-vs-floor check
# here: at the shared 1000 ms checkpoint this gate used to fire at, x86's
# 200 Hz PIT tick rate (5x slower than aarch64's) meant only ~200 nominal
# ticks had elapsed and the ring had not come close to wrapping in EITHER
# configuration -- measured `dropped=0` for both `TICK_SAMPLE=1` (the
# mutation this gate exists to catch) and `TICK_SAMPLE=16` (the fix), with
# unsampled `span_ms=2677` against sampled `span_ms=3642`-`3831`: both
# numbers were just "elapsed wall time so far", not a ring-wrap signal, so
# a build with the sampling guard deleted passed this gate anyway -- a
# vacuous mutation leg. Retuning the checkpoint later (tried out to 13 s
# nominal) traded that for the SAME wall-clock-jitter sensitivity the
# aarch64 strict gate hit re-testing its own floor in this same round (a
# confirmed real boot on this beast host read `span_ms=17043` against a
# 20000 floor calibrated from a DIFFERENT boot's `span_ms=27742` -- a ~40%
# swing between two otherwise-identical GREEN boots).
#
# The oracle now used instead does not depend on wall-clock timing:
# `ticks_total` (irq.rs's `TIMER_TICK_TOTAL.aggregate()`, incremented
# unconditionally on each tick with no sampling applied) against
# `tick_events` (how many TIMER_TICK entries the ring currently holds) is a
# pure count ratio. Measured on this beast host (x86_64, `-smp 1`): sampled
# (`TICK_SAMPLE=16`, the fix) reads a ratio of 16.67; unsampled
# (`TICK_SAMPLE=1`, the mutation) reads a ratio of 1.00 (each tick recorded,
# and at this checkpoint the ring had not yet evicted any of them).
# RING_SPAN_RATIO_FLOOR sits inside that gap with an order of magnitude of
# margin on each side.
# span_ms is kept as an informational, non-gating liveness print only.
RING_SPAN_PATTERN='\[RING_SPAN:cpu=0:span_ms=[0-9]+:writes=[0-9]+:dropped=[0-9]+:ticks_total=[0-9]+:tick_events=[0-9]+\]'
RING_SPAN_RATIO_FLOOR=10
# Thirteen oracle/counter lines are pinned by the success chain below; fields are exact except for the bounded boot-state-dependent KSTACK_OWNER fields documented above.
# Ten launched test programs, one smoke_hello_time (the RING3_SMOKE process
# kernel_main_continue creates after the twelve disk-loaded ones), one
# futex_handoff_oracle, one df_preempt_oracle, 64 retire-cohort children, five loopback_wake_test
# processes (parent, reader, peer, load, watchdog), 16 exec-cohort children, one
# clonevm_exec_test process (renamed by its second-stage exec), its phase-1
# CLONE_VM child, two clone-admission oracle rows, one init designation
# oracle terminated-row refusal (arm A5), and the two rows P6a PR-2's tombstone
# join oracle stages — its fixture has to reach the real zombie state, so both
# rows pass through terminate_minimal, which is the tally's choke point. The
# init designation oracle's other synthetic rows are removed with remove_process
# and contribute nothing, while its two construction-failure arms create no row
# at all:
# 10 + 1 + 1 + 1 + 64 + 5 + 16 + 1 + 1 + 2 + 1 + 2 = 105. The exec-detach oracle contributes
# zero because its rows use the deferred-reclaim path rather than the
# Process::terminate / terminate_minimal tally choke point. This is a floor,
# checked >= by scripts/x86-gate-verdict.sh; the production-path arm execs the
# cohort's already-inserted parent and fails without launching a new userspace
# process; re-pin consciously.
# claim-lint:ok: the 12 addends above are this paragraph's own enumeration,
# and their sum is checked as a FLOOR (>=) by scripts/x86-gate-verdict.sh, so a
# boot that exits fewer rows than the enumeration names still fails it. The
# "contribute nothing" clause names rows removed with remove_process, which
# never reach the Process::terminate / terminate_minimal choke point this
# floor counts.
#
# Re-derived for P6a PR-2, and the re-derivation closed a pre-existing one-row
# gap rather than only adding this PR's two: the enumeration read 101 while the
# gate's own runs measured 102 (two runs, 2026-08-23) because smoke_hello_time
# was never counted. 102 + this PR's two staged rows = 104, which is what the
# branch's own run measures.
#
# 105, not 104, since the 737 direction-flag preempt oracle joined the
# RING3_SMOKE roster. It is one more launched program that runs to
# completion and exits once, so it moves this floor by exactly 1. It never
# forks, so it contributes 0 to PRODUCTION_REAPED_ROWS above, which is
# derived from fork call sites and is left untouched by this addition.
# claim-lint:ok: 0 of 0 fork() call sites in
# userspace/programs/src/df_preempt_oracle.rs under the same
# grep -cE '(match|=) *([a-zA-Z_]+::)*fork\(\)' pattern the
# PRODUCTION_REAPED_ROWS loop above runs; the +1 is the single exit of the
# single process kernel/src/main.rs creates for it. #737.
readonly EXPECTED_USERSPACE_EXITS=105

cd "$BREENIX_ROOT"
cargo build --release --features boot_tests,testing,external_test_bins --bin qemu-uefi

# Binary-level guard for #791: the kernel-thread dispatch path must not allocate.
# tests/dispatch_path_lock_free_structure.rs checks the same property at source
# level, but only as a denylist of spellings it knows; this reads the kernel that
# is about to boot. It is placed here, immediately after the build that produces
# that ELF, because this is the only gate step that has the linked kernel in
# hand. It fails the gate rather than warning: an allocation on that path is the
# defect that wedged this very gate inside x86_retire_cohort.
# claim-lint:ok: the guard is scripts/check-x86-dispatch-no-alloc.sh; its
# anti-vacuity arm fails when the symbol is absent.
./scripts/check-x86-dispatch-no-alloc.sh
BREENIX_PRINT_UEFI_IMAGE=1 cargo run --release \
    --features boot_tests,testing,external_test_bins --bin qemu-uefi >/dev/null
# create-test-disk packs userspace/programs/*.elf without rebuilding them, so
# repack every run to pick up rebuilt userspace; callers must rebuild those
# ELFs with ./userspace/programs/build.sh when userspace or libs/libbreenix-libc changed.
rm -f target/test_binaries.img
cargo run -p xtask -- create-test-disk
# The ext2 image carries the same userspace binaries, so rebuild it every run:
# a cached image silently boots old programs, and a fresh program execv-ing its
# own installed path can land in a stale copy of itself.
rm -f target/ext2.img
./scripts/create_ext2_disk.sh

UEFI_IMG=$(ls -t target/release/build/breenix-*/out/breenix-uefi.img | head -1)
test -n "$UEFI_IMG"

for i in $(seq 1 "$COUNT"); do
    OUTPUT_DIR="$BREENIX_GATE_TMP/breenix_x86_boot_tests_$i"
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    cp target/ovmf/x64/code.fd "$OUTPUT_DIR/OVMF_CODE.fd"
    cp target/ovmf/x64/vars.fd "$OUTPUT_DIR/OVMF_VARS.fd"

    qemu_host_lock_acquire qemu-system-x86_64
    # #827/#865: "start" is sampled here, right after the lock is held and
    # right before QEMU is launched -- this is when THIS boot's own
    # wall-clock window actually begins, so time spent blocked on the host
    # lock is not folded into the window the guest-uptime ratio is measured
    # against.
    HOST_MS_START="$(gbf_host_ms_now)"
    QEMU_AT_START="$(qemu_host_lock_count qemu-system-x86_64)"
    LOAD_AT_START="$(gbf_load_1m)"
    qemu-system-x86_64 \
        -pflash "$OUTPUT_DIR/OVMF_CODE.fd" \
        -pflash "$OUTPUT_DIR/OVMF_VARS.fd" \
        -drive "if=none,id=hd,format=raw,readonly=on,file=$BREENIX_ROOT/$UEFI_IMG" \
        -device virtio-blk-pci,drive=hd,bootindex=0,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=testdisk,format=raw,readonly=on,file=$BREENIX_ROOT/target/test_binaries.img" \
        -device virtio-blk-pci,drive=testdisk,disable-modern=on,disable-legacy=off \
        -drive "if=none,id=ext2disk,format=raw,readonly=on,file=$BREENIX_ROOT/target/ext2.img" \
        -device virtio-blk-pci,drive=ext2disk,disable-modern=on,disable-legacy=off \
        -machine pc,accel=tcg -cpu qemu64 -smp 1 -m 512 \
        -display none -no-reboot -no-shutdown \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -serial "file:$OUTPUT_DIR/serial_user.txt" \
        -serial "file:$OUTPUT_DIR/serial_kernel.txt" \
        >"$OUTPUT_DIR/qemu.log" 2>&1 &
    RUNNER_PID=$!
    # F2 (#835 idiom): registers this PID with the lock's own EXIT trap so a
    # SIGTERM/SIGINT delivered to just this script's own PID during the poll
    # below still kills QEMU instead of orphaning it with the lock free.
    qemu_host_lock_track_pid "$RUNNER_PID"

    passed=false
    # #865: which named branch of the poll loop below broke it, set inline
    # at each existing break site rather than re-derived by re-grepping the
    # same patterns afterward -- see the loop's own comment on why.
    POLL_BREAK_REASON=""
    # Four scheduling tests remain deferred on x86 until #567 is fixed:
    # loopback_recv_wake_when_idle, loopback_recv_wake_under_load,
    # loopback_pump_does_not_busy_spin, and tcp_final_ack_survives_accept_publish_race.
    # Review finding B1: the boot-window loopback wake-loss counter gate is a
    # bonus, not the proof for #545. It samples before any user process exists,
    # so three of its four counters are structurally zero and it cannot go red
    # for a #545 regression. The userspace recv/EOF wake marker below is the
    # #545 regression marker on x86: it proves end-to-end loopback FIN delivery
    # and blocked-reader wake, and goes red under a wake-path defect injection.
    # It is NOT a proof that kloopbackd is necessary — syscall-path drains can
    # deliver the same FIN, so the mechanism-level necessity proof is the
    # aarch64 deterministic registry suite (loopback_recv_wake_when_idle /
    # loopback_recv_wake_under_load), which is red on main.
    # The poll loop kills QEMU the moment it breaks, so it must never break on a
    # marker the kernel prints BEFORE the one the verdict needs. `TEST_TALLY:` is
    # emitted first and `TEST RUNNER: All tests passed` / `TEST RUNNER: FAILED`
    # last (kernel/src/syscall/handlers.rs), so breaking on the tally alone raced
    # the terminal marker and scored a healthy boot as
    # "nonzero=0 but the all-tests-passed marker is absent". Wait for the terminal
    # verdict marker itself; either polarity ends the wait, and the failing
    # polarity is still rejected downstream by scripts/x86-gate-verdict.sh.
    for _ in $(seq 1 900); do
        if grep -q '\[TEST:process:frame_custody_refusal_gate:PASS\]' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$FRAME_CUSTODY_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:page_table_custody_disposition_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$PT_CUSTODY_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$EXEC_FAILED_RELEASE_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:retirement_fence_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:reclaim_progress_gate:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:x86_retire_cohort:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$PT_COHORT_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:x86_exec_cohort:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$PT_EXEC_COHORT_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:exec_detach_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$EXEC_DETACH_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:clone_admission_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$CLONE_ADMISSION_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:init_designation_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$INIT_DESIGNATION_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:process:init_group_refusal_oracle:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$INIT_GROUP_REFUSAL_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$SCHED_STRAND_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$CENSUS_WIDEN_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$FCNTL_PM_CONTENTION_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$IRQ_HOLD_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$TTY_IRQ_PM_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$TTY_IRQ_FG_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$EXEC_FAILED_RELEASE_PROD_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$KSTACK_OWNER_ORACLE_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$CREATION_LOCK_ORDER_INJECTED_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF "$TOMBSTONE_JOIN_ORACLE_LITERAL" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF '[TOMBSTONE_QUIESCE:' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE "$KSTACK_QUIESCE_LEAK_PATTERN" \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qF '[RECLAIM_DRAIN:' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q '\[TEST:userspace:loopback_recv_wake:PASS\]' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -q 'TEST_TALLY:' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null \
            && grep -qE 'TEST RUNNER: (All tests passed|FAILED)' \
                "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            passed=true
            POLL_BREAK_REASON="scored_pass"
            break
        fi
        # #865: each branch below records its own name in POLL_BREAK_REASON
        # right before the break that already existed here -- an annotation
        # of which existing branch fired, not a new grep of the pattern it
        # already checks. The GATE_BOOT_FACTS ended_by classification just
        # below the loop reads this instead of re-grepping these same
        # literal FAIL/PANIC patterns a second time, which would silently
        # desync loopback_pump_structure.rs's and teardown_structure.rs's
        # own exact-occurrence-count ratchets on this file's text.
        if grep -qE '\[BOOT_TESTS:FAIL|KERNEL PANIC|panic!' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            POLL_BREAK_REASON="crash_marker"
            break
        fi
        if grep -qE '\[CENSUS_WIDEN_ORACLE:x86:[^]]*:FAIL\]' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            POLL_BREAK_REASON="failure_marker"
            break
        fi
        # The scheduler publication seam emits this prefix if it publishes while
        # the process-manager lock is held on that CPU; fail early on any variant.
        if grep -qF '[CREATION_LOCK_ORDER:VIOLATION' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            POLL_BREAK_REASON="failure_marker"
            break
        fi
        if grep -qE '\[TEST:network:[^]]*:FAIL' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            POLL_BREAK_REASON="failure_marker"
            break
        fi
        if grep -qE '\[TEST:userspace:[^]]*:FAIL' \
            "$OUTPUT_DIR"/serial_*.txt 2>/dev/null; then
            POLL_BREAK_REASON="failure_marker"
            break
        fi
        if ! kill -0 "$RUNNER_PID" 2>/dev/null; then
            break
        fi
        sleep 1
    done

    # #827/#865: sampled together, immediately before this boot's own kill --
    # ps has no output for a PID already gone, so qemu_cpu_seconds and the
    # aliveness check below must both run before the kill line, not after.
    HOST_MS_END="$(gbf_host_ms_now)"
    QEMU_AT_END="$(qemu_host_lock_count qemu-system-x86_64)"
    LOAD_AT_END="$(gbf_load_1m)"
    QEMU_ACTUAL_PID="$(gbf_resolve_qemu_pid "$RUNNER_PID" qemu-system-x86_64)"
    QEMU_CPU_S="$(gbf_qemu_cpu_seconds "$QEMU_ACTUAL_PID")"
    QEMU_STILL_ALIVE=1
    kill -0 "$RUNNER_PID" 2>/dev/null || QEMU_STILL_ALIVE=0

    kill "$RUNNER_PID" 2>/dev/null || true
    wait "$RUNNER_PID" 2>/dev/null || true
    qemu_host_lock_release

    # #865: ended_by names which bound in the poll loop above actually ended
    # this boot -- a read taken immediately after the loop already stopped
    # (the same #827 idiom the aarch64 strict gate uses), not a new stop
    # condition. POLL_BREAK_REASON is set inline inside the loop above, at
    # the same existing branches (including the success branch, alongside
    # `passed=true`) -- deriving ended_by from a fresh re-grep of those same
    # literal patterns here, or from a second read of $passed, would
    # silently add a bypass-worthy extra occurrence to text
    # loopback_pump_structure.rs and teardown_structure.rs both pin the
    # exact occurrence count of, which is what the first version of this
    # change did (caught by both ratchets going red on this branch).
    ENDED_BY="poll_exhausted"
    if [ -n "$POLL_BREAK_REASON" ]; then
        ENDED_BY="$POLL_BREAK_REASON"
    elif [ "$QEMU_STILL_ALIVE" = "0" ]; then
        ENDED_BY="qemu_exited_early"
    fi
    GUEST_UPTIME_MS="$(gbf_last_heartbeat_uptime_ms "$OUTPUT_DIR/serial_kernel.txt")"
    FACTS_LINE="$(gbf_emit_line "$i" "$HOST_MS_START" "$HOST_MS_END" \
        "$QEMU_AT_START" "$LOAD_AT_START" "$QEMU_AT_END" "$LOAD_AT_END" \
        "$QEMU_CPU_S" "$GUEST_UPTIME_MS" "$ENDED_BY")"
    printf '%s\n' "$FACTS_LINE" > "$OUTPUT_DIR/gate_boot_facts.txt"
    echo "  $FACTS_LINE"

    # Device-enumeration census leg (green arc 5, bus+NIC blended). Placed
    # BEFORE the passed-flag check below (and before the ~40 marker-count
    # assertions that follow it): none of those checks prove pci::enumerate()
    # found the device set this script itself declared, only that the boot
    # reached USERSPACE TEST COMPLETE. #702 is a silent hang inside PCI
    # enumeration right after "E1000 network device found" — a boot that dies
    # there sets $passed=false and prints none of the later markers, so a
    # census placed after the passed-flag check would never run on exactly
    # the boot it exists to name. Running it first makes that failure region
    # legible on its own: the census line's mere absence is signal, and its
    # counts are checked against what this script itself attached, not a
    # second hand-pinned literal (the #549/#551/[[gate-target-fidelity-528]]
    # census-not-literal lesson — self-count via grep on this script's own
    # command array, so a future edit to the -device flags above cannot
    # silently desync the assertion from what actually boots).
    # Anchored to actual command-line flag lines (leading whitespace then
    # `-device`), not a bare substring match: an earlier, unanchored version
    # of this pattern matched its own definition on the line immediately
    # below (this very line also contains the literal `-device
    # virtio-blk-pci,drive=` text inside the grep pattern argument),
    # self-counting 4 instead of the real 3 QEMU flags and making this
    # assertion permanently false on every boot -- the same self-referential
    # vacuity shape the aarch64 leg's own unanchored regex hit (see that
    # leg's fix, `^[[:space:]]*-device virtio-[a-z]*-device` in
    # run-aarch64-full-test.sh), just triggered by matching this script's own
    # source instead of its own prose. Caught by Confirm running this gate
    # for real and observing `4 != 3` on a healthy, correct boot.
    EXPECTED_VIRTIO_BLOCK=$(grep -cE -- '^[[:space:]]*-device virtio-blk-pci,drive=' "${BASH_SOURCE[0]}")
    test "$EXPECTED_VIRTIO_BLOCK" -ge 1
    # `|| true`: under `set -o pipefail`, a census-absent boot (e.g. the
    # #702 hang this leg exists to catch) makes `grep` exit 1 with no match,
    # which would otherwise abort the script AT THIS ASSIGNMENT — the ERR
    # trap still fires and still prints a FAIL, but it names the grep
    # command rather than the `test -n "$PCI_CENSUS_LINE"` assertion below
    # that is actually making the claim. Let the assignment succeed with an
    # empty value and let the explicit `test -n` name the real failure.
    PCI_CENSUS_LINE=$(grep -h -E 'PCI: Enumeration complete\. Found [0-9]+ devices \([0-9]+ VirtIO block, [0-9]+ network\)' \
        "$OUTPUT_DIR"/serial_*.txt | tail -1) || true
    test -n "$PCI_CENSUS_LINE"
    CENSUS_VIRTIO_BLOCK=$(printf '%s\n' "$PCI_CENSUS_LINE" | \
        sed -n 's/.*Found [0-9]* devices (\([0-9]*\) VirtIO block, [0-9]* network).*/\1/p')
    CENSUS_NETWORK=$(printf '%s\n' "$PCI_CENSUS_LINE" | \
        sed -n 's/.*Found [0-9]* devices ([0-9]* VirtIO block, \([0-9]*\) network).*/\1/p')
    test -n "$CENSUS_VIRTIO_BLOCK"
    test -n "$CENSUS_NETWORK"
    test "$CENSUS_VIRTIO_BLOCK" -eq "$EXPECTED_VIRTIO_BLOCK"
    # This invocation passes no -net/-netdev/-nic option at all, and QEMU
    # (confirmed empirically against the beast host's QEMU 8.2, not merely
    # assumed from reading this script) auto-attaches its own default NIC
    # whenever none of those flags is given -- `-nic none` is required to
    # suppress it, and nothing here passes that. A real e1000 device is
    # therefore present on every healthy boot of this gate, so the honest
    # floor is >=1.
    test "$CENSUS_NETWORK" -ge 1
    echo "  Device census: $PCI_CENSUS_LINE"

    # Per-function PCI facts, asserted against this script's own bytes
    # (green arc, docs/planning/green-program/bus/
    # BUS-X86-ENUM-GATE-2026-09-04.md).
    #
    # The kernel prints facts and nothing else:
    # kernel/src/drivers/pci.rs::dump_enumerated_functions(), called from
    # drivers::init() in every x86-64 profile, emits one
    #   PCI_FN <bus>:<dev>.<fn> <vendor>:<device> class=<cc>/<sub>
    #     bar0=<addr>/<size> irq=<line>
    # line per enumerated function plus one PCI_FN_TOTAL line. It carries no
    # expected-device set and no PASS/FAIL verdict. The expectations are
    # here, derived the way EXPECTED_VIRTIO_BLOCK above is derived -- by
    # counting this file's own QEMU flag lines -- so a future edit to the
    # -device flags cannot silently desync the assertion from what actually
    # boots (#549/#551/[[gate-target-fidelity-528]]: census, never a
    # hand-pinned literal list).
    # claim-lint:ok: mechanical description of
    # kernel/src/drivers/pci.rs::dump_enumerated_functions() and of the
    # derivation immediately below, both readable in this repo; the
    # measured profile visibility is in
    # docs/planning/green-program/bus/BUS-X86-ENUM-GATE-2026-09-04.md.
    #
    # The one table in this script that maps a QEMU device model to the
    # identity the kernel's PCI_FN line prints for it:
    #
    #   -device virtio-blk-pci,...,disable-modern=on,disable-legacy=off
    #       -> 1af4:1001 class=01/00
    #          disable-modern=on forces the legacy VirtIO transport, whose
    #          block-device ID is 0x1001 (VIRTIO_BLOCK_DEVICE_ID_LEGACY in
    #          kernel/src/drivers/pci.rs) rather than the modern 0x1042;
    #          PCI class 0x01 MassStorage, subclass 0x00.
    #   QEMU's implicit default NIC for -machine pc (no NIC flag; see below)
    #       -> 8086:100e class=02/00
    #          Intel e1000; PCI class 0x02 Network, subclass 0x00.
    PCI_FN_VIRTIO_BLK_ID='1af4:1001 class=01/00'
    PCI_FN_E1000_ID='8086:100e class=02/00'

    # Expected e1000 count, derived from this file's bytes plus QEMU's
    # implicit-default-NIC rule. QEMU auto-attaches its own default NIC for
    # -machine pc whenever no -net/-netdev/-nic option is present on the
    # command line -- `-nic none` is what suppresses it -- REGARDLESS of how
    # many explicit `-device e1000,...` flags are also present; a -device
    # flag alone never counts as a -net/-netdev/-nic option, so the implicit
    # NIC and any explicit e1000 device coexist as separate functions. On
    # the beast host's QEMU 8.2 that implicit default is an e1000 -- the
    # same rule the CENSUS_NETWORK >= 1 leg above already relies on, here
    # tightened from ">= 1 network device" to "exactly N 8086:100e". So the
    # count is additive, not either/or: EXPECTED_E1000 = the explicit
    # -device e1000 flag count, PLUS one whenever no -net/-netdev/-nic
    # option is present. This invocation passes no -net/-netdev/-nic option
    # and no explicit -device e1000 flag, so the additive formula reduces to
    # 0 + 1 = 1 here; it is still derived, not pinned -- an added `-device
    # e1000,netdev=...` flag raises the count with the flags on top of
    # whatever the implicit-NIC term contributes, and adding a
    # -net/-netdev/-nic option zeroes that term.
    # `|| true` on both: grep -c prints 0 and exits 1 when nothing
    # matches, and a zero count is the expected, healthy reading here -- an
    # unguarded $() would abort the script at the assignment under set -e.
    # claim-lint:ok: mechanical description of the two grep derivations on
    # the next two lines; the implicit-default-NIC rule is the same one
    # the CENSUS_NETWORK leg above already documents, and the measured
    # 8086:100e reading is in
    # docs/planning/green-program/bus/BUS-X86-ENUM-GATE-2026-09-04.md.
    EXPECTED_E1000_FLAGS=$(grep -cE -- '^[[:space:]]*-device e1000,' "${BASH_SOURCE[0]}") || true
    NIC_OPTION_FLAGS=$(grep -cE -- '^[[:space:]]*-(net|netdev|nic)[[:space:]]' "${BASH_SOURCE[0]}") || true
    if [ "$NIC_OPTION_FLAGS" -eq 0 ]; then
        EXPECTED_E1000=$((EXPECTED_E1000_FLAGS + 1))
    else
        EXPECTED_E1000="$EXPECTED_E1000_FLAGS"
    fi

    # `|| true` on each capture for the same reason as PCI_CENSUS_LINE
    # above: let an absent line fail the explicit `test` below with a named
    # assertion, not an aborted $() at a pipefail boundary.
    PCI_FN_LINES=$(grep -h -E 'PCI_FN [0-9a-f]{2}:[0-9a-f]{2}\.[0-7] ' \
        "$OUTPUT_DIR"/serial_*.txt) || true
    test -n "$PCI_FN_LINES"
    PCI_FN_TOTAL_LINE=$(grep -h -E 'PCI_FN_TOTAL [0-9]+' \
        "$OUTPUT_DIR"/serial_*.txt | tail -1) || true
    test -n "$PCI_FN_TOTAL_LINE"
    echo "  PCI function facts ($PCI_FN_TOTAL_LINE):"
    printf '%s\n' "$PCI_FN_LINES" | sed 's/^/    /'

    # The dump is complete in the capture: as many PCI_FN lines as the
    # kernel's own total says it printed.
    PCI_FN_TOTAL_VALUE=$(printf '%s\n' "$PCI_FN_TOTAL_LINE" | awk '{ print $2 + 0 }')
    PCI_FN_LINE_COUNT=$(printf '%s\n' "$PCI_FN_LINES" | grep -c . ) || true
    test "$PCI_FN_LINE_COUNT" -eq "$PCI_FN_TOTAL_VALUE"

    # Per vendor:device: as many enumerated functions as flags that attach
    # one. Not a floor and not a bare "> 0" -- an equality against a count
    # this script derived from its own bytes.
    MATCHED_VIRTIO_BLK=$(printf '%s\n' "$PCI_FN_LINES" \
        | grep -c -F -- "$PCI_FN_VIRTIO_BLK_ID") || true
    MATCHED_E1000=$(printf '%s\n' "$PCI_FN_LINES" \
        | grep -c -F -- "$PCI_FN_E1000_ID") || true
    test "$EXPECTED_VIRTIO_BLOCK" -ge 1
    test "$EXPECTED_E1000" -ge 1
    test "$MATCHED_VIRTIO_BLK" -eq "$EXPECTED_VIRTIO_BLOCK"
    test "$MATCHED_E1000" -eq "$EXPECTED_E1000"

    # Per matched function, exactly what the failure message says: BAR 0
    # decoded a non-zero size AND a non-zero address, and the interrupt line
    # is not 0xff, the PCI "unknown / not connected" sentinel. Both BAR
    # halves are checked because size>0 alone is satisfiable with
    # address==0, which is the state an unassigned BAR is in.
    # claim-lint:ok: mechanical description of the awk predicate on the
    # next lines; the mutation that reddens it is recorded in
    # docs/planning/green-program/bus/BUS-X86-ENUM-GATE-2026-09-04.md.
    PCI_FN_FACT_VIOLATIONS=$(printf '%s\n' "$PCI_FN_LINES" \
        | grep -F -e "$PCI_FN_VIRTIO_BLK_ID" -e "$PCI_FN_E1000_ID" \
        | awk '
            {
                addr = ""; size = ""; irq = ""
                for (i = 1; i <= NF; i++) {
                    if ($i ~ /^bar0=/) { split(substr($i, 6), a, "/"); addr = a[1]; size = a[2] }
                    else if ($i ~ /^irq=/) { irq = substr($i, 5) }
                }
                if (addr == "" || size == "" || irq == "") { bad++; next }
                if (addr == "0x0" || size == "0x0" || irq == "0xff") bad++
            }
            END { print bad + 0 }') || true
    test -n "$PCI_FN_FACT_VIOLATIONS"
    test "$PCI_FN_FACT_VIOLATIONS" -eq 0

    # Explicit assertion, not a bare boolean variable: a bare boolean value
    # executed as a command is the same silent-abort shape as the `test`
    # assertions below, and is exactly as opaque on failure without the
    # ERR trap installed above.
    test "$passed" = true
    test "$(grep -h -c '\[TEST:process:frame_custody_refusal_gate:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:page_table_custody_disposition_gate:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:retirement_fence_gate:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:reclaim_progress_gate:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:x86_retire_cohort:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:x86_exec_cohort:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:exec_detach_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:clone_admission_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:init_designation_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c '\[TEST:process:init_group_refusal_oracle:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    # Four scheduling tests remain deferred on x86 until #567 is fixed:
    # loopback_recv_wake_when_idle, loopback_recv_wake_under_load,
    # loopback_pump_does_not_busy_spin, and tcp_final_ack_survives_accept_publish_race.
    test "$(grep -h -c '\[TEST:userspace:loopback_recv_wake:PASS\]' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -c 'Refusing to map' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$FRAME_CUSTODY_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$PT_CUSTODY_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$EXEC_FAILED_RELEASE_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$PT_COHORT_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$PT_EXEC_COHORT_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$EXEC_DETACH_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$CLONE_ADMISSION_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$INIT_DESIGNATION_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$INIT_GROUP_REFUSAL_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$SCHED_STRAND_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -ge 1
    test "$(grep -h -F -c "$CENSUS_WIDEN_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$FUTEX_HANDOFF_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c 'CLONEVM_EXEC_TEST: post-exec rendezvous complete' \
        "$OUTPUT_DIR"/serial_*.txt | \
        awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$EXEC_FAILED_RELEASE_PROD_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$KSTACK_OWNER_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -F -c "$CREATION_LOCK_ORDER_INJECTED_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    # The one assertion in this file that expects ZERO matches, and therefore
    # the one that has to survive grep's no-match exit status. Under
    # `set -o pipefail` a grep that matches nothing exits 1, that status
    # becomes the pipeline's, and the ERR trap fires inside the command
    # substitution - so report_gate_failure's own output lands in the
    # substitution and `test` is handed 400 lines of serial instead of a
    # count. The gate then failed with "integer expression expected" on every
    # healthy boot, i.e. exactly when this violation was correctly absent.
    # `|| test $? -eq 1` accepts no-match and nothing else: a real grep error
    # (exit 2) still aborts.
    test "$( { grep -h -F -c "$CREATION_LOCK_ORDER_VIOLATION_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt || test $? -eq 1; } | \
        awk '{ total += $1 } END { print total + 0 }')" -eq 0
    test "$(grep -h -F -c "$TOMBSTONE_JOIN_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    # (1) The userspace-end census, pinned by conservation rather than by a race.
    # The poll loop deliberately waits on marker PRESENCE, not on these field
    # values: a field the fix makes timing-dependent must never be able to turn a
    # real regression into a 900-second poll timeout instead of a legible
    # assertion failure here.
    test "$(grep -h -F -c '[TOMBSTONE_CENSUS:' \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" \
        -eq "$TOMBSTONE_CENSUS_EMISSIONS"
    TOMBSTONE_CENSUS_USERSPACE_END_LINE=$(grep -h -F '[TOMBSTONE_CENSUS:' \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    CENSUS_RESIDENT=$(printf '%s\n' "$TOMBSTONE_CENSUS_USERSPACE_END_LINE" | \
        sed -n 's/.*\[TOMBSTONE_CENSUS:resident=\([0-9][0-9]*\):.*/\1/p')
    CENSUS_REMOVED=$(printf '%s\n' "$TOMBSTONE_CENSUS_USERSPACE_END_LINE" | \
        sed -n 's/.*\[TOMBSTONE_CENSUS:resident=[0-9][0-9]*:removed=\([0-9][0-9]*\):.*/\1/p')
    test -n "$CENSUS_RESIDENT"
    test -n "$CENSUS_REMOVED"
    test "$CENSUS_RESIDENT" -le "$PRODUCTION_REAPED_ROWS"
    test "$CENSUS_REMOVED" -ge "$TOMBSTONE_FIXTURE_REMOVALS"
    test "$(( CENSUS_RESIDENT + CENSUS_REMOVED - TOMBSTONE_FIXTURE_REMOVALS ))" \
        -eq "$PRODUCTION_REAPED_ROWS"
    # (2) Quiesce: return to zero, plus the join-arm sum the split cannot pin.
    test "$(grep -h -E -c "$TOMBSTONE_QUIESCE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    QUIESCE_LINE=$(grep -h -E "$TOMBSTONE_QUIESCE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    QUIESCE_REAP_SECOND=$(printf '%s\n' "$QUIESCE_LINE" | \
        sed -n 's/.*:reap_second=\([0-9][0-9]*\):.*/\1/p')
    QUIESCE_RETIRE_SECOND=$(printf '%s\n' "$QUIESCE_LINE" | \
        sed -n 's/.*:retire_second=\([0-9][0-9]*\):.*/\1/p')
    test "$(( QUIESCE_REAP_SECOND + QUIESCE_RETIRE_SECOND ))" -eq "$TOMBSTONE_JOINED_REMOVALS"
    test "$(grep -h -E -c "$KSTACK_QUIESCE_LEAK_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    # (3) The drain's refusal counters, with the injected arm proving the refusal
    # path still executes.
    test "$(grep -h -E -c "$RECLAIM_DRAIN_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    RECLAIM_DRAIN_LINE=$(grep -h -E "$RECLAIM_DRAIN_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    # Depth conservation: every receipt still queued at quiesce is named by a
    # root-proof term. An unattributed receipt is a receipt the drain left behind
    # for a reason nothing in this serial states, which is the condition #653 was
    # filed under and is gate-failing here.
    QUIESCE_PENDING=$(printf '%s\n' "$QUIESCE_LINE" | \
        sed -n 's/.*:pending=\([0-9][0-9]*\):.*/\1/p')
    PEND_EPOCH=$(printf '%s\n' "$RECLAIM_DRAIN_LINE" | \
        sed -n 's/.*:pend_epoch=\([0-9][0-9]*\):.*/\1/p')
    PEND_HW=$(printf '%s\n' "$RECLAIM_DRAIN_LINE" | \
        sed -n 's/.*:pend_hw=\([0-9][0-9]*\):.*/\1/p')
    PEND_SHADOW=$(printf '%s\n' "$RECLAIM_DRAIN_LINE" | \
        sed -n 's/.*:pend_shadow=\([0-9][0-9]*\):.*/\1/p')
    test "$QUIESCE_PENDING" -eq "$(( PEND_EPOCH + PEND_HW + PEND_SHADOW ))"
    # x86 has no production designated init, so the runtime refusal never fires
    # and the whole-boot walk is legitimately zero here; this is the `None`-arm
    # evidence, not the whole-boot-walk evidence, and it becomes non-zero only
    # when a phase gives x86 a real init.
    INIT_GROUP_WALK_COUNT=$(awk 'index($0, "[INIT_GROUP_WALK") { count++ } END { print count + 0 }' \
        "$OUTPUT_DIR"/serial_*.txt)
    test "$INIT_GROUP_WALK_COUNT" -eq 0
    # (4) #767: the timer-scale oracle, emitted once and passing.
    test "$(grep -h -F -c -- "$TIMER_SCALE_ORACLE_PREFIX" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    test "$(grep -h -E -c "$TIMER_SCALE_ORACLE_PASS_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | awk '{ total += $1 } END { print total + 0 }')" -eq 1
    # (5) the ring-span self-check: present, and its sampling ratio (a pure
    # count relationship, immune to wall-clock jitter -- see the comment at
    # this script's RING_SPAN_RATIO_FLOOR definition) clears the floor.
    RING_SPAN_LINE=$(grep -h -oE "$RING_SPAN_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    test -n "$RING_SPAN_LINE"
    RING_SPAN_TICKS_TOTAL=$(printf '%s\n' "$RING_SPAN_LINE" | \
        sed -n 's/.*:ticks_total=\([0-9][0-9]*\):.*/\1/p')
    RING_SPAN_TICK_EVENTS=$(printf '%s\n' "$RING_SPAN_LINE" | \
        sed -n 's/.*:tick_events=\([0-9][0-9]*\)\].*/\1/p')
    test -n "$RING_SPAN_TICKS_TOTAL"
    test -n "$RING_SPAN_TICK_EVENTS"
    test "$RING_SPAN_TICK_EVENTS" -gt 0
    test "$RING_SPAN_TICKS_TOTAL" -ge "$((RING_SPAN_TICK_EVENTS * RING_SPAN_RATIO_FLOOR))"
    EXPECTED_EXITS="$EXPECTED_USERSPACE_EXITS" \
        "$BREENIX_ROOT/scripts/x86-gate-verdict.sh" "$OUTPUT_DIR"/serial_*.txt
    COUNTER_LINE=$(grep -hE "$FRAME_CUSTODY_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    KSTACK_OWNER_LINE=$(grep -hE "$KSTACK_OWNER_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$COUNTER_LINE"
    echo "$PT_CUSTODY_LITERAL"
    echo "$PT_COHORT_LITERAL"
    echo "$PT_EXEC_COHORT_LITERAL"
    echo "$EXEC_DETACH_ORACLE_LITERAL"
    echo "$CLONE_ADMISSION_ORACLE_LITERAL"
    echo "$INIT_DESIGNATION_ORACLE_LITERAL"
    echo "$INIT_GROUP_REFUSAL_ORACLE_LITERAL"
    SCHED_STRAND_ORACLE_LINE=$(grep -h -E "$SCHED_STRAND_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$SCHED_STRAND_ORACLE_LINE"
    CENSUS_WIDEN_ORACLE_LINE=$(grep -h -F "$CENSUS_WIDEN_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$CENSUS_WIDEN_ORACLE_LINE"
    # Surfaced for the same reason its sibling above is: the wait predicate
    # already requires this literal fixed-string, so a run that reaches here
    # matched it. Until this echo existed the gate printed its sibling oracle
    # lines and not this one, so a gate log could not be read as a receipt for
    # what the uniprocessor arm said -- and a round doc cited a gate log for
    # exactly that. #796 and #812 both pass this gate by pinning a SKIP line
    # the && chain above requires, but neither line was echoed here either,
    # so a preserved copy of this driver's stdout showed the verdict and not
    # the evidence behind it.
    # claim-lint:ok: the citation and its correction are recorded in
    # docs/planning/green-program/syscalls/819-ORACLE-ARMING-2026-09-05.md
    FCNTL_PM_CONTENTION_ORACLE_LINE=$(grep -h -F "$FCNTL_PM_CONTENTION_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$FCNTL_PM_CONTENTION_ORACLE_LINE"
    IRQ_HOLD_ORACLE_LINE=$(grep -h -F "$IRQ_HOLD_ORACLE_LITERAL" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$IRQ_HOLD_ORACLE_LINE"
    TTY_IRQ_PM_ORACLE_LINE=$(grep -h -E "$TTY_IRQ_PM_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$TTY_IRQ_PM_ORACLE_LINE"
    TTY_IRQ_FG_ORACLE_LINE=$(grep -h -E "$TTY_IRQ_FG_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$TTY_IRQ_FG_ORACLE_LINE"
    FUTEX_HANDOFF_ORACLE_LINE=$(grep -h -E "$FUTEX_HANDOFF_ORACLE_PATTERN" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$FUTEX_HANDOFF_ORACLE_LINE"
    echo "$EXEC_FAILED_RELEASE_PROD_LITERAL"
    echo "$KSTACK_OWNER_LINE"
    echo "$CREATION_LOCK_ORDER_INJECTED_LITERAL"
    echo "$TOMBSTONE_JOIN_ORACLE_LITERAL"
    echo "$TOMBSTONE_CENSUS_USERSPACE_END_LINE"
    echo "$QUIESCE_LINE"
    echo "$RECLAIM_DRAIN_LINE"
    TIMER_SCALE_ORACLE_LINE=$(grep -h -F -- "$TIMER_SCALE_ORACLE_PREFIX" \
        "$OUTPUT_DIR"/serial_*.txt | tail -1)
    echo "$TIMER_SCALE_ORACLE_LINE"
    echo "$RING_SPAN_LINE"
    if grep -qE '\[BOOT_TESTS:FAIL|KERNEL PANIC|panic!' \
        "$OUTPUT_DIR"/serial_*.txt; then
        echo "x86 frame-custody gate run $i: FAIL (BOOT_TESTS:FAIL, KERNEL PANIC, or panic! marker present)"
        false
    fi
    if grep -qE '\[CENSUS_WIDEN_ORACLE:x86:[^]]*:FAIL\]' \
        "$OUTPUT_DIR"/serial_*.txt; then
        echo "x86 frame-custody gate run $i: FAIL (CENSUS_WIDEN_ORACLE:FAIL marker present)"
        false
    fi
    if grep -qE '\[TEST:network:[^]]*:FAIL' \
        "$OUTPUT_DIR"/serial_*.txt; then
        echo "x86 frame-custody gate run $i: FAIL (TEST:network:*:FAIL marker present)"
        false
    fi
    if grep -qE '\[TEST:userspace:[^]]*:FAIL' \
        "$OUTPUT_DIR"/serial_*.txt; then
        echo "x86 frame-custody gate run $i: FAIL (TEST:userspace:*:FAIL marker present)"
        false
    fi
    # Absence is meaningful because KSTACK_OWNER_ORACLE pins a nonzero
    # sched_publications driver and the injected marker proves the detector fires.
    if grep -qF '[CREATION_LOCK_ORDER:VIOLATION' \
        "$OUTPUT_DIR"/serial_*.txt; then
        echo "x86 frame-custody gate run $i: FAIL (CREATION_LOCK_ORDER:VIOLATION marker present)"
        false
    fi
    echo "x86 frame-custody gate run $i: PASS"
done
