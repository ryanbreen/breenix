# F16 isr_unblock_for_io() Idle-Scan Audit

Date: 2026-04-16
Branch: `probe/f16-idle-scan-fix`
Base: `origin/main` / `2a7f96c2`

## Current Breenix Algorithm

### Entry and first mutation

`kernel/src/task/scheduler.rs:2530` defines `isr_unblock_for_io(tid)`. The first
stateful operation is diagnostic publication to the AHCI ring at
`scheduler.rs:2531-2543`. The first scheduling-state read is the raw MPIDR CPU ID
read at `scheduler.rs:2544`, implemented as `mrs mpidr_el1` and `mpidr & 0xff` at
`scheduler.rs:2729-2737`.

The first real wake-state mutation is the per-CPU ISR wake-buffer push:

- `scheduler.rs:2558-2586`: if the current CPU index is in range, publish
  `tid` to `ISR_WAKEUP_BUFFERS[cpu]`.
- `scheduler.rs:94-105`: `IsrWakeupBuffer::push()` scans exactly 32 slots and
  performs one `compare_exchange(EMPTY, tid)` per slot. This is a bounded
  lock-free probe, not an unbounded CAS loop.

### Wake buffer

The wake buffer is per-CPU and lock-free:

- `scheduler.rs:75-81`: `ISR_WAKEUP_SLOTS = 32`, `IsrWakeupBuffer` stores an
  array of `AtomicU64` slots.
- `scheduler.rs:119`: the system has eight per-CPU ISR wakeup buffers.
- `scheduler.rs:108-116`: draining swaps each slot back to empty and appends
  non-empty tids to a temporary vector.
- `scheduler.rs:912-923`: `schedule_deferred_requeue()` drains all ISR wake
  buffers while holding the scheduler lock, then calls `unblock_for_io(tid)`.

This means the ISR only publishes a wake request. The real thread state
transition and ready-queue insertion occur later under the scheduler lock.

### set_need_resched

After publishing the wake buffer entry, `isr_unblock_for_io()` calls
`set_need_resched()` at `scheduler.rs:2600`.

- `scheduler.rs:2470-2477`: `set_need_resched()` sets the global
  `NEED_RESCHED` atomic and, on AArch64, the current CPU's per-CPU
  `need_resched`.
- `kernel/src/per_cpu_aarch64.rs:20-32`: the per-CPU flag is byte field
  `need_resched`.
- `kernel/src/per_cpu_aarch64.rs:221-228`: `set_need_resched()` writes that
  per-CPU field through the HAL percpu accessors.
- `scheduler.rs:2479-2503`: the interrupt-return path checks and clears both
  the per-CPU flag and the global flag.

The local CPU already has enough information to schedule after IRQ return.

### Idle-CPU scan

The audited stall corridor starts after `UNBLOCK_AFTER_NEED_RESCHED` at
`scheduler.rs:2601-2613` and ends at `UNBLOCK_SCAN_DONE` at
`scheduler.rs:2698-2709`.

Inside that corridor Breenix performs a broadcast idle scan:

- `scheduler.rs:2614-2629`: enters the AArch64-only scan and reads
  `cpus_online()`.
- `kernel/src/arch_impl/aarch64/smp.rs:274-277`: `cpus_online()` is a single
  `AtomicU64::load(Ordering::Acquire)`.
- `scheduler.rs:2642-2697`: loops over `0..online.min(MAX_CPUS)`.
- `scheduler.rs:2655`: tests `target != cpu && is_cpu_idle(target)`.
- `scheduler.rs:210-215`: `is_cpu_idle()` is a single relaxed load from
  `CPU_IS_IDLE[target]`.
- `scheduler.rs:131-143`: `CPU_IS_IDLE` is a lock-free idle-state hint updated
  by scheduler decisions.
- `scheduler.rs:2680-2683`: for each idle target, sends SGI 0 via
  `gic::send_sgi()`.

The loop itself is bounded by `MAX_CPUS`, and the idle flag read is not a lock or
CAS. The F16 audit therefore does **not** find an unbounded CAS loop, a
contended lock-free queue retry loop, or an iterated mutable list head in this
function. The problematic construct is instead the ISR-context broadcast
selection algorithm: Breenix scans every online CPU using lock-free idle hints
and can send multiple reschedule SGIs from the AHCI hard-IRQ path after the wake
event was already published.

### SGI call-site

The SGI call-site under audit is only:

- `scheduler.rs:2680-2683`: `gic::send_sgi(SGI_RESCHEDULE, target)`.

F11/F14 already audited the GIC layer. The relevant caller-side detail is that
this call happens inside the idle scan and may execute once per idle CPU.

## Linux Equivalent

### try_to_wake_up() to ttwu_queue()

Linux's wake path serializes on the task, selects a task CPU, then queues the
wake on that CPU:

- `/tmp/linux-v6.8/kernel/sched/core.c:4222-4256`:
  `try_to_wake_up()` handles `p == current` locally, otherwise takes
  `p->pi_lock` and validates the task state.
- `/tmp/linux-v6.8/kernel/sched/core.c:4282-4318`: after ordering against
  `p->on_rq` / `p->on_cpu`, Linux marks the task `TASK_WAKING`.
- `/tmp/linux-v6.8/kernel/sched/core.c:4339-4341`: if the task is still
  `on_cpu`, Linux first tries `ttwu_queue_wakelist(p, task_cpu(p), wake_flags)`
  to avoid spinning on that CPU.
- `/tmp/linux-v6.8/kernel/sched/core.c:4352-4364`: otherwise Linux waits until
  `p->on_cpu` clears, calls `select_task_rq()`, and migrates `p` only if the
  selected CPU differs.
- `/tmp/linux-v6.8/kernel/sched/core.c:4369`: Linux calls
  `ttwu_queue(p, cpu, wake_flags)` for that selected CPU.

### ttwu_queue_wakelist() pattern

Linux's remote wake queue is per-target-CPU:

- `/tmp/linux-v6.8/kernel/sched/features.h:42-46`: `TTWU_QUEUE` queues remote
  wakeups on the target CPU and processes them through the scheduler IPI.
- `/tmp/linux-v6.8/kernel/sched/core.c:3930-3944`:
  `__ttwu_queue_wakelist()` sets the target runqueue's `ttwu_pending` and queues
  the task's wake entry on that target CPU.
- `/tmp/linux-v6.8/kernel/sched/core.c:3978-4016`:
  `ttwu_queue_cond()` decides whether the target CPU should process the wake via
  its wakelist; it does not scan all idle CPUs.
- `/tmp/linux-v6.8/kernel/sched/core.c:4018-4026`:
  `ttwu_queue_wakelist()` queues and returns true only for that selected CPU.
- `/tmp/linux-v6.8/kernel/sched/core.c:4038-4049`: `ttwu_queue()` either uses
  the wake list or directly locks the selected runqueue and activates the task.

The underlying single-call queue sends an IPI only when the target list
transitions from empty to non-empty:

- `/tmp/linux-v6.8/kernel/smp.c:349-383`: `__smp_call_single_queue()` uses
  `llist_add(node, &per_cpu(call_single_queue, cpu))`; if that add returns true,
  it calls `send_call_function_single_ipi(cpu)`.
- `/tmp/linux-v6.8/kernel/smp.c:111-118`: `send_call_function_single_ipi(cpu)`
  prepares and sends a single target CPU IPI.

On ARM64, Linux's reschedule IPI is a single target cross-call:

- `/tmp/linux-v6.8/arch/arm64/kernel/smp.c:1044-1047`:
  `arch_smp_send_reschedule(cpu)` calls `smp_cross_call(cpumask_of(cpu),
  IPI_RESCHEDULE)`.
- `/tmp/linux-v6.8/arch/arm64/kernel/smp.c:1050-1057`: wakeup IPI aliases to
  `smp_send_reschedule(cpu)`.

### No idle-CPU broadcast scan

Linux does not wake a task by looping over every CPU that appears idle. It either
uses the task's existing CPU (`task_cpu(p)`), or calls `select_task_rq()` to pick
one target CPU, then queues the wake on that target CPU.

## Divergence

Breenix publishes the wake to a per-current-CPU ISR wake buffer, sets local/global
`need_resched`, and then performs an additional AArch64-only broadcast scan over
all online CPUs that look idle (`scheduler.rs:2614-2697`). Each selected idle CPU
receives a reschedule SGI from hard IRQ context (`scheduler.rs:2680-2683`).

Linux's corresponding wake path does not do this. Linux targets the task's
selected CPU and queues the wake work there. Its lockless wake-list path adds one
node to the target CPU's per-CPU list and sends at most the target CPU's IPI when
the list was previously empty.

The audited Breenix construct is therefore:

```text
scheduler.rs:2642-2697
for target in 0..online.min(MAX_CPUS) {
    if target != cpu && is_cpu_idle(target) {
        gic::send_sgi(SGI_RESCHEDULE, target)
    }
}
```

This is not an unbounded spin loop, but it is a scheduler-internal Linux
divergence in the exact failing corridor. It extends the AHCI ISR with a
lock-free idle-CPU enumeration and repeated cross-CPU SGI sends after the wake
buffer publication and `need_resched` flag are already sufficient for local
forward progress.

## Proposed Fix

Remove the ISR-context idle-CPU broadcast scan from `isr_unblock_for_io()` and
replace it with a selected-target wake:

- Record the current CPU when a thread enters `BlockedOnIO`.
- In `isr_unblock_for_io(tid)`, look up that CPU without taking the scheduler
  lock.
- Push the wake entry to that CPU's ISR wake buffer.
- Send at most one reschedule SGI to that target CPU when it differs from the
  IRQ CPU.

Rationale:

- The wake event is already published to an ISR wake buffer before the scan
  (`scheduler.rs:2558-2572`).
- The current CPU is already requested to schedule after IRQ return via
  `set_need_resched()` (`scheduler.rs:2600`, `scheduler.rs:2470-2477`), but a
  remote waiter still needs its owning CPU nudged to drain the wake promptly.
- Scheduler code drains all ISR wake buffers on the next schedule entry
  (`scheduler.rs:912-923`).
- Linux's equivalent does not perform idle-CPU broadcast selection; it targets
  one selected CPU and sends a single IPI when queueing remote wake work.

Expected behavioral signature:

- The AHCI ring should no longer emit `UNBLOCK_SCAN_START`, `UNBLOCK_SCAN_CPU`,
  or `UNBLOCK_SCAN_DONE` from `isr_unblock_for_io()`.
- `UNBLOCK_BEFORE_SEND_SGI` / `UNBLOCK_AFTER_SEND_SGI` should appear only for a
  single selected remote wake target, not once per idle CPU.
- Stalls between `UNBLOCK_AFTER_NEED_RESCHED` and `UNBLOCK_SCAN_DONE` should
  disappear because the scan corridor no longer exists.
- Wake latency should be bounded by either the IRQ CPU's interrupt-return
  scheduling path for local wakes or one selected reschedule SGI for remote
  waiters.
