# Turn 8 fix proposal

## Exact race

CPU A enters the AArch64 deferred-requeue or inline-schedule path for thread T. T may be marked `Ready` before CPU A has fully released ownership of T's saved frame and kernel stack. The safe handoff marker is `cpu_state[A].previous_thread` plus the later `DEFERRED_REQUEUE[A]` drain.

If a stale duplicate of T is already present in any per-CPU ready queue, CPU B can pop it. Before Turn 8, the dequeue code accepted any popped thread unless its state was `Terminated`. That means CPU B could dispatch T even when T was still current/deferred on CPU A, or after another duplicate had already changed T to `Running`.

Turn 7's failing boot fits this pattern: CPU 7 switched from tid 10 to tid 14 while the deferred snapshot stream showed tid 14 still owned by another CPU's scheduler/deferred path.

## Minimal fix

Strengthen ready-queue dequeue to validate dispatchability under the scheduler lock:

- reject missing or terminated entries
- reject entries whose state is not `Ready`
- reject entries current on a remote CPU
- reject entries present in any AArch64 `previous_thread`

The current CPU's own current thread remains eligible as a same-thread candidate so the existing no-switch and deferred-requeue branches keep their semantics. Remote current ownership is the unsafe case.

The same helper is used by both `schedule()` and `schedule_deferred_requeue()` so stale entries are handled consistently.

## Invariant counters

Add three `TraceCounter`s:

- `SCHED_STALE_QUEUE_NOT_READY`
- `SCHED_STALE_QUEUE_CURRENT`
- `SCHED_STALE_QUEUE_DEFERRED`

Any nonzero value proves that a queue entry existed that was not dispatchable by the strengthened invariant. These are lock-free atomic counters and do not add logging to hot interrupt/syscall/context-switch paths.

## Constraint check

The fix touches only non-prohibited files:

- `kernel/src/fs/procfs/xhci.rs`
- `kernel/src/task/scheduler.rs`
- `kernel/src/tracing/providers/counters.rs`

`/proc/xhci/counters` is extended only to expose the three scheduler invariant counters next to the Turn 5 xHCI counters for stress-run capture.

The Turn 5 xHCI patch remains applied in:

- `kernel/src/drivers/usb/xhci.rs`
- `kernel/src/main_aarch64.rs`

No gold-master region and no Tier 1 syscall/timer interrupt file is modified.
