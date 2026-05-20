# Turn 11 Suspect Deep Dive

Carrier: scheduler dequeue validation in `cb73f6e3`.

The xHCI split exonerates the xHCI conversion for this specific regression:

- Variant A restored the pre-Turn-5 xHCI polling code and still failed with CPU0 stuck at tick 5.
- Variant B kept the Turn 5 IRQ-driven xHCI code, including `MSI_EVENT_COUNT=60` before timer init, and CPU0 advanced past 45000 ticks.

## What Changed In The Scheduler

`cb73f6e3` replaced the old dequeue logic in both scheduler paths:

- `schedule()`
- `schedule_deferred_requeue()`

Before `cb73f6e3`, dequeue skipped only terminated threads. Otherwise it trusted queue membership and returned the next thread ID from the local queue or a stolen remote queue.

After `cb73f6e3`, dequeue calls `pop_next_dispatchable_thread()` and `pop_next_dispatchable_thread_excluding()`. These use `queued_thread_is_dispatchable()` to reject entries when:

- The thread state is not `Ready`.
- The thread is current on another CPU.
- The thread is an idle thread for another CPU.
- The thread is in an AArch64 deferred requeue slot.

The important detail is that rejected entries are popped and not restored to a queue. That makes the validation destructive.

## Why This Is Unsafe

The AArch64 scheduler intentionally has transitional states during a context switch:

- `schedule_deferred_requeue()` can publish an outgoing thread as `Ready` while holding it in `previous_thread` until the context-save tail runs.
- The old CPU remains authoritative for the outgoing thread until `commit_cpu_state_after_save()` and `requeue_thread_after_save()` finish.
- Wake paths also intentionally avoid queuing a thread that is still current or deferred, because queueing it too early can double-schedule the same stack.

Those states are valid handoff states, not proof that a queue entry can be destroyed.

The Turn 8 filter moved this invariant to dequeue time, but dequeue does not have enough ownership information to know whether a non-dispatchable entry is stale garbage or a legitimate in-flight handoff. Dropping it can lose the only queue-reachable path back to runnable work.

## Failure Signature

Variant A shows the scheduler-side failure even with xHCI reverted:

- `[xhci] post-activation: MSI_EVENT_COUNT=1 ...`
- `[init] Breenix init starting (PID 1)`
- Freeze-watch reports `timer_ticks_cpu0=5` while other CPUs advance into the tens of thousands.
- Ready queues are empty in the watchdog snapshots: `rq_total=0`.
- `wake-attrib` and `enqueue-attrib` remain near the early-boot floor:
  - first sample: `schedule=2`, `timer=1`, `deferred=3`, `deferred_drained=2`
  - later sample before panic: `schedule=2`, `timer=23`, `deferred=3`, `deferred_drained=2`

That matches destructive dequeue loss: the system has made a small amount of early scheduler progress, then no queue-reachable work remains for the path that should continue init and spawn userland services.

Variant B is the opposite:

- `[xhci] post-activation: MSI_EVENT_COUNT=60 ...`
- Init spawns the expected services.
- CPU0 advances from hundreds of ticks to over 45000 ticks.
- Wake/enqueue counters grow continuously.

The pre-Turn-8 scheduler does not get wedged by the same xHCI interrupt volume.

## Current CPU / Bootstrap Theory

The evidence does not point first at `current_cpu()` or TPIDR_EL1 returning the wrong CPU. Freeze-watch in Variant B shows coherent per-CPU current-thread movement, and the same xHCI interrupt volume does not break CPU0 when the scheduler filter is removed.

The stronger explanation is a chicken-and-egg in the new dequeue filter: it requires stable `Ready`, not-current, and not-deferred state at exactly the point where AArch64 scheduling deliberately uses temporary `Ready/current/deferred` combinations to keep context-save ownership safe.

## Secondary Risk In The Same Change

The `same_thread_requeued` path also changed behavior:

- The new code pushes the current thread, excludes it while searching for another dispatchable thread, and then avoids re-pushing it in one idle-switch path if `same_thread_requeued` is true.
- This is probably not the primary carrier by itself, but it combines badly with destructive dequeue. If the exclusion scan cannot find another dispatchable thread because candidates are dropped, the current thread can be left in an inconsistent queue/current relationship.

Turn 12 should treat the destructive dequeue filter as the primary suspect and the same-thread/exclusion path as part of the patch review surface.
