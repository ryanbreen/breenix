# A.3 retained recovery audit

Status: blocked at runtime head `857367b3`. This continuation reads Vigil
`20261112T011722Z-qemu-12409` and the producing code; it does not change runtime
code, assertions, deadlines or selection. The capture reports 79/81, with
`recovery/stack-restored` and `recovery/supervisor-restarts` failing. MEMORY
reclaims 10 pages within the 48-page allowance. Acceptance remains blocked.

## Click failure precedes restart

`wm_suite/cases.rs::stack_restored` calls `twin`, then `d.focus(a)`, then
minimize and kill. Its failure message comes from that focus call. PID 728
is the initial compositor in this case, not a replacement. The failing case
does not reach its restart assertion.

The serial supplies these guest monotonic stamps:

| Observation | Nanoseconds | Serial line |
| --- | ---: | ---: |
| Press request | 741997806166 | 1068758 |
| Acknowledgment wait begins | 742227514833 | 1069056 |
| Local deadline | 743227514833 | 1069056 |
| Wait ends without acknowledgment | 743266162166 | 1069056 |
| Kernel queues press, window 80, serial 951 | 743396053041 | 1069058 |
| Client handles that press | 743396105041 | 1069060 |

The wait lasted 1038.647333 ms against 1000 ms: margin -38.647333 ms.
Delivery occurred 1168.538208 ms after wait start, margin -168.538208 ms,
and 1398.246875 ms after the request. The client handled it 52 microseconds
after kernel delivery. This is delayed delivery, rather than evidence of
the event being permanently lost.

`window/ops.rs::write_events` stamps delivery when the compositor's event
enters the window ring. `bwm/input.rs` queues routed events, and bwm flushes
them on its main loop. `gpu/mod.rs::ring_doorbell` brackets a volatile MMIO
write after releasing the GPU core lock. The retained journal has these
long notification brackets for PID 728:

| Sequence | Begin ns | Return ns | Elapsed ms |
| --- | ---: | ---: | ---: |
| 111626 | 742325414666 | 742611970583 | 286.555917 |
| 111636 | 742612084791 | 742667902833 | 55.818042 |
| 111646 | 742668734125 | 742721163833 | 52.429708 |
| 111695 | 742798167625 | 743215733375 | 417.565750 |
| 111709 | 743216929375 | 743388507000 | 171.577625 |

Deduplicating by sequence, clipping to the original 1000 ms deadline and
unioning the 28 intersecting PID 728 brackets gives 996.911918 ms. These are
elapsed brackets around notification calls, including possible interrupts
or descheduling; they are not CPU accounting or host mutex ownership.

The retained host join maps two trace gaps to exact wire fences: fence
59073 follows PID 737/context 336/type 519/payload 19660 for 284.144 ms;
fence 59111 follows the same origin with payload 33048 for 417.356 ms.
The second sample's main loop has 115/136 stacks in `select`, seven in
link and six in compile. Its four vCPU BQL counts are 6, 8, 18 and 16 of
136. Aggregate sample counts do not place those stacks at the individual
MMIO brackets. Thus the notification delay is measured, while its precise
host cause remains unresolved.

## Supervisor watchdog follows a different wait

`cases.rs::supervisor_restarts` first launches the supervised desktop and
waits for its initial client, creates the test client, waits for its frame,
kills bwm, waits for a different child PID, then requests current content.
These waits must not be conflated:

| Wait | Duration ms | Local deadline ms | Margin ms |
| --- | ---: | ---: | ---: |
| Initial supervised client | 1872.926666 | 2000 | 127.073334 |
| Replacement child appears | 1089.306208 | 2000 | 910.693792 |
| Post-restart client 83/commit 4, unsuccessful | 1749.187333 | 2000 | 250.812667 unused |

The initial client's 6.35% margin does not meet the coordinator's 25%
requirement. The child replacement has 45.53% margin; it is not the wait
that fails here. Serial line 1094576 records its interval as
768764908000–769854214208 ns.

The unsuccessful content wait spans 769875855083–771625042416 ns. In
`desktop.rs::until_from`, the local deadline and `case_ms_left() < 200`
are alternative refusal conditions. This wait exits before its local
deadline, when the outer case budget is almost exhausted. At the watchdog
snapshot, the case records 8418.297832 ms in waits and 1242.704181 ms in
578 diagnostic writes. Diagnostic and wait intervals are not necessarily
disjoint, so these totals must not be added into a purported complete
budget decomposition. The parent reports its 10000 ms watchdog; the child
has no logic-end/report-end stamp. Its final detailed failure export is
incomplete. The code explains why the outer watchdog can mask the inner
failure; it does not establish the source of the startup/readback delay.

## Correct the earlier interval labels

Kernel `window/scene.rs` calls `accounting::composite` on a successful
composite, without requiring a client entry. `Accounting::end` closes the
active window there. Window 64 ends at 740305504833 ns, before the desktop
observer finishes at 740415207500 ns and before the userspace client
first-composite stamp at 741159696958 ns. Its 2390.422375 ms duration is
therefore not a replacement's first-client receipt. Window 66's
3538.819208 ms duration similarly denotes its first kernel composite;
that record alone does not establish client 83/commit 4 receipt. Its case
archive freezes before that end. The prior ledger and PR descriptions
overstated what these accounting intervals measured.

## Stop and missing observation

The coordinator's mechanism-first stop and normal-design restriction
remain in force. A cause-specific forcing test would currently encode an
unresolved hypothesis. No implementation repair or acceptance boot follows
this audit.

The ledger blocker names one measurement question: during the exact
notification brackets delaying input, is the submitting vCPU waiting for
BQL held by named GL work, or descheduled; and which startup, content,
readback and diagnostic operation consumes the supervisor's outer budget?
Time-resolved host stacks plus bounded per-operation guest boundaries
through the failed wait and watchdog would distinguish these paths. This
question is not authorization for that boot. Issues #2872 and #2890 remain
open, and PR #2874 stays draft.

## Validation

Read-only extraction deduplicated GPU rows by sequence and unioned clipped
intervals, rather than summing the recent and archived copies. It read
the existing host join and raw sample call graph. Fresh `origin/main`
`ac3cdf6c` remains an ancestor of `857367b3`; no rebase is needed.
Documentation is the only repository change in this continuation, so kernel
builds, host suites and ARM/R240 boots are not repeated.

Validation invocations:

- `python3 scripts/claim-lint.py --files docs/planning/arm64-catchup/a3-retained-recovery-audit.md`
  -> exit 0 on the initial report.
- `python3 scripts/claim-lint.py --base 857367b3` -> exit 0.
- `python3 scripts/claim-lint.py --commit-msg /tmp/a3-retained-audit-commit.txt`
  -> exit 0.
- `git diff --check` -> exit 0.
