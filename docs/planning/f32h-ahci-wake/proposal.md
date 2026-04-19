# F32h Proposed ISR Wake Fix Design

## Constraints

This is design only. Do not implement in F32h.

The design must preserve F32e/F32f:

- Keep the Linux-style prepare-to-wait ordering.
- Keep immediate task-context waitqueue wake.
- Do not add serial output or logging to AHCI, syscall, interrupt, or scheduler
  hot paths.
- Do not call the global scheduler mutex from hard IRQ context.

F32h evidence changes the starting assumption: the two trace-backed failures did
not show a stuck pending ISR wake entry. The representative chains reached
`BlockedOnIO -> Ready` and enqueued tid 10. Therefore the proposed fix should be
framed as Linux-parity risk reduction for the ISR wake design, not as a proven
fix for an observed non-draining ring in these captures.

## Option A: IRQ-Safe Immediate Wake For I/O Completions

Replace `isr_unblock_for_io(tid)`'s deferred-buffer publish with an IRQ-safe
immediate wake operation that performs the state transition and runqueue enqueue
inline from interrupt context.

Linux cite:

- `/tmp/linux-v6.8/kernel/sched/completion.c:16-25`: `complete_with_flags()`
  uses `raw_spin_lock_irqsave()` and calls `swake_up_locked()`.
- `/tmp/linux-v6.8/kernel/sched/swait.c:21-30`: `swake_up_locked()` calls
  `try_to_wake_up(curr->task, TASK_NORMAL, wake_flags)` directly.
- `/tmp/linux-v6.8/kernel/sched/core.c:4186-4222`: `try_to_wake_up()` is the
  atomic state-to-runnable operation.

Breenix shape:

- Add a narrow IRQ-safe scheduler wake primitive, for example
  `try_wake_io_from_irq(tid)`.
- It must not take the global `SCHEDULER` mutex.
- It should use only a scheduler-safe lock subset. The likely minimum is:
  per-thread wake/state lock or atomic state transition; per-runqueue lock for
  the selected target queue; per-CPU reschedule flag/IPI emission.
- It should share the same state logic as `wake_io_thread_locked()`:
  wake only BlockedOnIO or Ready+blocked_in_syscall; clear `wake_time_ns`; avoid
  enqueue when the thread is still current or in deferred requeue; set
  `need_resched`; send target reschedule IPI if enqueued on a remote or idle CPU.
- The existing global-lock `wake_io_thread_locked()` remains the task-context
  implementation until scheduler locking is split enough to share the lower
  primitive.

Safety analysis:

- Safe only if the required state/runqueue data can be protected without the
  global mutex. Linux uses `p->pi_lock` and rq locks; Breenix currently stores
  thread state and per-CPU queues inside the global scheduler object. F32i would
  need to split enough state out or introduce raw spinlocks around those
  substructures.
- The old `with_scheduler()` from IRQ is not acceptable. Commit `4caa2639`
  showed that global scheduler lock contention with IRQs masked starves CPU0's
  timer.
- The primitive must handle the same deferred-requeue race F32e/F32f handle:
  if the target is still current or in previous/deferred state, do not enqueue
  it prematurely.

Expected latency:

- Best latency. The waiter becomes scheduler-visible during the AHCI IRQ itself,
  matching Linux's completion wake semantics.
- Removes the drain dependency and makes wake latency independent of the next
  scheduler entry.

Risk:

- Highest implementation risk because it requires scheduler lock refactoring.
- If implemented by taking the current global mutex, it regresses to the
  pre-`4caa2639` CPU0 IRQ starvation failure.

## Option B: Keep Deferred Buffer, Make Drain Bounded At IRQ Exit

Keep `isr_unblock_for_io()` as a lock-free ISR publish, but guarantee that the
publish is drained before returning to the interrupted context whenever the
scheduler lock can be acquired without blocking.

Linux cite:

- Linux does not use this design for completions; it calls
  `try_to_wake_up()` directly (`completion.c:16-25`, `swait.c:21-30`).
- Linux does, however, avoid unbounded spinning in IRQ-disabled regions by using
  raw spinlocks designed for this path. Option B is a transitional design for
  Breenix because it does not yet have those locks.

Breenix shape:

- `isr_unblock_for_io(tid)` continues to push into the per-CPU slots.
- At IRQ tail, attempt a non-blocking drain:
  acquire scheduler state only with `try_lock`; if unavailable, leave the entry
  pending and rely on the existing scheduler-entry drain.
- If drain succeeds, run the existing `wake_io_thread_locked()` under the
  scheduler lock and send target reschedule IPI.
- Add permanent low-overhead counters, not serial logs:
  pushed count, dropped/full count, drained count, max pending depth,
  try-lock-miss count, max push-to-drain latency.

Safety analysis:

- Does not spin in hard IRQ context. A failed `try_lock` exits quickly.
- Reuses the existing scheduler-locked wake logic, so it is lower risk than
  splitting scheduler locks immediately.
- Still not Linux parity: if the lock is busy, wake remains deferred. It reduces
  average latency but not worst-case latency.

Expected latency:

- Lower than the current design when the scheduler lock is uncontended at IRQ
  tail.
- Still unbounded under sustained scheduler lock contention or if IRQ-tail
  scheduling progress is the thing that has failed.

Risk:

- Medium. It touches interrupt-tail code and scheduler lock acquisition policy,
  so it must avoid Tier 1 files unless explicitly approved in F32i.
- It could hide the real scheduler/timer progress bug by making the common case
  faster while leaving rare deferred failures.

## Option C: Split Scheduler Wake State Into Linux-Like Locks, Then Remove Ring

This is the clean Linux-parity endpoint. Introduce enough scheduler structure to
make `try_to_wake_up` possible, then delete the deferred ISR buffer entirely.

Linux cite:

- `/tmp/linux-v6.8/kernel/sched/core.c:4202-4214`: `try_to_wake_up()` relies on
  `p->pi_lock` and runqueue locks rather than one global scheduler lock.
- `/tmp/linux-v6.8/kernel/sched/core.c:4253-4369`: the wake path serializes
  state, on-rq/on-cpu, CPU selection, migration, and enqueue under that lock
  protocol.
- `/tmp/linux-v6.8/kernel/sched/swait.c:21-30`: completions reach that same
  primitive directly.

Breenix shape:

- Introduce per-thread wake/state serialization separate from the global
  scheduler mutex.
- Introduce per-CPU runqueue locks or lock-free queue protocol sufficient for
  IRQ-context enqueue.
- Represent "current", "previous/deferred requeue", and "queued" with a protocol
  equivalent to Linux `on_cpu`/`on_rq` ordering.
- Route both completion ISR wake and task-context waitqueue wake through the
  same lower-level wake primitive.
- Remove `ISR_WAKEUP_BUFFERS` after parity tests pass.

Safety analysis:

- This avoids the global-lock-in-IRQ problem and removes the deferred
  side-channel.
- The hard part is preserving Breenix's AArch64 context-save/deferred-requeue
  invariant. The wake primitive must never allow another CPU to run a thread
  whose old CPU has not saved its context yet.
- This option should be developed test-first with the F32e/F32f waitqueue tests,
  an AHCI completion stress test, and Parallels boot captures.

Expected latency:

- Same class as Linux: wake becomes visible during completion, with IPI latency
  as the dominant cross-CPU cost.
- Removes buffer drain latency and removes scheduler-entry dependency.

Risk:

- Highest scope but best architectural result.
- This is the right target if F32i is allowed to refactor scheduler locking
  rather than patch the current mechanism.

## Recommended F32i Direction

Do not implement Option A by simply restoring the old
`with_scheduler(|s| s.unblock_for_io(tid))` ISR call. That directly conflicts
with the `4caa2639` evidence.

The recommended path is:

1. Add a permanent, low-overhead diagnostic counter set for ISR wake buffer
   depth, push/drop/drain counts, and max push-to-drain latency. This preserves
   the F32h evidence channel without serial breadcrumbs.
2. In parallel, design Option C's minimal lock split for a Linux-like
   `try_wake_io_from_irq()`.
3. If F32i needs a narrow interim experiment, try Option B with strict
   non-blocking `try_lock` only and compare traces. Treat it as transitional,
   not final Linux parity.
4. Implement Option C when the lock protocol is clear. The success criterion is
   that AHCI completion wake uses one immediate scheduler wake primitive in IRQ
   context, analogous to Linux `complete() -> swake_up_locked() ->
   try_to_wake_up()`.

F32h's captured failures point beyond the ISR ring drain itself: both runs show
the ring drained and tid 10 made runnable. F32i should therefore validate not
only wake latency but also CPU0/global tick progress, ready queue membership,
and the IRQ-return scheduler path after the wake.
