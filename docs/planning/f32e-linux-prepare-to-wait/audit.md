# F32e Linux `prepare_to_wait_event` Audit

Date: 2026-04-18

## Scope

This audit compares Linux v6.8's waitqueue sleep pattern against Breenix's F32c
`WaitQueueHead` implementation. F32c closed the lost-wake reproducer by holding
the waitqueue lock across `Scheduler::block_current_for_io()`, but that critical
section is larger than Linux's `prepare_to_wait_event` pattern.

## Linux Reference

| Mechanism | Linux v6.8 file:line evidence |
| --- | --- |
| Waiter insertion, state publish, unlock | `/tmp/linux-v6.8/kernel/sched/wait.c:270-300`: `prepare_to_wait_event` takes `wq_head->lock`, adds the entry if needed, calls `set_current_state(state)`, then unlocks. The simpler `prepare_to_wait` has the same lock scope at `/tmp/linux-v6.8/kernel/sched/wait.c:233-238`. |
| Why `set_current_state` is after list insertion | `/tmp/linux-v6.8/kernel/sched/wait.c:216-227`: Linux documents that the state store needs a memory barrier after the waitqueue add so wakeups either see the queued waiter or the waiter sees the wake. |
| State-store barrier | `/tmp/linux-v6.8/include/linux/sched.h:184-231`: `set_current_state(state)` uses `smp_store_mb(current->__state, state)`; `__set_current_state` is only a `WRITE_ONCE`. |
| Wake takes same waitqueue lock | `/tmp/linux-v6.8/kernel/sched/wait.c:99-108`: `__wake_up_common_lock` takes `wq_head->lock`, calls `__wake_up_common`, then unlocks. `/tmp/linux-v6.8/kernel/sched/wait.c:120-127` notes a wake executes a full memory barrier before task-state access. |
| Wake/state barrier pairing | `/tmp/linux-v6.8/kernel/sched/core.c:4247-4255`: `try_to_wake_up` executes `smp_mb__after_spinlock()` before checking `p->__state`, pairing with `set_current_state()`'s `smp_store_mb`. |
| Schedule state check | `/tmp/linux-v6.8/kernel/sched/core.c:6653-6681`: `__schedule` reads `prev->__state` once; if `prev_state` is zero (`TASK_RUNNING`), it does not deactivate the task as sleeping. |

## Side-by-Side

| Question | Linux v6.8 | Breenix F32c | Finding |
| --- | --- | --- | --- |
| 1. Does Breenix's `set_state(BlockedOnIO)` carry the equivalent of `smp_store_mb`? | `set_current_state` is a state store plus full barrier (`sched.h:227-231`), and Linux explains the barrier serializes the store with the following condition check (`sched.h:184-208`). | `WaitQueueHead::prepare_to_wait` calls `sched.block_current_for_io()` while holding the waitqueue lock, then executes a `SeqCst` fence before leaving that waitqueue critical section (`kernel/src/task/waitqueue.rs:58-72`). `block_current_for_io_with_timeout` writes `thread.state = ThreadState::BlockedOnIO` and `blocked_in_syscall = true` (`kernel/src/task/scheduler.rs:1617-1631`). | **Partial pass.** The F32c ordering provides a full fence after the blocked-state write while the waitqueue lock is still held, which is Linux-like for the race. It is not a named state-publish primitive, and the state field itself is not an atomic store-release. |
| 2. Does Breenix's wake path take the same waitqueue lock that the waiter took? | Wake takes `wq_head->lock` around the list walk and wake callbacks (`wait.c:99-108`). `prepare_to_wait_event` relies on wake locking/unlocking the same lock so the caller cannot miss the event (`wait.c:281-283`). | `wake_up` drains waiters under the same `waiters` lock via `drain_waiters()` (`kernel/src/task/waitqueue.rs:99-103`, `kernel/src/task/waitqueue.rs:125-137`), then calls `scheduler::isr_unblock_for_io()` after the lock has been released (`kernel/src/task/waitqueue.rs:100-103`). | **Partial pass.** The list removal is serialized by the same lock, so F32c closes the original lost-wake window. It is not exact Linux because the wake action is queued after releasing the waitqueue lock. Since `isr_unblock_for_io` is lock-free (`kernel/src/task/scheduler.rs:2542-2561`), Breenix can move that wake publication into the same waitqueue critical section without taking the scheduler lock. |
| 3. Does Breenix's equivalent of `schedule()` re-check state before actually blocking? | `__schedule` checks `prev->__state`; `TASK_RUNNING` is not deactivated (`core.c:6653-6681`). This is what makes a wake between `prepare_to_wait_event` and `schedule()` turn `schedule()` into a no-op sleep-wise. | `schedule_current_wait` checks the current thread state under the scheduler lock and only calls `schedule_from_kernel()` while it remains `BlockedOnIO` (`kernel/src/task/waitqueue.rs:166-189`). However, the lower-level AArch64 `scheduler::schedule()` wrapper always enters `schedule_from_kernel()` (`kernel/src/task/scheduler.rs:2205-2209`), and `prepare_to_wait` currently performs the heavyweight `block_current_for_io()` before the caller can reach that state check (`kernel/src/task/waitqueue.rs:63-72`). | **Pass at wrapper level, missing as a primitive split.** Breenix has the no-op state re-check in `schedule_current_wait`, but the Linux pattern is obscured because `prepare_to_wait` uses the full block operation under the waitqueue lock. F32e should split this into: publish `BlockedOnIO` under the waitqueue lock, unlock, then call a state-checking schedule helper that returns immediately if a wake already made the thread `Ready`. |

## Required F32e Change

F32e should keep the race-closure invariant from F32c but shrink the waitqueue
critical section to the exact Linux shape:

1. Under the waitqueue lock, add the current TID if absent and publish
   `ThreadState::BlockedOnIO`.
2. Use an explicit full barrier before unlocking, mirroring Linux's
   `smp_store_mb` in `set_current_state` (`/tmp/linux-v6.8/include/linux/sched.h:227-231`).
3. Move the lock-free `isr_unblock_for_io` wake publication under the same
   waitqueue lock, mirroring Linux's same-lock wake path
   (`/tmp/linux-v6.8/kernel/sched/wait.c:99-108`), without taking the scheduler
   lock from the waitqueue critical section.
4. Keep scheduling outside the waitqueue lock and rely on `schedule_current_wait`
   to re-check state before calling into the architecture scheduler, matching
   Linux's `__schedule` state check (`/tmp/linux-v6.8/kernel/sched/core.c:6653-6681`).

The key implementation difference from F32c is replacing
`block_current_for_io()` in `prepare_to_wait` with a lightweight publish-only
primitive. The publish primitive may charge outgoing CPU ticks and mark
`blocked_in_syscall`, but it must not scan all per-CPU ready queues or otherwise
extend the waitqueue critical section beyond Linux's add-entry + state-publish
scope.

## Non-Goals Confirmed

- No timer fallback or arbitrary timeout is part of the F32e waitqueue fix.
- No CPU0 EL0 routing workaround from F32d is part of this branch.
- No Tier 1 prohibited file needs to change for this design.
