# Turn 8 scheduler map

## Scope

Turn 8 keeps the Turn 5 xHCI IRQ-completion patch applied as a race amplifier and maps the AArch64 deferred-requeue / inline-schedule path. The relevant ownership boundary is between `kernel/src/task/scheduler.rs` and `kernel/src/arch_impl/aarch64/context_switch.rs`.

## Dispatchability fields

A thread is safe to dispatch only when all of these are true:

- `Thread.state == ThreadState::Ready`
- the thread ID was removed from exactly one per-CPU ready queue
- no CPU has `cpu_state[cpu].current_thread == Some(tid)`, except the current CPU same-thread/no-switch case
- no CPU has `cpu_state[cpu].previous_thread == Some(tid)`
- the thread is not an idle thread belonging to another CPU

Before this turn, both `Scheduler::schedule()` and `Scheduler::schedule_deferred_requeue()` popped from `per_cpu_queues` and skipped only `ThreadState::Terminated`. They did not validate `Ready`, remote `current_thread`, or `previous_thread`.

## Inline-saved frame fields

Inline kernel scheduling is tracked in the thread and per-CPU context-switch state:

- `Thread.saved_by_inline_schedule`
- `Thread.inline_schedule_caller_lr`
- `Thread.inline_schedule_saved_sp`
- `INLINE_SCHEDULE_STATE[cpu].scheduler_ptr`
- `INLINE_SCHEDULE_STATE[cpu].old_thread_id`
- `INLINE_SCHEDULE_STATE[cpu].new_thread_id`
- `INLINE_SCHEDULE_STATE[cpu].should_requeue_old`

`schedule_from_kernel()` sets `saved_by_inline_schedule = true` after saving the outgoing kernel frame. The dispatcher later uses that marker to do a ret-based restore instead of an ERET restore.

## Deferred requeue fields

Deferred requeue uses two ownership markers:

- scheduler-owned: `cpu_state[cpu].previous_thread`
- context-switch-owned: `DEFERRED_REQUEUE[cpu]`

The intended ordering is:

1. `schedule_deferred_requeue()` observes the current thread.
2. If the current thread remains runnable and is not already queued, it sets `cpu_state[cpu].previous_thread = Some(old_tid)` and marks the thread `Ready`, but does not push it to a ready queue.
3. `context_switch.rs` saves the outgoing context.
4. `commit_cpu_state_after_save(new_tid)` publishes the new CPU owner.
5. The old thread is placed in `DEFERRED_REQUEUE[cpu]`.
6. A later scheduling entry drains `DEFERRED_REQUEUE[cpu]` and calls `requeue_thread_after_save(old_tid)`.

This creates a deliberate period where a thread may be `Ready` while not yet queue-dispatchable. During that period, `previous_thread`/`DEFERRED_REQUEUE` is the ownership guard.

## Load-bearing trace markers

`trace_defer_requeue()` in `context_switch.rs` emits the Turn 5 trace markers:

- `DEFER_REQUEUE_STAGE`
- `DEFER_REQUEUE_SP`
- `DEFER_REQUEUE_ELR`
- `DEFER_REQUEUE_X30`
- `DEFER_REQUEUE_FLAGS`

The marker stores the stage, thread ID, auxiliary thread ID, thread state, inline-saved flag, blocked-in-syscall flag, has-started flag, owner flag, and low 32 bits of SP/ELR/X30. Turn 7 boot 17 showed a context switch to tid 14 while another CPU still had tid 14 in deferred/inline state, matching the unsafe publication window.

## Identified gap

The wakeup and deferred-requeue paths try to avoid adding threads that are current or deferred. The ready-queue consumer was weaker: it trusted existing queue entries and filtered only terminated threads. A stale or duplicate queue entry could therefore survive the producer-side checks, then later be popped by a different CPU and dispatched while the thread was still:

- current on another CPU,
- in another CPU's `previous_thread`,
- or already changed away from `Ready` after a duplicate dispatch.

The minimal invariant belongs at dequeue time as well as enqueue time: a ready-queue entry is only a candidate, not proof of dispatchability.
