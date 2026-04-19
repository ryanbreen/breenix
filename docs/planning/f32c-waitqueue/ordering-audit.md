# F32c Waitqueue Ordering Audit

Date: 2026-04-18

## Scope

This audit compares F32's Breenix `WaitQueueHead` against Linux v6.8 waitqueue
ordering. Phase 1 produced a deterministic lost-wake reproducer:

```text
WAIT_STRESS_STALL sample=3 entered=269 returned=268 wakes=17280 waiters=0
```

The important detail is `waiters=0`: the waker observed and drained the waiter
from the queue, but the waiter still became permanently blocked. That points at
the transition between queue enrollment and scheduler state publication.

## Linux Reference

Linux's waitqueue contract is split across these source locations:

- `/tmp/linux-v6.8/kernel/sched/wait.c:217-238`: `prepare_to_wait`
- `/tmp/linux-v6.8/kernel/sched/wait.c:270-300`: `prepare_to_wait_event`
- `/tmp/linux-v6.8/kernel/sched/wait.c:99-127`: `__wake_up_common_lock`
- `/tmp/linux-v6.8/include/linux/sched.h:184-231`: `set_current_state`
- `/tmp/linux-v6.8/kernel/sched/core.c:4247-4255`: `try_to_wake_up`
- `/tmp/linux-v6.8/kernel/sched/core.c:6653-6681`: `__schedule`
- `/tmp/linux-v6.8/kernel/sched/wait.c:356-379`: `finish_wait`

## Side-by-Side Audit

| Invariant | Linux v6.8 | Breenix F32 | Verdict |
| --- | --- | --- | --- |
| Queue enrollment and blocked-state publication are serialized by the waitqueue lock. | `prepare_to_wait` takes `wq_head->lock`, adds the wait entry, calls `set_current_state(state)`, then unlocks (`wait.c:233-238`). `prepare_to_wait_event` uses the same pattern (`wait.c:275-300`). | `WaitQueueHead::prepare_to_wait` adds the TID under `waiters` lock (`waitqueue.rs:58-62`), releases that lock, and only then calls `sched.block_current_for_io()` (`waitqueue.rs:64-66`). | **Fail.** There is an unlocked window where a waker can drain the queued TID before the thread state is `BlockedOnIO`. |
| The blocked-state store has a barrier before the waiter retests the condition or schedules. | `set_current_state` is `smp_store_mb(current->__state, state)` (`sched.h:227-231`). Linux explicitly documents that the barrier serializes the state write with the subsequent condition test (`sched.h:184-208`). | `block_current_for_io` writes `thread.state = ThreadState::BlockedOnIO` under the scheduler lock (`scheduler.rs:1617-1631`), but this write is not inside the waitqueue critical section and has no explicit waitqueue-paired full barrier. | **Fail.** The state write is neither protected by the same lock as queue enrollment nor documented with the Linux-equivalent full barrier. |
| Wake-up observes the same serialization point used by prepare. | `__wake_up` calls `__wake_up_common_lock`; that takes `wq_head->lock`, scans the wait list, and invokes each wake function while still holding the lock (`wait.c:99-127`). `prepare_to_wait_event` relies on this exact fact: wakeup locks and unlocks the same `wq_head->lock`, so the caller cannot miss the event (`wait.c:281-283`). | `wake_up` drains the waitqueue under the `waiters` lock (`waitqueue.rs:93-98`, `119-121`) but calls `isr_unblock_for_io` after the waiter has been drained. Because `prepare_to_wait` does not publish `BlockedOnIO` until after releasing the same lock, `unblock_for_io` can later see a still-running thread and decide there is nothing to wake (`scheduler.rs:1653-1668`). | **Fail.** The list operation is serialized, but the scheduler-state handoff is not. The Phase 1 `waiters=0` stall is this failure mode. |
| If wake wins before schedule, schedule becomes a no-op. | `try_to_wake_up` pairs with `set_current_state` using a full barrier before checking task state (`core.c:4247-4255`). Later, `__schedule` reads `prev->__state`; if it is `TASK_RUNNING` (`0`), it does not deactivate the task (`core.c:6653-6681`). | `schedule_current_wait` has the right shape: it checks current state and only schedules while state remains `BlockedOnIO` (`waitqueue.rs:166-183`). However, in the lost-wake window the wake is ignored before `BlockedOnIO` is set, so this check later sees `BlockedOnIO` and sleeps. | **Partial.** The schedule-side no-op exists, but it depends on a wake being able to flip the state before schedule. The prepare/wake race prevents that. |
| Finish path normalizes state and removes any leftover waiter. | Linux `finish_wait` sets the task running and removes a still-queued entry under the waitqueue lock if needed (`wait.c:356-379`). | `finish_wait` removes the TID and sets the current thread ready if it is still `BlockedOnIO` (`waitqueue.rs:75-90`). | **Pass for this race.** The reproducer never reaches `finish_wait` for the stuck waiter. |

## Lost-Wake Timeline

The deterministic reproducer can fail with this interleaving:

1. Waiter enters `prepare_to_wait`.
2. Waiter adds its TID to `WAIT_STRESS_WQ` and releases `waiters` lock
   (`waitqueue.rs:58-62`).
3. Waker calls `wake_up`, takes the same `waiters` lock, drains that TID, releases
   the lock, and queues `isr_unblock_for_io` (`waitqueue.rs:93-98`).
4. Scheduler wake processing reaches `unblock_for_io(tid)` before the waiter has
   run `block_current_for_io`; the thread is not `BlockedOnIO`, so
   `should_queue` is false (`scheduler.rs:1653-1668`).
5. Waiter resumes and sets itself `BlockedOnIO` (`scheduler.rs:1617-1631`).
6. `schedule_current_wait` observes `BlockedOnIO` and sleeps (`waitqueue.rs:166-183`).
7. The waitqueue is empty and the wake was already discarded, so the waiter never
   returns.

This is the exact condition Phase 1 reported: one more wait entered than returned,
many wake attempts, and zero queued waiters.

## Required Fix

F32c must restore the Linux ordering invariant:

1. Hold the waitqueue lock across both waiter insertion and the current-thread
   `BlockedOnIO` state publication.
2. Publish the state with a Linux-equivalent full barrier before releasing the
   waitqueue lock. Linux uses `smp_store_mb` in `set_current_state`
   (`sched.h:227-231`).
3. Keep wake-up serialized on the same waitqueue lock. A wake must not be able to
   remove a waiter until the waiter's scheduler state is visible as blocked.
4. Preserve `schedule_current_wait`'s no-op behavior when a wake wins before the
   explicit schedule call.

The minimal Breenix change is therefore in `WaitQueueHead::prepare_to_wait`:
perform the duplicate-free enqueue and `sched.block_current_for_io()` inside the
same `with_waiters` critical section, followed by a full fence before unlocking.
With that ordering, any `wake_up` that drains the TID must happen after the waiter
is already `BlockedOnIO`, so `unblock_for_io` can mark it ready. If the wake wins
before `schedule_current_wait`, the schedule-side state check becomes a no-op, as
in Linux.

No compositor fallback, timer polling, or arbitrary timeout is involved in this
fix. It is a lock-ordering and memory-ordering repair to match Linux's waitqueue
contract.
