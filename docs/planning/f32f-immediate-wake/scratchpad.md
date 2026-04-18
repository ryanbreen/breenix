# Scratchpad - F32f Immediate Wake Under Waitqueue Lock

## 2026-04-18T22:02:39Z
Starting M1. I will collect line-numbered Linux evidence for `try_to_wake_up`, `__wake_up_common`, and `autoremove_wake_function`, then map the current Breenix waitqueue wake path through scheduler drain sites and interrupt/task context callers.

## 2026-04-18T22:12:00Z
M1 findings: Linux `__wake_up_common_lock` holds `wq_head->lock` while invoking each waiter's wake function (`wait.c:99-108`), `autoremove_wake_function` calls `default_wake_function` and removes the entry only on successful wake (`wait.c:382-389`), and `try_to_wake_up` performs the state match/state transition and runqueue enqueue before returning (`core.c:4186-4375`). Breenix `WaitQueueHead::wake_up` currently pops waiters under the waitqueue lock and calls `isr_unblock_for_io` (`waitqueue.rs:104-119`), while the scheduler drains the ring and calls `unblock_for_io` later from `schedule()`/`schedule_deferred_requeue()` (`scheduler.rs:671-680`, `scheduler.rs:921-933`).

## 2026-04-18T22:17:00Z
M1 validation passed with `test -s docs/planning/f32f-immediate-wake/audit.md && rg "Linux Citations|Breenix Findings|Conclusion" docs/planning/f32f-immediate-wake/audit.md`. Next I will commit the audit docs, then start M2 by adding a scheduler helper for immediate task-context waitqueue wake.
