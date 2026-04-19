# F32h Linux AHCI Wake Audit

## Question

Linux completes AHCI I/O from interrupt context and can wake the blocked task
directly from that context. Breenix currently defers AHCI completion wakeups
through `isr_unblock_for_io()`. This audit checks whether the deferred ring was
introduced for a still-valid reason and what Linux does instead.

## Linux AHCI Completion Path

Linux AHCI IRQ handling reads the port status, acknowledges it, and completes
commands while still in the interrupt path:

- `/tmp/linux-v6.8/drivers/ata/libahci.c:1963-1966`: `ahci_port_intr()` reads
  `PORT_IRQ_STAT`, writes the value back to acknowledge, then calls
  `ahci_handle_port_interrupt()`.
- `/tmp/linux-v6.8/drivers/ata/libahci.c:1975-1980`:
  `ahci_multi_irqs_intr_hard()` does the same under `spin_lock(ap->lock)`.
- `/tmp/linux-v6.8/drivers/ata/libahci.c:1863-1888`: `ahci_qc_complete()`
  reads `PORT_SCR_ACT` and/or `PORT_CMD_ISSUE` to compute the still-active
  command mask, then calls `ata_qc_complete_multiple(ap, qc_active)`.
- `/tmp/linux-v6.8/drivers/ata/libahci.c:1896-1955`:
  `ahci_handle_port_interrupt()` handles error/SDB cases and then calls
  `ahci_qc_complete()` for completed commands.

Linux libata's multiple-completion helper is explicitly intended for IRQ
handlers:

- `/tmp/linux-v6.8/drivers/ata/libata-sata.c:630-641` documents
  `ata_qc_complete_multiple()` as the helper for completing in-flight commands
  from a low-level driver's interrupt routine.
- `/tmp/linux-v6.8/drivers/ata/libata-sata.c:664-685` computes `done_mask` and
  calls `ata_qc_complete(qc)` for each completed tag.
- `/tmp/linux-v6.8/drivers/ata/libata-core.c:4870-4872` documents
  `ata_qc_complete()` locking as `spin_lock_irqsave(host lock)`.
- `/tmp/linux-v6.8/drivers/ata/libata-core.c:4966-4969` reaches
  `__ata_qc_complete(qc)` for normal completion.
- `/tmp/linux-v6.8/drivers/ata/libata-core.c:4798-4834` clears active state and
  calls `qc->complete_fn(qc)`.

The block layer then ends the request and bios:

- `/tmp/linux-v6.8/block/blk-mq.c:1058-1063`: `blk_mq_end_request()` calls
  `blk_update_request()` and `__blk_mq_end_request()`.
- `/tmp/linux-v6.8/block/blk-mq.c:895-987`: `blk_update_request()` advances
  request bios and calls `req_bio_endio()`.
- `/tmp/linux-v6.8/block/blk-mq.c:765-793`: `req_bio_endio()` calls
  `bio_endio()` when a bio is fully consumed.
- `/tmp/linux-v6.8/block/bio.c:1576-1608`: `bio_endio()` invokes
  `bio->bi_end_io(bio)`.
- `/tmp/linux-v6.8/block/bio.c:1352-1355`: `submit_bio_wait_endio()` wakes the
  waiter by calling `complete(bio->bi_private)`.

## Linux completion() Wake Path

Linux `complete()` is IRQ-safe because it uses raw spinlocks and the scheduler's
real wake path:

- `/tmp/linux-v6.8/kernel/sched/completion.c:16-25`:
  `complete_with_flags()` takes `x->wait.lock` with `raw_spin_lock_irqsave()`,
  increments `x->done`, calls `swake_up_locked()`, then unlocks.
- `/tmp/linux-v6.8/kernel/sched/completion.c:45-48`: `complete()` is a thin
  wrapper around `complete_with_flags(x, 0)`.
- `/tmp/linux-v6.8/kernel/sched/swait.c:21-30`: `swake_up_locked()` selects the
  first waiter and calls `try_to_wake_up(curr->task, TASK_NORMAL, wake_flags)`
  before removing it from the wait list.
- `/tmp/linux-v6.8/kernel/sched/swait.c:38-40` explicitly notes that the locked
  all-waiters variant is designed for hard interrupt context and interrupt
  disabled regions.
- `/tmp/linux-v6.8/kernel/sched/core.c:4186-4222`: `try_to_wake_up()` is the
  core wake operation, documented as atomically changing a matching task state
  to runnable and enqueueing it if it is not already queued/runnable.
- `/tmp/linux-v6.8/kernel/sched/core.c:4253-4369`: the implementation uses
  `p->pi_lock`, memory barriers, `p->on_rq`/`p->on_cpu` ordering, and runqueue
  queueing rather than deferring to an out-of-band ring.
- `/tmp/linux-v6.8/kernel/sched/core.c:4499-4507`: `wake_up_process()` and
  `wake_up_state()` are direct wrappers over `try_to_wake_up()`.

The key Linux property is not merely "wake from IRQ". It is "wake from IRQ while
holding only scheduler-designed raw spinlocks and using the same state/on-rq
protocol as normal task wakeups."

## Why Breenix Introduced the Deferred Ring

Git history shows the reason directly:

- Commit `4caa26395bae48ea134b30867099f38c82704514`
  (`perf: lock-free ISR wakeup buffer to eliminate SCHEDULER contention from
  ISR`) introduced the current `isr_unblock_for_io()` buffer.
- The commit message says the AHCI ISR previously called
  `with_scheduler(|s| s.unblock_for_io(tid))`, spinning on the global
  `SCHEDULER` mutex with IRQs masked on CPU0. Contention from seven other CPUs
  kept CPU0 IRQ-masked for milliseconds, starving its timer (`5 ticks in 30s`
  versus roughly 4000 expected) and stalling AHCI SPI34.
- The replacement was a per-CPU atomic slot buffer: the ISR pushes the tid,
  sets reschedule state, and the scheduler drains all buffers later under its
  normal lock.

That reason was valid at introduction time. Breenix did not have Linux's
fine-grained IRQ-safe scheduler wake machinery. Calling through the global
scheduler mutex from hard IRQ context was not equivalent to Linux
`try_to_wake_up()`; it was an IRQ-masked spin on a contended global lock.

## Is The Reason Still Valid?

Partly yes, partly no.

Still valid:

- Breenix still has a global `SCHEDULER: Mutex<Option<Scheduler>>` for broad
  scheduler state. Reintroducing `with_scheduler()` from AHCI hard IRQ would
  recreate the exact `4caa2639` failure mode.
- Linux's direct IRQ wake is safe because the locks are raw spinlocks designed
  for IRQ context (`completion.c:20`, `swait.c:21-30`, `core.c:4253-4369`), not
  because immediate wakeups are inherently safe.

Not sufficient anymore:

- F32e/F32f moved task-context waitqueue wake toward Linux parity, including a
  tighter prepare-to-wait sequence and immediate task-context wake. That leaves
  the ISR completion path as the outlier.
- F32h evidence shows the deferred buffer can drain promptly in the captured
  failures, but the architecture still has a semantic mismatch with Linux:
  completion wake is not one atomic scheduler wake operation. It is split into
  "publish tid into side buffer" and "some later scheduler entry converts state
  and queues the task."
- That split creates extra states Linux avoids. A task can be completed but not
  yet scheduler-visible until a drain runs, and the drain depends on scheduler
  progress on some CPU. The captured F32h failures show CPU0/global tick
  progress stops while other timer IRQs continue, so the design still depends on
  exactly the progress mechanism that is suspect.

## Audit Conclusion

Linux's AHCI path completes I/O in IRQ context and reaches `complete()`, which
immediately calls the scheduler wake path (`try_to_wake_up(..., TASK_NORMAL, ...)`)
through IRQ-safe raw spinlocks.

Breenix's deferred ISR wake buffer was introduced for a real reason: avoiding
global scheduler mutex contention from hard IRQ context. The fix should not
revert to the old `with_scheduler()` call. However, the deferred ring is also not
Linux parity. The Linux-parity direction is an IRQ-safe immediate wake path that
uses a scheduler-safe lock subset, not the global mutex and not an unbounded
deferred side channel.
