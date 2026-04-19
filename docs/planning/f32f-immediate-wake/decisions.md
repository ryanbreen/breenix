# Decisions - F32f Immediate Wake Under Waitqueue Lock

## 2026-04-18T22:02:39Z - Initial Factory Scope
**Choice:** Keep this run scoped to auditing and implementing Linux-parity immediate task-context waitqueue wake only.
**Alternatives considered:** Broader AHCI, CPU routing, timer fallback, or Parallels-specific mechanisms.
**Evidence:** The task contract explicitly identifies deferred waitqueue wake latency as the remaining Linux-parity gap and prohibits those alternative mechanisms.

## 2026-04-18T22:22:00Z - Immediate Wake Routing
**Choice:** Route waitqueue wakes through an immediate scheduler helper only when not in interrupt context; keep hard IRQ completion and interrupt-origin waitqueue wakes on `isr_unblock_for_io`.
**Alternatives considered:** Convert all `isr_unblock_for_io` callers to immediate wake, or leave waitqueue wake fully deferred.
**Evidence:** Linux waitqueue wake invokes the wake function while holding `wq_head->lock` (`wait.c:73-108`), while Breenix completion wakes explicitly require the lock-free ISR buffer (`completion.rs:466-496`, `scheduler.rs:2564-2573`).
