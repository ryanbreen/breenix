# Turn 7: Marker-Lifecycle Fix Attempt

Status: INCONCLUSIVE

## A. Race Analysis

Turn 6 showed that the remaining `READY_SITE_SCHEDULE` inline rescues were concentrated in two scheduler-owned buckets:

```text
[rescue-attrib] dropped=21 isr_lost=0 wake_no_enq=0 other=0 inline=21 timer=0 total=21
[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared=15 ownership_skip=6 stale_queue=0 other=0
```

The Turn 7 hypothesis was that `previous_thread` was being cleared before the outgoing Ready thread had a durable owner. The candidate fix kept the marker visible longer in `requeue_thread_after_save()`, only cleared it after state/queue checks, and tried to make Ready-but-not-queued stale-current cases reachable by enqueueing them instead of returning.

That was too aggressive. The run reached a state with zero rescues, but then hit a context-corruption fault:

```text
[PC_ALIGN] ELR=0x100000001 FAR=0x100000001 from_el0=0 cpu=0 sp=0xffff000054254eb0
  x29=0x0 x30=0x100000001 x0=0x8 x1=0xffff0000402e1000
  current_tid=13 owner_pid=3 bis=0 saved_elr=0x4000934c saved_x30=0x4000933c
  last_dispatch_elr=0x4000934c last_dispatch_spsr=0x40000000
  stack[0]=0xa stack[1]=0x9 stack[2]=0x8 stack[3]=0x7
```

Interpretation: the `ownership_skip` bucket is not safe to fix by force-enqueueing a thread that still appears current. That can make a thread runnable while its CPU/context ownership is still ambiguous, leading to a bad return context or double-dispatch shape. The marker-lifecycle invariant is still the likely target, but the fix needs stronger ownership evidence before changing behavior.

## B. Fix Shape

The attempted fix was a scheduler-only marker lifecycle change. It did not touch rescue paths, timer code, context switch assembly, exception return, GIC, CPU0 guard, or idle-loop WFI.

The shape was:

- Delay clearing the same-CPU `previous_thread` marker in `requeue_thread_after_save()` until after state and queue checks.
- Treat Ready, not-queued, same-CPU stale-current cases as enqueueable instead of returning.
- Preserve the existing other-CPU deferred marker guard.
- In exception cleanup, enqueue a Ready, not-queued previous thread before clearing the marker when it was not still current.

This shape drained Turn 6's `marker_cleared` and `ownership_skip` evidence to zero during the test window, but it regressed correctness. The candidate was reverted immediately.

## C. Diff

Attempted code commit:

```text
ca04ebbb fix(scheduler): tighten previous_thread marker lifecycle
 kernel/src/task/scheduler.rs | 85 ++++++++++++++++++++++++++++++++------------
```

Revert commit:

```text
0e60f530 revert: tighten previous_thread marker lifecycle
 kernel/src/task/scheduler.rs | 85 ++++++++++++--------------------------------
```

Final code state excludes the bad Turn 7 fix. The branch is back to the Turn 5 deferred-requeue fix plus Turn 6 memory-only attribution counters.

## D. Boot Evidence

Artifacts:

- Build before attempted run: `turn7-artifacts/marker-lifecycle-fix/kernel-build.txt`
- Build after revert: `turn7-artifacts/marker-lifecycle-fix/kernel-build-after-revert.txt`
- Run log: `turn7-artifacts/marker-lifecycle-fix/run.out`
- Serial log: `turn7-artifacts/marker-lifecycle-fix/serial-regression.log`
- VM name: `turn7-artifacts/marker-lifecycle-fix/vm-name.txt`

The candidate build was clean:

```text
Finished `release` profile [optimized] target(s) in 0.05s
```

The after-revert build was also clean:

```text
Compiling kernel v0.1.0 (/Users/wrb/fun/code/breenix.worktrees/scheduler-wake-atomic/kernel)
Finished `release` profile [optimized] target(s) in 4.41s
```

Parallels VM: `breenix-1779218816`

Healthy sample immediately before the fault:

```text
[freeze-watch] uptime_ms=215421 submits=117746 completes=117749 fails=0 last_completion_ms=215421 fps_last_5s=181
[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared=0 ownership_skip=0 stale_queue=0 other=0
```

Regression:

```text
[PC_ALIGN] ELR=0x100000001 FAR=0x100000001 from_el0=0 cpu=0 sp=0xffff000054254eb0
  x29=0x0 x30=0x100000001 x0=0x8 x1=0xffff0000402e1000
  current_tid=13 owner_pid=3 bis=0 saved_elr=0x4000934c saved_x30=0x4000933c
  last_dispatch_elr=0x4000934c last_dispatch_spsr=0x40000000
  stack[0]=0xa stack[1]=0x9 stack[2]=0x8 stack[3]=0x7
```

Freeze after the regression:

```text
[freeze-watch] uptime_ms=220426 submits=118841 completes=118844 fails=0 last_completion_ms=217311 fps_last_5s=72
[freeze-watch] uptime_ms=225429 submits=118841 completes=118844 fails=0 last_completion_ms=217311 fps_last_5s=0
[freeze-watch] uptime_ms=230432 submits=118841 completes=118844 fails=0 last_completion_ms=217311 fps_last_5s=0
```

The final rescue sample still showed zero rescue attribution:

```text
[rescue-attrib] dropped=0 isr_lost=0 wake_no_enq=0 other=0 inline=0 timer=0 total=0
[rescue-detail] no_switch=0 inline_sched=0 exc_return=0 marker_cleared=0 ownership_skip=0 stale_queue=0 other=0
```

That makes the result clearly invalid: zero rescues were achieved by introducing a new context-corruption failure.

## E. Honesty Check

This turn is not complete. The attempted fix met the rescue-counter target only before a `PC_ALIGN` fault and subsequent BWM freeze. That is a correctness regression, so the fix was reverted and must not be carried forward as a solution.

The useful result is negative evidence: `ownership_skip` cannot be fixed by blindly enqueueing a thread that still appears current, and `marker_cleared` needs a narrower lifetime fix that does not create ambiguous CPU/context ownership.

## F. Status

INCONCLUSIVE.

Recommended Turn 8 scope:

1. Split `ownership_skip` into at least current-owner versus other-deferred-owner before changing behavior.
2. Split `marker_cleared` by source: exception cleanup, caller-side pre-clear, and same-CPU marker missing at `requeue_thread_after_save()` entry.
3. Avoid enqueue-on-current behavior. Preserve marker ownership or add generation/owner evidence first, then only enqueue when the thread is proven not current and not deferred elsewhere.
4. Keep the rejected raw `DEFERRED_REQUEUE` slot predicate out of the fix path.
