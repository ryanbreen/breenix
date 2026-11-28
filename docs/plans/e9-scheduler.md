# E9: the scheduler — design and PR plan

Ruling DEV-41(2), from the toy-OS audit's R18, R26 and the x86 part of R19, plus softirqs and the
preemption-count layout, which DEV-37(1c) places here as a clean-room rewrite from brief B3. Written
2026-11-27 against `origin/main` at b22a066f. This is a design document. It changes no code. E9 starts
coding only after it merges (DEV-41(2)).

E9's finish line:

- both architectures dispatch under CPU-local run-queue locks, with thread ownership as the serialisation
  boundary. No dispatch waits with interrupts masked for another CPU: a peer's lock is only tried, and a
  peer holding this CPU's lock defers this CPU's dispatch rather than stalling it;
- ready threads sit in bitmap-and-FIFO queues for the real-time ranks and in a fair class accounted by
  runtime for the rest, so selection and enqueue do bounded work;
- every CPU arms one-shot deadlines, and an idle CPU takes no periodic tick; <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- the thread registry grows with demand and dead threads retire through per-CPU queues;
- CPU identity comes from firmware with no fixed CPU ceiling, and GICv3 SGIs are routed by affinity;
- x86 uses PCIDs and sends TLB shootdowns as ordinary IPIs;
- deferred work and the CPU nesting word are rewritten clean-room from brief B3.

## 0. Provenance

Clean room (O6, DEV-34(5), DEV-36(2), DEV-37(1c)). Two areas carry an extra rule.

- **Deferred work (softirqs) and the preemption-count layout** (assessment area A13). For these, this
  design used only brief B3 (`docs/cleanroom/derivation-assessment.md`, Part B), the public signatures of
  `kernel/src/task/softirqd.rs` (its `pub` lines), the public signatures of the nesting accessors in
  `kernel/src/per_cpu.rs`, `kernel/src/per_cpu_aarch64.rs` and `kernel/src/arch_impl/aarch64/percpu.rs`, the
  layout constants B3 names (`kernel/src/arch_impl/aarch64/constants.rs:191-231`), and the callers of all
  of these. It did not read those functions' bodies, `kernel/src/task/softirq_tests.rs`'s bodies, or
  Part A of the assessment. The layout in §5 is chosen from what Breenix needs (B3), not from the
  existing constants. <!-- claim-lint:ok: provenance under ruling DEV-37(1c); the brief is docs/cleanroom/derivation-assessment.md. -->
- **The waitqueue.** E2.1 (#3260) already replaced it from brief B1. This design touches only its public
  interface.

Everything else here comes from:

- our own code, read freely apart from the bodies listed above;
- the Intel 64 and IA-32 SDM, the Arm Architecture Reference Manual, the Arm GICv3/v4 specification,
  PSCI, the ACPI specification and the Devicetree specification;
- POSIX.1-2017 and the Linux man pages, for user-visible scheduling semantics;
- the textbooks and papers in §16.

No other operating system's source was read. Some of our existing scheduler comments compare a
mechanism with another system by name (for example `kernel/src/task/scheduler.rs:2118` and `:4668`, and
`kernel/src/task/scheduler/rq.rs:122`). Those comments are part of the historical record (DEV-35(2)).
This design does not build on them, and E9's new code carries no such comparisons (DEV-34(5)).

The audit (`~/Downloads/breenix-toy-os-audit-2026-11-27.html`, sections 2 to 4), the sched/SMP lane
findings (`~/Library/Caches/fastr-rollout/2026-11-27/toy-os-audit/sched-smp-findings.md`), the E2 design
(`docs/plans/e2-sleepable-contexts.md`) and rulings DEV-23 to DEV-41 set the scope.

## 1. What the code does today

Line numbers are at b22a066f.

### 1.1 ARM64 dispatches under one lock (R18)

- **The lock.** `SCHEDULER` is one `spin::Mutex<Option<Scheduler>>` (`kernel/src/task/scheduler.rs:388`).
  On ARM64 every dispatch takes it through `lock_for_context_switch` (`scheduler.rs:1280`): the interrupt
  return (`kernel/src/arch_impl/aarch64/context_switch.rs:5313`, `:5443`), the inline switch from kernel
  code (`schedule_from_kernel`, `:6804`) and the exit switch (`:6111`). <!-- claim-lint:ok: read from kernel/src/task/scheduler.rs and the other files this bullet cites, at b22a066f. -->
- **Across the pivot.** The inline and exit switches leak the guard (`:6127`, `:6965`), pivot to the
  CPU's scheduler stack, and release the lock there with `force_unlock_scheduler` (`:6307`, `:6528`,
  `:6651`). Every other CPU's dispatch waits for that release, with interrupts masked. <!-- claim-lint:ok: read from kernel/src/task/scheduler.rs and the other files this bullet cites, at b22a066f. -->
- **Under the lock.** The pass reclaims stalled queues, delivers pinned wakes and drains the R17 wake
  inboxes (`schedule_deferred_requeue`, `scheduler.rs:4613-4660`), then picks. Thread lookups go through
  `Scheduler::get_thread`, a linear scan of the thread list (`scheduler.rs:7760-7765`).
- **What is already per CPU.** The ready queues (`per_cpu_queues`, `scheduler.rs:3028`) and
  `CpuSchedulerState` (`:2902`) are per CPU. The lock is not. The ARM64 timer heap is one global heap
  (`TimerHeaps`, `:2970`, non-x86 arm).
- **Task-context callers** of `with_scheduler_lock` wait with interrupts enabled and take the lock masked
  (`scheduler.rs:805-822`). `scheduler.rs` has 88 lines that name a whole-scheduler entry point
  (`with_scheduler`, `with_scheduler_lock`, `lock_scheduler`, `try_lock_scheduler`,
  `lock_for_context_switch`), and the ARM64 `context_switch.rs` has 12.

### 1.2 x86 dispatches under per-CPU run-queue locks (#1649), the design E9 ports

`kernel/src/task/scheduler/rq.rs` (x86 only: `scheduler.rs:390-391`):

- **One lock per CPU** (`RQ_LOCKS`, `rq.rs:68`). It covers that CPU's queue, its `CpuSchedulerState`, its
  timer heap and the scheduling state of every thread it owns (`rq.rs:1-14`). It is held with interrupts
  masked. <!-- claim-lint:ok: read from kernel/src/task/scheduler/rq.rs and the other files this bullet cites, at b22a066f. -->
- **Owner index.** Each indexed thread's slot records its owner CPU beside its address (`Slot`,
  `rq.rs:145-165`). `owned_thread(tid, cpu)` returns the thread only while `cpu` owns it and the caller
  holds `cpu`'s lock (`rq.rs:360-371`). A change of owner holds both CPUs' locks (`set_owner`, `:373`).
- **Remote work never waits.** `try_remote` tries a peer's lock once (`rq.rs:619-626`). `wake` reads the
  owner, try-locks it, and checks the owner again after taking the lock (`rq.rs:1159-1174`). A wake whose
  owner stays busy is published to that CPU's durable inbox (`hold`, `:1254`; `wake_or_hold`, `:1364`,
  after 256 tries). <!-- claim-lint:ok: read from kernel/src/task/scheduler/rq.rs and the other files this bullet cites, at b22a066f. -->
- **Lock-free summaries.** Load, idle, queue length and current thread are published on release
  (`Held::publish`, `rq.rs:562-583`) and read by placement without locks.
- **Stack custody.** `LEAVING_STACK` names the thread whose kernel stack a CPU is still leaving
  (`scheduler.rs:2120`). Selection passes such a thread over (`x86_stack_owner_elsewhere`, `:2581`) and
  does not wait for it. `wait_for_stacks_x86` (`:2615-2626`) sets this CPU's bit in the owner's
  `LEAVING_STACK_WAITERS` and then rechecks `LEAVING_STACK`. A clear it sees makes this CPU ask itself to
  pick again. `release_leaving_stack` (`:2224-2236`) clears the mark, then swaps the waiter set to empty
  and sends each waiter a reschedule request.
- **Whole-scheduler holders** take the list lock and every run-queue lock (`lock_all`, `rq.rs:87`), and
  recompute the owners and summaries before release (`refresh`, `:472`). New threads are published on a
  lock-free list (`splice_spawned`, `:2231`). <!-- claim-lint:ok: read from kernel/src/task/scheduler/rq.rs and the other files this bullet cites, at b22a066f. -->

The audit and the sched lane rate this design sound and say to keep it (lane findings, "Keep"). ARM64 has
its own stack-custody mark: `switch_save` marks the outgoing thread until SP leaves its stack, and
selection skips a marked thread without waiting (`kernel/src/arch_impl/aarch64/switch_save.rs:1-16`,
strict-gate ruling S8). The older `DEFERRED_REQUEUE` slot remains for a switch that cannot stage its frame
(`context_switch.rs:1841-1869`).

### 1.3 Ready structures and placement (R26, part)

- Ready queues are `VecDeque<u64>` of thread IDs. Ranking scans and rotates the queue
  (`promote_best_of`, `rq.rs:755`; `scheduler.rs:3848`, `:3938`). Higher-ranked stealing scans every
  peer's queue (`rq.rs:1509-1553`). Membership tests are linear (`queue().contains`, for example
  `rq.rs:1082`, `:1231`). <!-- claim-lint:ok: read from kernel/src/task/scheduler/rq.rs and the other files this bullet cites, at b22a066f. -->
- Ranks: SCHED_IDLE 0, SCHED_OTHER and SCHED_BATCH 1, real-time 2 to 100, the kernel's fatal-exit thread
  101 (`SchedPolicy::own_rank`, `kernel/src/task/thread.rs:661-668`). All normal threads share rank 1, so
  within it the order is FIFO with a fixed quantum. A process's nice value is stored but gives no share
  (`kernel/src/syscall/priority.rs:1-5`). <!-- claim-lint:ok: read from kernel/src/task/thread.rs and the other files this bullet cites, at b22a066f. -->
- Wake placement picks the least-loaded CPU by a scan of all CPUs (`least_loaded`, `rq.rs:704-710`). <!-- claim-lint:ok: read from kernel/src/task/scheduler/rq.rs and the other files this bullet cites, at b22a066f. -->

### 1.4 Ticks, quanta and deadlines (R26, part)

- **x86** runs the LAPIC timer periodically at 200 Hz (`kernel/src/time/timer.rs:18`, `init` at `:69`). The
  one-shot overlay for selected sleeps returns to periodic mode after the next regular tick
  (`kernel/src/arch_impl/x86_64/apic/wakeup.rs`, `apic/clockevent.rs`).
- **ARM64** rearms a 1 kHz interval on every CPU, idle or not (`TARGET_TIMER_HZ`,
  `kernel/src/arch_impl/aarch64/timer_interrupt.rs:46`; `rearm_timer`, `:1270-1312`), with a ten-tick
  quantum (`:39`). A 64-entry per-CPU fine-deadline heap shortens the interval for selected sleeps
  (`:239`, `:470-501`; `kernel/src/time/deadlines.rs`). When it is full the caller keeps the coarse
  scheduler timeout. <!-- claim-lint:ok: read from kernel/src/arch_impl/aarch64/timer_interrupt.rs and the other files this bullet cites, at b22a066f. -->
- **Tick time.** Only timer interrupts advance `TICKS`. On x86 the interrupt sets it from the TSC's
  elapsed periods (`time/timer.rs:119-128`); on ARM64 the interrupt advances it from the counter
  (`advance_ticks_to`, `:142`). Either way, a CPU that takes no timer interrupt does not move it.
  Liveness uses ticks: a CPU that has not entered the scheduler in
  20 ticks on ARM64 or 2 on x86 is treated as stalled (`CPU_STALL_TICKS`, `scheduler.rs:1972`, `:1981`).
- **xAPIC sends.** `send_ipi` waits for delivery with interrupts masked (`apic.rs:332-371`,
  `wait_delivery` at `:395`). `try_send_reschedule` skips a busy ICR and relies on the target's periodic
  tick to see its inbox (`apic.rs:370-373`).

### 1.5 Thread index and reclamation (R26, part; #1712)

- **x86.** `INDEX` is a fixed 4,096-slot open-addressed table (`rq.rs:105`, `:167`), with 256 slots kept
  for kernel threads (`KERNEL_RESERVE`, `:116`). Fork and clone reserve a slot or fail with EAGAIN
  (`reserve`, `:127`). Reclamation sweeps under the list lock in batches of 16 (`RECLAIM_BATCH`, `:2322`;
  `reclaim`, `:2329`).
- **ARM64.** A 1,024-bucket chained index of each thread's wake record serves interrupt-side lookups
  (`kernel/src/task/scheduler/wake_index.rs:12`). Scheduler lookups use the linear `get_thread`.
- **Pins.** Lock-free readers hold a global pin count; a thread unlinked from the index is freed only
  when no pin is held (`rq.rs:383-427`; `wake_index.rs:13-41`).

### 1.6 CPU identity and interrupt routing (R26, part)

- `MAX_CPUS` is 8 on both architectures (`scheduler.rs:1961`, `:1967`;
  `kernel/src/arch_impl/aarch64/smp.rs:16`). Under `kernel/src`, 233 lines size an array by `MAX_CPUS`,
  60 files name it, and about 30 lines build a CPU mask as `1 << cpu` in a `u64`.
- **x86** reads the MADT and clamps the count to `MAX_CPUS` (`CPUS_PRESENT`,
  `kernel/src/arch_impl/x86_64/smp.rs:78-80`).
- **ARM64** brings CPUs up by PSCI CPU_ON with the MPIDR set to the logical index, all other affinity
  fields zero (`smp.rs:424-426`). Firmware reports only a count (`firmware_cpu_count`,
  `kernel/src/platform_config.rs:822`). Redistributors are found at `cpu * 0x20000`
  (`kernel/src/arch_impl/aarch64/gic.rs:336`). <!-- claim-lint:ok: read from kernel/src/arch_impl/aarch64/smp.rs and the other files this bullet cites, at b22a066f. -->
- **GICv3 SGIs** write `ICC_SGI1R_EL1` with Aff3, Aff2 and Aff1 zero and the logical index as the target
  bit (`gic.rs:849-872`). <!-- claim-lint:ok: read from kernel/src/arch_impl/aarch64/gic.rs and the other files this bullet cites, at b22a066f. -->

### 1.7 TLB maintenance

- **ARM64** has per-address-space ASIDs since #3243 (`kernel/src/arch_impl/aarch64/ttbr0.rs:1-10`). That
  part of R19 is done, and E9 does not change it.
- **x86** has no PCIDs, so every CR3 write drops non-global translations. Shootdowns are NMIs, because the
  kernel spins on locks with interrupts masked (`kernel/src/memory/tlb.rs:15-49`). The initiator waits
  for every acknowledgement. <!-- claim-lint:ok: read from kernel/src/memory/tlb.rs and the other files this bullet cites, at b22a066f. -->

### 1.8 Deferred work and the nesting word (interface only; §0)

- **Interface.** `softirqd.rs` registers a handler per class (`:157`), raises by class or number (`:171`,
  `:177`), runs pending work (`:197`), queries pending (`:185`) and starts per-CPU daemons (`:323`,
  `:338`). It defines ten classes (`NR_SOFTIRQS`, `:45`).
- **Classes named.** Outside `softirqd.rs`, kernel code and its tests name NetRx 37 times, Timer 13,
  Tasklet and HiTasklet 9, IrqPoll 3, Rcu 3, and NetTx, Block, Hrtimer and Sched once each. B3 names the
  ones that must exist: timer, network receive and transmit, block, and the scheduler.
- **Callers.** Run sites are the interrupt exits (`kernel/src/per_cpu.rs`,
  `kernel/src/arch_impl/aarch64/exception.rs`) and `syscall/handlers.rs`. Hold-off is
  `bh_disable`/`bh_enable`. Lines under `kernel/src` that name the nesting accessors, definitions
  included: `preempt_disable` 143, `preempt_enable` 166, `preempt_count` 126, `in_interrupt` 46,
  `in_softirq` 28, `irq_enter` 21, `irq_exit` 33, `bh_disable` and `bh_enable` 9 each.
- **The count is per CPU.** E2 stores its park baseline per thread and reaches the count only through
  `preempt::depth()` and guard types (E2 §3.2, §11).
- **ARM64 migration hazard.** An update that computes the per-CPU address and then performs a separate
  read-modify-write can land on the wrong CPU if the thread migrates between the two (#1075). Any layout
  E9 chooses must keep the update CPU-local.

## 2. Invariants E9 establishes

Each is checked by a deterministic test named in §11.

- **S1, one owner.** Every live thread has exactly one owner CPU. Its scheduling state changes only under
  its owner's run-queue lock. A change of owner holds both CPUs' locks. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **S2, no dispatch waits on a peer.** A dispatch pass try-locks its own CPU's run-queue lock and at
  most try-locks a peer's. A busy peer is passed over, and a wake for one of its threads is published to
  its inbox. When a peer holds this CPU's lock, the dispatch is owed rather than waited for (§3.5), so a
  stopped peer cannot hold this CPU with interrupts masked. A peer holds another CPU's lock only for a
  bounded section: revalidate, one queue operation and the summary publish.
- **S3, stack custody.** No CPU dispatches a thread while another CPU still stands on that thread's
  kernel stack. Selection skips such a thread, records itself as a waiter and then rechecks the mark.
  The CPU that leaves the stack clears the mark, then notifies the waiters through a flag the idle
  recheck reads. No CPU waits for a stack.
- **S4, bounded work under the lock.** Enqueue, dequeue, membership and selection are O(1) or
  O(log n) in the CPU's runnable count. Balancing does a constant amount of work per pass.
- **S5, every wake lands.** A wake either transitions the thread under its owner's lock or is published
  to the owner's durable inbox (R17). Every publication that makes an inbox non-empty notifies the owner.
  A notification that cannot be sent stays owed until it is. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **S6, deadline completeness.** A CPU's next timer interrupt is armed at or before its earliest
  deadline: sleepers, the running thread's slice end and housekeeping. Correctness needs no periodic tick.
- **S7, no fixed ceilings.** The CPU count comes from firmware and the thread count from memory.
  Admission fails with an error before anything is published.
- **S8, deferred work.** B3's requirements 1 to 5 (§5).
- **S9, shootdown completeness.** No frame is freed while any CPU can still translate to it, PCID-tagged
  entries included. No initiator waits for an acknowledgement with interrupts masked.

## 3. Run-queue ownership on both architectures (E9.1)

### 3.1 One module, two architectures

E9.1 makes `task/scheduler/rq.rs` and `timer_groups.rs` architecture-neutral and moves ARM64 onto them.
The x86 behaviour does not change. What differs between the architectures moves behind one small trait,
so the shared logic is host-testable (§11):

```
trait RqArch {
    fn cpu() -> usize;                                   // this CPU's dense index
    fn stack_owner_elsewhere(tid: u64, me: usize) -> Option<usize>;   // S3
    fn kick(cpu: usize);                                  // reschedule IPI or SGI, deferred past unlock
    fn interrupts_masked() -> bool;
}
```

- **x86** implements `stack_owner_elsewhere` with `LEAVING_STACK` and `kick` with the existing deferred
  reschedule IPIs (`send_deferred_resched_ipis`).
- **ARM64** implements `stack_owner_elsewhere` with the `switch_save` mark and the `DEFERRED_REQUEUE`
  slot, and `kick` with `send_sgi_outside_scheduler_lock` (`scheduler.rs:699-714`), whose deferral moves
  from "while this CPU holds the scheduler lock" to "while this CPU holds any run-queue lock".

### 3.2 What the run-queue lock covers on ARM64

The same as on x86 (§1.2), plus the ARM64-only per-CPU state that today sits under `SCHEDULER`:

- `CpuSchedulerState::previous_thread` and `pending_next` (`scheduler.rs:2920`, `:2924`);
- `INLINE_SCHEDULE_STATE[cpu]` and the outgoing thread's inline-save fields;
- this CPU's R17 inbox drain (`apply_arm_wake_inboxes`, `scheduler.rs:6298`);
- this CPU's timer heap. `TimerHeaps` becomes per CPU on ARM64, as on x86.

**Pinned placement moves onto R17 records.** ARM64's held pinned wakes are delivered today by
`deliver_pinned_wakes_for_this_cpu`, through `pinned_wake_is_waiting_here`, which reads every CPU's queue
and `cpu_state` (`scheduler.rs:7316-7386`). That is sound only while one global lock covers every queue.
Under the home CPU's lock alone, a peer could grow its own queue during the scan and leave the reader
holding freed storage. So E9.1 deletes both functions on ARM64. x86 already skips them (`:7364`). ARM64
then uses x86's placement path:

- `hold(tid, WAKE_PLACE, home)` records the placement and the home CPU in the thread's R17 record
  (`rq.rs:1252-1268`);
- the home CPU's drain applies it through `place_held` (`rq.rs:1304-1330`). That function reads the owner
  from the index, works under the local lock when the owner is the home, and otherwise try-locks the
  owner and checks the owner again once it holds the lock. A busy owner or a stack still being left
  re-holds the record, without dropping it.

No CPU reads a queue or a CPU's state without holding that CPU's lock, or without having try-locked it
and revalidated the owner. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

What stays under the list lock and `lock_all`: the thread list, retirement grace, `unpinned_free`, the
exit-kick watch and the signal-timer groups, exactly as `rq.rs:23-27` lists for x86.

### 3.3 The owner index on ARM64

E9.1 needs an index from TID to (thread, owner) on ARM64. x86's `INDEX` has a fixed 4,096 slots. ARM64
has no such cap today: its wake index is chained (`wake_index.rs`). So E9.1 extends the ARM64 wake
record with the thread pointer and gives `find`, `owner`, `owned_thread` and `Pin` an ARM64
implementation over the chained index. That adds no thread cap to ARM64. E9.4 later replaces both
indexes with one growable registry (§6). `get_thread`'s linear scan leaves every dispatch path; it stays
only for whole-scheduler holders until E9.4. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

### 3.4 Stack custody (S3)

Neither architecture waits for a stack (ruling S8). They differ in what follows the skip. x86 registers
as a waiter, rechecks the mark and is kicked by the clear (§1.2). ARM64 registers nothing and relies on
a later selection. The shared selection calls `RqArch::stack_owner_elsewhere`. x86's implementation
keeps today's protocol, unchanged in every PR. ARM64's adopts the same protocol in E9.1, as follows.

**ARM64 registers, rechecks and is kicked durably (E9.1).** Today ARM64 skips a marked thread and relies
on a later selection, "at the latest the next tick" (`switch_save.rs:11-14`). Tickless idle (E9.6b)
removes that tick. So E9.1 gives `switch_save` a waiter set per marking CPU and this order:

- *Selector*: on finding `tid` marked by CPU m, set its own bit in m's waiter set, then load m's mark
  again. If the mark is clear or names another thread, it selects again at once rather than idling.
- *Leaver*: clear the mark, then swap m's waiter set to empty, and kick each waiter. A kick sets the
  waiter's resched-requested flag before its SGI. That flag is the ARM64 counterpart of x86's
  `RESCHED_REQUESTED`, which E9.1 adds. The idle masked recheck reads that flag, so a waiter
  that is entering WFI does not sleep through it.

The waiter's set-then-load and the leaver's clear-then-swap are sequentially consistent operations.
Either the selector's second load sees the clear, or the leaver's swap sees the selector's bit. That
holds for every interleaving (model in §10, E9.1). This is the order x86 already uses (§1.2).

### 3.5 The ARM64 switch transaction

- **Today:** lock `SCHEDULER` masked, decide, save the outgoing context, leak the guard, pivot to the
  scheduler stack, force-unlock there (§1.1).
- **Under E9.1:** take this CPU's run-queue lock masked (`rq::try_local`, below), decide, save the outgoing context,
  mark the outgoing thread in `switch_save`, forget the `Held` guard before the pivot (as the global
  guard is leaked today) in exchange for a `PivotToken`, and release the lock on the scheduler stack with
  a new `rq::release_after_pivot(token)`. That function publishes the summaries
  (`Held::publish`), unlocks, clears the `SCHEDULER_HELD` count and sends the deferred SGIs, in the same
  order `Held::drop` uses.
- **The outgoing thread.** It is queued on this CPU, or on its pinned home through `retain_affine`,
  while the mark is set. A peer that finds it skips it (S3). `aarch64_complete_exception_switch` clears
  the mark (`context_switch.rs:6093`), then takes the waiter set and kicks each waiter (§3.4).
- **Identity.** The CPU index used for the lock comes from the same masked read the pivot already checks
  (`CpuId::current_checked`, `context_switch.rs:6972`). A disagreement retracts the transaction as it does
  today.
- **The acquired lock travels with the transaction.** Today's rollback can release any CPU's view of the
  lock, because there is one global lock (`force_unlock_scheduler`). Under E9.1 the lock is per CPU. So
  forgetting the `Held` guard yields a `PivotToken { cpu }` naming the lock actually acquired. The
  token passes through `aarch64_inline_schedule_switch` to the trampoline in a callee-saved register.
  It is not kept in `INLINE_SCHEDULE_STATE[cpu]`, which the retraction empties and which a disagreeing
  identity would index differently. `release_after_pivot(token)` and the retraction fallback both
  release `token.cpu`'s lock. Neither re-reads the CPU identity to choose it. A token whose CPU differs
  from the fresh identity at release is recorded as a receipt, alongside the existing retraction
  counters.

- **Own lock busy.** A peer may hold this CPU's lock for a remote section (a wake, a steal or a
  placement), and its vCPU may be stopped there by the host. So dispatch takes its own lock with
  `rq::try_local`, not the waiting `rq::local` (`rq.rs:602-615`). When the try fails:
  - *interrupt-return dispatch* sets this CPU's `DISPATCH_OWED` flag, tries the lock once more, and
    returns to the interrupted thread if that also fails, with interrupts enabled as they were. Nothing
    has been dequeued, so the thread is still the CPU's current thread;
  - *the inline and exit switches* set the flag and retry with interrupts enabled between attempts. A
    thread that has not committed its park is still running (E2 §3.3), so it keeps its CPU and services
    interrupts while it retries;
  - every release of a run-queue lock by a CPU other than its owner reads the owner's `DISPATCH_OWED`
    after the unlock and kicks the owner if the flag is set. The kick is deferred past the unlock, like
    the other SGIs.

  The flag store and the retry on one side, and the unlock and the flag read on the other, are
  sequentially consistent. So either the retry finds the lock free or the releaser sees the flag. That
  holds for every interleaving (model in §10, E9.1). E9.1 moves ARM64 to `try_local` for all three entry
  points. x86 keeps `rq::local` in E9.1, which leaves x86 unchanged, and moves to `try_local` in E9.2a
  under the same kernel check.

The lock is now this CPU's alone, so a peer's dispatch, wake or steal is held up only on try-locks of
this CPU, and those do not wait (S2).

### 3.6 Lock order

From outermost: the list lock (whole-scheduler holders only), then run-queue locks in ascending CPU
order for `lock_all`, or one local lock plus try-locks of peers otherwise. No path waits for a second
run-queue lock while holding one; a second lock is only ever tried.

Against the rest of the kernel, the run-queue locks and the list lock take `SCHEDULER`'s place at level
1 of the documented order (`scheduler.rs:5-17`): none of them is acquired while PROCESS_MANAGER (level 2)
is held. That is why wakers that hold PROCESS_MANAGER publish to the durable inbox instead (E2 §3.3,
"Deferred"). The E2 park commit (E2 §3.3 step 3) takes the owner's run-queue lock; E2 says that is the
global `SCHEDULER` lock on ARM64 until E9 (E2 §3.3). E9.1 is that change. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

## 4. Ready structures and the fair class (E9.2)

### 4.1 Classes

Each CPU's run queue holds three structures, chosen strictly in this order:

| Class | Ranks | Structure | Order within |
|---|---|---|---|
| Fixed | 2-101: SCHED_FIFO and SCHED_RR priorities, the kernel's fatal-exit thread, and any thread boosted into that range by priority inheritance or a ceiling (`SchedPolicy::rank`) | A 128-bit occupancy bitmap and one intrusive FIFO list per rank | FIFO; SCHED_RR rotates at quantum end (POSIX XSH 2.8.4) |
| Fair | SCHED_OTHER and SCHED_BATCH (rank 1) | An intrusive pairing heap keyed by virtual time (§4.2) | Lowest virtual time first |
| Background | SCHED_IDLE (rank 0) | One intrusive FIFO list | FIFO |

- **Selection** is: the highest set bit of the bitmap, else the fair heap's minimum, else the background
  list's head, else the idle thread. Each step is O(1) apart from the heap's O(log n) amortised removal.
- **Membership.** Each thread embeds its queue link (list node or heap node) and a `queued_on: Option<cpu>`
  field written under the owner's lock. "Is it queued" is a field read, and removal is by handle. The
  VecDeque scans and rotations in §1.3 go.
- **No allocation.** Links live in the `Thread` box, whose address is stable. Nothing in enqueue, dequeue
  or selection allocates, so all of it is legal with interrupts masked. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

### 4.2 The fair class: virtual time by weight

Each fair thread has a weight from its process's nice value and a virtual time `vt`:

- **Weight.** `w(n) = round(1024 * 2^(-n/4))` for nice `n` in -20..=19, so each step is about 19% and
  nice 0 is 1024. This is our own choice; POSIX and nice(2) fix only the order. A table test pins the
  values.
- **Charging.** When a thread stops running, its `vt` grows by `elapsed_ns * 1024 / w`. Elapsed time is
  the counter-derived CPU time the code already charges (`Thread::charge_cpu`), so a late or coalesced
  interrupt does not distort it. That is stride scheduling's pass value with the stride proportional to
  1/w (Waldspurger and Weihl 1995).
- **Each CPU keeps `vt_floor`,** the minimum `vt` among its runnable fair threads and the running one,
  never decreasing. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Joining.** A thread that wakes after sleeping, or arrives by migration, has `vt` set to
  `max(vt, vt_floor - sleep_credit)`. That bounds the credit a long sleeper can bank (the stride paper's
  rule for clients that leave and rejoin). `sleep_credit` is one slice.
- **Migration** carries `vt - source.vt_floor` and re-adds it to the target's floor, so a thread neither
  gains nor loses its place by moving.
- **Slice.** A fair thread runs until its `vt` passes the next fair thread's by one slice, measured in
  weighted time, or it blocks. The slice is a constant in the PR, chosen by measurement (the series
  decides), and it is armed as a one-shot deadline (§8). Today's fixed 50 ms quantum on x86 and ten ticks
  on ARM64 go.
- **Wake preemption.** A woken fair thread preempts the running fair thread when its `vt` is lower by more
  than a wake granularity, so a thread that sleeps often gets the CPU soon after its wake. That is the
  latency half of BVT (Duda and Cheriton 1999), done with the `sleep_credit` bound and no separate warp
  parameter.
- **SCHED_BATCH** is fair with no wake preemption (sched(7)).

### 4.3 Placement and balancing

- **Wake placement** in this order, all from lock-free summaries: <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  1. the thread's previous CPU, if idle;
  2. the waking CPU, if the thread is not pinned elsewhere and the waker is about to block (a
     synchronous wake, such as a pipe write followed by a read);
  3. an idle CPU from the idle `CpuSet`, preferring one that shares a cache with the previous CPU (§7);
  4. the previous CPU.

  Each step is a word-at-a-time bitmap operation. `least_loaded`'s scan of every CPU goes. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Pull balancing.** A CPU about to idle, and each busy CPU at a balancing deadline, try-lock the most
  loaded peer from the summaries and take at most one fair thread, or one fixed-class thread that
  outranks what the puller would run. That is one try-lock and O(1) work per pass, with an exponential
  back-off on a peer that refuses. Topology from §7 orders the candidates: same cache first.
- **Preserved behaviour.** CPU affinity and pins, priority inheritance and ceilings, the wake-to-idle
  promotion (`promoted_wake`) and S3 stack custody are kept. The real-time class never waits behind a fair
  thread: a fixed-class wake on a CPU running a fair thread sets that CPU's `need_resched` and kicks it. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **The idle "no work anywhere" check** reads the `QUEUED_CPUS` summary, which becomes a `CpuSet` in E9.5.

## 5. Deferred work and the nesting word (E9.3, from brief B3)

Rewritten from B3 alone (§0). The names below are new. `softirqd.rs` and its accessors are replaced and
their callers updated in the same change, as E2.1 did for the waitqueue.

### 5.1 Classes and their handlers

```
enum DeferClass { Timer, NetRx, NetTx, Block, Sched }   // the five B3 names; nothing else
defer::register(class, handler: fn())                    // once per class, at init
defer::raise(class)                                      // any context except NMI (§5.3)
defer::pending(class) -> bool
defer::run_pending()                                     // the interrupt-exit and release entry
defer::init_cpu(cpu)                                     // the per-CPU worker thread
DeferHold                                                // guard: hold deferred work off on this CPU
```

- **The unused classes go.** Tasklet, HiTasklet, IrqPoll, Rcu and Hrtimer have callers today (§1.8). Each
  caller moves to one of the five classes or to a workqueue item, chosen by reading that caller. A
  tasklet-shaped caller that needs one function run soon after an interrupt becomes a `NetRx` or `Block`
  item when it belongs to that path, or a workqueue item otherwise. The PR body lists each move.
- **Pending state** is one word per CPU, one bit per class.

### 5.2 The nesting word

One 32-bit word per CPU, read in one load. Every field is a count or a flag, and "may this CPU be
preempted or may it run deferred work" are mask tests on the whole word. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

| Bits | Field | Meaning |
|---|---|---|
| 0-7 | `depth` | Preemption depth: spin locks held and explicit `CpuPin`s (E2 §4.1, §4.5) |
| 8-13 | `hold` | `DeferHold` nesting |
| 14 | `deferring` | This CPU is running deferred work now |
| 15 | reserved | Zero | <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
| 16-23 | `hardirq` | Hard-interrupt nesting |
| 24 | `nmi` | In an NMI (x86) or an equivalent non-maskable entry |
| 25-30 | reserved | Zero | <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
| 31 | `switching` | The exception return is mid-switch: no switch may start (today's in-return flag) |

- **Running against held off.** B3 asks that the word tell "running deferred work" apart from "deferred
  work held off". Here they are separate fields, a flag and a count, so neither is inferred from the
  other's parity.
- **`depth` is what E2 calls the depth** (E2 §3.2). `preempt::depth()` returns it. E2's `park::context()`
  reads `hardirq`, `deferring` and `nmi` through `cpu_nest::in_hard_irq()`, `in_deferred()` and
  `in_nmi()`. A context is preemptible when `depth`, `hold`, `hardirq`, `deferring`, `nmi` and
  `switching` are all zero. `hold` is in that list because the hold-off count is per CPU: a thread
  preempted and moved while holding would leave its hold on the old CPU. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Widths.** Eight bits of depth allow 255 nested spin locks. In testing builds an increment that would
  overflow any field writes a receipt and panics.
- **CPU-local updates.**
  - On x86 every update is one instruction with a GS-relative memory operand. An interrupt cannot divide
    an instruction (SDM Vol. 3A, "Interrupt and Exception Handling"), and migration needs an interrupt, so
    the update cannot land on another CPU. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - On ARM64 the address comes from `tpidr_el1`, and the update is a separate load and store. So every
    update made where interrupts may be enabled is bracketed by `msr daifset, #2` and a restore of the
    saved DAIF. With IRQs masked the thread cannot be preempted or migrated between reading `tpidr_el1`
    and storing the word (Arm ARM D1, PSTATE.I masking). Updates made in exception entry and exit, where
    interrupts are already masked, skip the bracket. This is the #1075 hazard (§1.8) closed by
    construction. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Interfaces kept for E2.** E2 reaches the count only through `preempt::depth()` and its guard types
  (E2 §3.2, §11). Those names keep their meaning across E9.3, whichever lands first.

### 5.3 Running deferred work (B3 requirements 1-5)

- **Raise** (requirement 1, 5). It sets the class's bit in this CPU's pending word, with a single atomic
  OR under the §5.2 locality rule. It takes no lock, allocates nothing and does not log. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - From hard-IRQ context, the work runs at that interrupt's exit.
  - From task context, the raise runs the work before it returns when the conditions of "interrupt
    exit" below hold for the caller: interrupts enabled, `hold` zero and no spin lock held beyond the
    baseline. Otherwise the work runs at the first point where they hold: the release of the hold, the
    lock or the interrupt mask. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - With work held off, it runs at the release.
  - An idle CPU's idle loop checks the pending word in its masked recheck before WFI or HLT, so a raise on
    an idle CPU cannot sleep through.
- **Interrupt exit** (requirement 2). When the outermost hard interrupt returns, `hardirq` becomes 0. The
  exit then runs pending work only if `deferring`, `hold` and `nmi` are clear, and the interrupted context
  held no spin lock beyond its baseline:
  - after E2.2 that is `depth == park_baseline` (E2 §3.2);
  - before E2.2 it is `depth == 0`, and work left pending is run at the release of the last depth level,
    as today (E2 §4.6).

  The run sets `deferring`, enables interrupts, runs, masks interrupts, clears `deferring`, and then
  continues to the existing reschedule check.
- **Budget** (requirement 3). A run loops over the pending classes in a fixed order (Timer, Block, NetRx,
  NetTx, Sched) until the pending word is clear, or 10 rounds pass, or 2 ms of counter time passes,
  whichever is first. The constants are the PR's to set from measurement; the bound is what the tests
  check.
  - Past the budget, the remainder stays pending and the CPU's worker thread is woken through the local
    wake path, and the interrupt returns. Received-packet processing works this way: a budget per pass,
    then a thread (Salim, Olsson and Kuznetsov 2001).
  - The worker runs the same `run_pending` loop with `deferring` set and yields between rounds. It is a
    fair-class kernel thread at weight 1024, so it shares the CPU with user threads and cannot starve them.
- **Work raised while a class runs** (requirement 1, second half). The run clears a class's bit before
  calling its handler, so a raise during the handler sets the bit again and the loop sees it.
- **Nested interrupt** (requirement 2). An interrupt taken during a run raises `hardirq`. Its exit finds
  `deferring` set and does not re-enter.
- **Hold-off** (requirement 4). `DeferHold` increments `hold` and its drop decrements it. The drop that
  takes `hold` to zero runs pending work if the other conditions of "interrupt exit" hold. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **NMI** (requirement 2). Never runs deferred work, and never raises it. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - *Why.* An NMI is taken even with interrupts masked (SDM Vol. 3A, "Nonmaskable Interrupt (NMI)").
    One that lands after the idle loop's final masked check returns to the HLT. A bit raised there
    would wait for whatever interrupt wakes the CPU next, and a tickless idle CPU may have none.
  - *How it is held.* The only NMI handler is the x86 shootdown handler
    (`tlb::shootdown_nmi_handler`, installed at `interrupts.rs:254`), and it raises no deferred work.
    E9.7 retires that NMI. In testing builds `defer::raise` checks `nmi` and writes a receipt and
    panics if it is set. A structure test, `tests/cpu_nest_structure.rs`, finds no path from the NMI
    handler to `defer::raise`. ARM64 takes no non-maskable entry today. One added later, on either
    architecture, must state its notification before it may produce deferred work.
- **Locks shared with deferred work.** A handler runs with interrupts enabled. So a lock a handler takes is
  taken elsewhere either as an `IrqSpinLock` (E2 §4.1), which masks interrupts and so blocks the exit
  path, or under a `DeferHold`. E2.6's validator records a class taken in deferred context, and a class
  taken with deferred work allowed, and raises a receipt for a class taken both ways (E2 §4.2).

## 6. The thread registry and retire queues (E9.4)

### 6.1 Registry

- **Shape.** One registry replaces x86's `INDEX` and ARM64's chained wake index. It is a two-level radix
  table indexed by TID: a directory of leaf pointers, each leaf holding 256 slots of today's `Slot` fields
  (thread, wake record, owner, cached root, pid, flags, group, policy). Lookups are lock-free acquire loads
  of the directory entry and the slot.
- **TIDs.** They come from a bitmap allocator that hands out the lowest free TID after a moving cursor,
  so a TID is not reused soon after it is freed. The directory covers the TID range in use and grows by
  publishing a larger directory, which copies the old one's leaf pointers. Readers that hold the old one
  stay valid, because leaves are never moved, and the old directory is retired through §6.2. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Admission.** Fork, clone and kthread spawn reserve a TID and, if its leaf is absent, allocate the
  leaf. They do this in their sleepable prologue, before anything is published. Failure returns EAGAIN
  (fork(2), clone(2)) to user callers and an error to kernel callers. Nothing panics: `insert`'s "no
  slot" panic goes. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Limits.** A per-user limit (RLIMIT_NPROC, getrlimit(2)), and a system limit derived from memory, which
  E1 reports. A fixed reserve of TIDs and leaf memory is kept for kernel threads, as `KERNEL_RESERVE` does
  today.

### 6.2 Retirement

- **Per-CPU retire queues.** A thread that terminates is pushed, by its owner under its owner's lock, onto
  that CPU's intrusive retire queue, tagged with the CPU's current grace epoch. Nothing scans the registry
  to find dead threads. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Grace.** A retired thread is freed once:
  - no CPU stands on its stack (S3) and two scheduling epochs have passed on its last CPU, the rule
    `note_scheduling_epoch` already applies (`scheduler.rs:2101`);
  - every CPU has passed a quiescent state since it was unlinked from the registry. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Quiescent states.** A CPU passes one when it enters the scheduler, or the idle loop, holding no `Pin`.
  This is quiescent-state-based reclamation (McKenney and Slingwine 1998; Hart et al. 2007). It replaces
  the single global `PINS` counter, which a long-lived reader on any CPU holds up for everyone. A `Pin`
  becomes a per-CPU nesting count, read only by that CPU's quiescent-state report.
- **Idle is an extended quiescent state.** A CPU in tickless idle enters neither the scheduler nor the
  idle loop again until something wakes it, so it would never report. Each CPU therefore keeps an idle
  counter. It increments the counter, making it odd, as the last step before WFI or HLT, holding no
  `Pin`. It increments it again, making it even, as the first step on any exit from idle: an interrupt
  entry from idle, before the handler can take a `Pin`, or the idle loop's return to the scheduler. Both
  increments are sequentially consistent. A grace period snapshots each CPU's counter after the unlink.
  A CPU whose snapshot is odd, or whose counter has changed since, has passed a quiescent state. So an
  idle CPU holds up no grace period, and a handler that leaves idle to take a `Pin` is waited for.
- **Bounded batches.** Each CPU frees at most a fixed batch per pass from its own queue, in its idle loop
  or its housekeeping deadline. An idle CPU frees its batch before it halts. While its own backlog is
  non-empty it arms the reclamation housekeeping deadline rather than halting with no deadline. A
  tickless idle CPU therefore never sits on a backlog of its own.
- **Backpressure.** When a CPU's backlog passes a threshold, new admissions on that CPU park on reclamation
  progress (E2 `park::wait`) until the backlog falls. Until E2.2 lands, they return EAGAIN as today.
- **What reclamation preserves.** Batched translation invalidation for kernel stacks, the per-thread
  all-CPU grace targets (`retirement_grace`, `scheduler.rs:3043`), and exit-kick observation are kept.
  Only their discovery changes, from scans to the queue.

## 7. CPU identity, per-CPU storage and interrupt routing (E9.5)

### 7.1 Descriptors from firmware

Each CPU gets a dense logical index into a table of descriptors built once at boot from firmware:

```
struct CpuDesc { hw_id: u64, package: u32, cluster: u32, core: u32, thread: u32, state: CpuState }
```

- **x86.** The MADT's Processor Local APIC entries (type 0) and Processor Local x2APIC entries (type 9),
  enabled or online-capable (ACPI 6.5, "Multiple APIC Description Table"). The topology fields come from
  CPUID leaf 0x1F, or 0x0B where 0x1F is absent (SDM Vol. 2A, CPUID). No clamp.
- **ARM64 under UEFI** (Parallels, VMware). The MADT's GIC CPU Interface (GICC) entries, whose MPIDR
  field gives each CPU's affinity (ACPI 6.5, "GIC CPU Interface (GICC) Structure"). The loader records the
  MPIDR list beside the count it already records.
- **ARM64 on QEMU.** The device tree's `/cpus` nodes and their `reg` properties (Devicetree
  Specification v0.4, "/cpus Node" and "/cpus/cpu* Node Properties").
- **Self-check.** Each secondary CPU reads its own MPIDR_EL1, or its own APIC ID, at entry (Arm ARM,
  MPIDR_EL1; SDM, "Local APIC ID"). It finds its descriptor by that value, not by the index it was
  started with. A CPU whose hardware ID matches no descriptor stays offline with a receipt.
- **Bring-up.** PSCI CPU_ON takes the target's MPIDR (PSCI, DEN0022, CPU_ON `target_cpu`), so ARM64
  bring-up passes `hw_id`. Today's "MPIDR = logical index" goes (`smp.rs:424-426`).

### 7.2 Per-CPU storage sized at boot

- **Allocation.** The boot CPU allocates one per-CPU area per described CPU from the boot frame pool (E1-1
  and E1-2, merged), before any secondary starts. The scheduler's per-CPU state, the run-queue lock, the
  summaries, the wake inbox, the deadline queue, the deferred-work word and the nesting word live there.
- **Indexing.** `[T; MAX_CPUS]` statics become `PerCpu<T>`: a pointer and a length, written once before
  secondaries start, then read-only. The 233 arrays move by subsystem, each PR safe alone, with a
  structure test whose allowlist shrinks to zero. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **NMI-reachable state.** The x86 TLB shootdown handler finds its CPU from its APIC ID with no GS base
  (`tlb.rs:44-45`). Its slots move to the per-CPU areas, and it finds them through a lookup table from APIC
  ID to area, built once at boot and never written again. The NMI path stays lock-free and
  allocation-free. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **CPU sets.** `u64` masks become `CpuSet`: a word array sized from the CPU count. The fixed-size uses,
  such as `QUEUED_CPUS`, the deferred-SGI bits and the stack-wait masks, get preallocated per-CPU scratch
  sets, so no path allocates one with interrupts masked.

### 7.3 Interrupt routing

- **GICv3 SGIs.** `send_sgi` takes a `CpuSet` and writes `ICC_SGI1R_EL1` once per affinity group
  (Aff3.Aff2.Aff1, and the range selector RS when Aff0 exceeds 15). It sets Aff3 at [55:48], Aff2 at
  [39:32], Aff1 at [23:16], RS at [47:44], the INTID at [27:24] and the target list at [15:0] (Arm GICv3/v4
  specification, IHI 0069, `ICC_SGI1R_EL1`). RS is used only when `ICC_CTLR_EL1.RSS` reports range
  selector support. Without it, Aff0 values above 15 are a receipt, and those CPUs stay offline.
- **Redistributors.** Each CPU finds its redistributor by walking the frames until `GICR_TYPER`'s affinity
  value matches its MPIDR affinity, stopping at the frame marked Last (IHI 0069, `GICR_TYPER`). The
  `cpu * 0x20000` offset goes.
- **GICv2.** GICv2's `GICD_SGIR` target list has eight bits, so a GICv2 system addresses at most eight CPU
  interfaces (IHI 0048). That is the hardware's limit, and E9.5 records it as such rather than as a
  kernel cap.
- **x86 IPIs.** x2APIC IPIs address 32-bit APIC IDs (SDM, "x2APIC"). xAPIC physical mode addresses IDs
  below 255, and higher IDs need x2APIC, which E9.5 enables when CPUID reports it.

### 7.4 Topology for placement

The descriptors give §4.3 its "shares a cache" test. On x86 that is the same package and core from CPUID.
On ARM64 it is the same Aff1 cluster, unless firmware's cache description says otherwise. This is a
placement preference only; nothing depends on it for correctness. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

## 8. One-shot deadlines and tickless idle (E9.6)

### 8.1 One deadline queue per CPU

Each CPU owns one deadline queue under its run-queue lock. It holds:

- sleepers' wake times: the scheduler timer heap and today's 64-entry fine heap, merged;
- the running thread's slice end (§4.2) or SCHED_RR quantum end;
- housekeeping deadlines: balancing (§4.3), reclamation (§6.2), the lockup and stall watchdogs, and the
  signal interval timers.

The queue is a per-CPU heap of intrusive nodes, one per sleeping thread (in the thread) and one per
housekeeping kind (in the per-CPU area), so arming never allocates and never refuses. "A full heap refuses
admission" (`deadlines.rs:2`) goes.

A thread's deadline is never posted to another CPU. The R17 inbox carries wake bits only
(`publish_wake(tid, bits, ..)`, `rq.rs:1263`). A deadline left there would stay with the old owner after a
migration. Instead:

- **Arm.** Only the owner arms a thread's deadline, under the owner's lock. A sleeper arms its own
  deadline in the park commit (E2 §3.3 step 3), which runs on its owner CPU. Housekeeping deadlines are
  per CPU and armed by that CPU.
- **Migrate.** The thread's deadline node is part of the scheduling state that `set_owner` moves while
  it holds both CPUs' locks (S1). It unlinks the node from the source queue and links it into the
  target's. If the node becomes the target's head, the target is kicked after the unlock to reprogram
  its compare. The source reprograms at its next queue change.
- **Cancel and expire.** Each park takes a new sleep sequence number, stored in the node. A wake cancels
  the node under the owner's lock. That happens either directly or in the drain of the held wake, which
  also runs under that lock. An expiry wakes the thread only if the node's sequence equals the thread's
  current sleep sequence and the thread is still parked. Otherwise it is a stale expiry, counted and
  dropped. Arm, cancel and expiry all run under the one owner lock, so they are totally ordered. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

### 8.2 Programming the hardware

After every change to the queue's head, and on every return to a thread, the CPU programs one compare
for the head: <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

- **ARM64.** It writes the absolute compare value (`CNTV_CVAL_EL0`, or `CNTP_CVAL_EL0` where the platform
  uses the physical timer) and enables it in `CNT*_CTL_EL0` (Arm ARM, "The Generic Timer in AArch64
  state"). No interval is rearmed. The VMware path that arms both timers
  (`timer_interrupt.rs:1279-1308`) becomes "arm the timer the platform delivers", chosen once at init.
- **ARM64 counter domain.** `CNTP_CVAL_EL0` compares against `CNTPCT_EL0`, and `CNTV_CVAL_EL0` against
  `CNTVCT_EL0`, which reads the physical count minus the hypervisor's `CNTVOFF_EL2` (Arm ARM, same
  chapter). EL1 cannot read that offset. So the deadline queue keeps its values in the domain of the
  timer chosen at init, and every "now" it computes from reads that timer's own counter. That is
  `CNTPCT_EL0` for the physical timer and `CNTVCT_EL0` for the virtual one. Today's arm paths already
  pair them this way (`timer_interrupt.rs:544-559`). A path that arms both timers computes each compare
  from its own counter. A virtual count is never written to the physical compare.
- **x86.** It uses TSC-deadline mode when CPUID.01H:ECX[24] reports it: the deadline goes into the
  IA32_TSC_DEADLINE MSR (SDM Vol. 3A, "APIC Timer", TSC-deadline mode). Otherwise it uses one-shot mode with
  an initial count computed from the calibrated rate. The invariant TSC (CPUID.80000007H:EDX[8]) is the
  clock. If it is absent, deadlines are bounded to the calibrated period and re-armed, as today's
  overlay does.
- **Ticks.** `get_ticks()` becomes a function of the clock, `(counter - base) / counter_per_tick`, on both
  architectures, computed when it is read. It no longer depends on a timer interrupt having run, so it
  stays correct on a CPU that takes none. The interrupt-side updates of `TICKS` (`time/timer.rs:119-142`)
  go. `MS_PER_TICK` keeps its meaning for the code that converts with it. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

### 8.3 Tickless idle

- **When idle arms nothing extra.** An idle CPU with an empty run queue, an empty wake inbox (no
  remainder left by a budgeted drain), no pending deferred work, no resched request and no owed kick arms
  only its queue's head, which may be "no deadline". It then halts (HLT) or waits for an interrupt (WFI)
  in the existing masked recheck. A drain that stops at `DRAIN_BUDGET` leaves the inbox non-empty and sets
  `need_resched` (`scheduler.rs:10458-10461`). The recheck therefore sends the CPU back through the
  scheduler for the next batch at once, not at a later deadline. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Waking an idle CPU.** Everything that can make an idle CPU runnable already notifies it, or is made to:
  - a wake posted to its inbox kicks it on the empty-to-non-empty edge (S5, R17), and a deadline moved
    to it by `set_owner` that becomes its head kicks it (§8.1);
  - a raise of deferred work on that CPU happens on that CPU (§5.3);
  - device interrupts are delivered to it directly.
- **Owed kicks (S5).** A reschedule notification that cannot be sent never relies on a periodic tick: <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - On x86 under x2APIC, a send is one ICR write with no busy state to refuse it (`apic.rs:355`,
    `:387`), so nothing is owed.
  - Under xAPIC, `try_send_reschedule` skips a busy ICR (`apic.rs:370-373`). A busy ICR means only that
    the previous IPI is still being delivered (SDM Vol. 3A, "Interrupt Command Register (ICR)", delivery
    status). Kicks are sent after the run-queue unlock. So under E9.6b a sender that finds the ICR
    busy records the target in its owed-kick set and finishes the send before it returns from the path
    that unlocked. It polls the delivery-status bit with interrupts enabled, outside every run-queue
    lock, for at most `KICK_POLL_BOUND`, then writes the ICR. Only a poll that reaches the bound leaves
    the kick owed. The sender then retries at its next unlock and interrupt exit, and on a
    housekeeping deadline `KICK_POLL_BOUND` away. So a kick lands at most about two bounds after the
    wake, not after 1 ms. The PR sets `KICK_POLL_BOUND` from the measured ICR busy time, and the check
    holds it below the smallest wake granularity and slice. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - `send_ipi`'s masked delivery wait (`wait_delivery`, `apic.rs:395`) is kept only for INIT and STARTUP during
    bring-up. Every other caller uses the non-waiting send plus the owed set. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Liveness without ticks.** A CPU that idles tickless publishes "idle, wakeable by IPI, next deadline
  T". Stall detection (`CPU_STALL_TICKS`) treats such a CPU as live until T plus the stall bound has
  passed, or until a kick to it has been owed longer than the bound. The x86 "returned from an interrupt
  this tick" stamp (`scheduler.rs:420-440`) becomes "returned, or idle and wakeable".
  Reclamation of a stalled CPU's queue (`reclaim_stalled`) keeps its meaning for CPUs that claim to run.
- **Busy CPUs.** A busy CPU's interrupts come from its slice end and its housekeeping deadlines, not from a
  fixed rate. E9 does not make a CPU with one runnable thread take no interrupts at all. Its housekeeping
  deadlines set its interrupt rate, and those deadlines are constants in the PR.

## 9. x86 PCIDs and IPI shootdowns (E9.7)

### 9.1 PCIDs

- **Enabling.** CR4.PCIDE is set when CPUID.01H:ECX[17] reports PCIDs. CR3 bits 11:0 must be zero when
  it is set (SDM Vol. 3A, "Process-Context Identifiers"). Kernel mappings stay global (CR4.PGE), so they
  survive every PCID switch. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **The cache.** Each CPU keeps a small cache of (address space, generation) to PCID: PCIDs 1 to 6,
  replaced least recently used. PCID 0 is the kernel root. A switch to an address space that hits the
  cache, at the generation the CPU last flushed it to, loads CR3 with bit 63 set, so its entries survive
  (SDM, "Mov to CR3" with PCIDE=1). A miss, or a stale generation, loads CR3 with bit 63 clear, which
  invalidates that PCID's entries.
- **Generations.** Each address space has a TLB generation. A page-table change that needs invalidation
  increments it, under the change's page-table lock (E2.8's `AddressSpace`).
- **INVPCID.** Single-address invalidation uses INVLPG for the current PCID. INVPCID type 0 or 1
  (CPUID.07H:EBX[10]) handles another PCID this CPU caches, where an immediate flush is needed.

### 9.2 Shootdowns as IPIs

- **Who may hold translations.** Each address space keeps a `CpuSet` of the CPUs that may hold its
  translations: those that have it loaded, plus those that cache its PCID at some generation. A CPU
  leaves the set when it evicts the PCID from its cache.
- **Ordering between activation and target selection.** Each address space A keeps a `loaded` `CpuSet`
  beside its generation. Both sides use locked read-modify-writes. Locked instructions have one total
  order, and loads are not reordered with them (SDM Vol. 3A, "Memory Ordering in P6 and More Recent
  Processor Families").
  - *Activation*, a CPU switching to A with interrupts masked:
    1. set its bit in `A.loaded` with a locked RMW;
    2. only then read `A.gen`;
    3. load CR3 with the no-flush bit only if its cached generation for A equals that value;
    4. record that value as its cached generation.

    Leaving A clears the bit after CR3 holds the next root.
  - *Change*, the initiator after editing A's page-table entries:
    1. increment `A.gen` with a locked RMW;
    2. only then read `A.loaded`;
    3. the CPUs in that read are the targets.

  If the initiator's read misses an activating CPU, that CPU's RMW came after the increment in the total
  order, so its generation read sees g and it flushes. If the activating CPU read the old generation,
  its RMW came first, so the initiator's read includes it and the IPI reaches it. An IPI handler flushes
  when its current address space is A at a cached generation below g, and acknowledges in every case.
- **Targets.** A shootdown for address space A at generation g goes only to the CPUs in that read of
  `A.loaded`. They invalidate the range or flush the PCID and acknowledge. A CPU that caches A but does
  not have it loaded needs no interrupt: at its next switch to A it finds its cached generation below g
  and loads CR3 without the no-flush bit. That is lazy TLB invalidation (Black et al. 1989), with batched
  ranges (Amit 2017).
- **Delivery.** A fixed-vector IPI. An NMI is no longer needed, because:
  - after E2.7, x86 syscalls run with interrupts enabled, so a CPU in a syscall takes the IPI;
  - an `IrqSpinLock` holder masks interrupts only for a bounded section;
  - E2.6's validator rejects waiting for a shootdown while holding an `IrqSpinLock`.
- **Initiator waits** with interrupts enabled. While it waits it services shootdown requests addressed to
  it, so two initiators cannot wait for each other.
- **Kernel-root flushes.** A flush that names no address space (`flush_page`, `flush_all`) goes to every
  online CPU. A tickless idle CPU on the kernel root takes it as an ordinary interrupt. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Lifetime (S9).** A frame or page-table page is freed only after every CPU in the set at the time of
  the change has acknowledged, or has flushed that address space's PCID since the change. Since E9 keeps
  idle CPUs on the kernel root (`load_kernel_root`, `tlb.rs:229`), a halted CPU still holds up no user
  shootdown. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

## 10. PR sequence

Every E9 PR follows E2's rules (E2 §8): <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

- its check is its own first commit and is red on the code before it, through a seam that the production
  path calls;
- a check that guards a mechanism the PR adds, and so cannot be red on the old tree, says so;
- tests run once, at the PR's final head;
- the other model family reviews it, including a clean-room provenance check (DEV-34(5));
- it runs claim-lint;
- it adds no warnings on x86-64 or ARM64.

There are no effort-suite runs until the program's final run (DEV-30). The coordinator records a QEMU
bbench series before and after each PR that §12 says could move a row (DEV-31(3)). x86-only effects get
their evidence at the coordinator's batched R240 flash (DEV-31(2)); there is no x86 QEMU.

| PR | Work | Needs | Needs E2? | Tier-1/2 files |
|---|---|---|---|---|
| **E9.1** | ARM64 per-CPU dispatch: `rq` made architecture-neutral; ARM64 dispatch, wakes and timers under the local run-queue lock; ARM64 owner index over the chained wake index | R17 (#3238, merged) | No | `interrupts/context_switch.rs` (Tier 2) only for moving x86 callers to the shared names; `per_cpu.rs` (Tier 2) not touched |
| E9.2a | Fixed and background classes: bitmap and FIFO lists, intrusive links, O(1) membership; x86 dispatch on `try_local` (§3.5) | E9.1 | No | — |
| E9.2b | Fair class by virtual time with nice weights; wake placement and pull balancing from summaries | E9.2a | No | — |
| E9.3 | Deferred work and the nesting word, clean-room from B3 | — (independent of E9.1) | No. Uses E2.2's baseline at the exit check when present (§5.3) | `per_cpu.rs` (Tier 2); `interrupts.rs` (Tier 2) if the x86 exit path calls the new runner |
| E9.4 | Growable registry; per-CPU retire queues; quiescent-state pins | E9.1 | Partly: the admission backpressure park needs E2.2; before it, EAGAIN as today | — |
| E9.5 | CPU descriptors from firmware; per-CPU areas sized at boot; `CpuSet`; GICv3 SGIs by affinity; redistributors by `GICR_TYPER`; x2APIC | E9.1 | No | `per_cpu.rs` (Tier 2); `gdt.rs` (Tier 2) for per-CPU TSS storage |
| E9.6a | One deadline queue per CPU; one-shot compare on both architectures; counter-derived ticks; slice ends as deadlines | E9.1, E9.2b | No | `interrupts/timer.rs` (Tier 1), one commit: its tick call and its 50 ms quantum countdown (`:32-62`) give way to the slice deadline |
| E9.6b | Tickless idle; owed kicks; liveness for tickless CPUs; xAPIC sends without masked waits | E9.6a, E9.5 (`CpuSet` for owed kicks) | No | `interrupts/timer.rs` (Tier 1), one commit: the idle CPU's per-tick reschedule request (`:15`) goes |
| E9.7 | x86 PCIDs; IPI shootdowns | E9.5 | **Yes: E2.6 (lock types and validator) and E2.7 (x86 syscalls with interrupts enabled)**. The address-space generation also uses E2.8's `AddressSpace` | `interrupts.rs` (Tier 2) for the vector |

E9.1 goes first and needs nothing unmerged. E9.3 can run in its own lane beside E9.1, because it touches
the nesting word and the deferred-work runner, not run queues. E9.2a and E9.4 can run beside each other
after E9.1. One lane owns each PR (DEV-30(3)). <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

**Protocol models.** Several checks below carry a small Python model of the rule they guard, run with
`python3 -m doctest docs/plans/e9-scheduler.md`. A model states the design's rule, not the implementation.
The implementation PR's own check binds that PR. Models that race two CPUs enumerate every interleaving
of their steps, keeping each CPU's program order. The steps are sequentially consistent, which §3.4 and
§9.2 require of the operations they name.

```pycon
>>> from itertools import combinations
>>> def interleavings(a, b):
...     n = len(a) + len(b)
...     for picks in combinations(range(n), len(a)):
...         ia, ib = iter(a), iter(b)
...         yield [next(ia) if i in picks else next(ib) for i in range(n)]
>>> len(list(interleavings(['a1', 'a2'], ['b1', 'b2'])))
6

```

### E9.1: ARM64 per-CPU dispatch

- **Scope.** §3. Commits in order:
  1. the checks below (red on the old tree);
  2. the `RqArch` seam: `rq.rs` and `timer_groups.rs` compile on both architectures, with x86 behaviour
     unchanged and ARM64 not yet calling them;
  3. the ARM64 owner index over the chained wake index (§3.3);
  4. ARM64 timer heaps per CPU;
  5. the switch transaction under `rq::local` with `release_after_pivot` (§3.5), one commit per entry
     point: interrupt return, inline switch, exit switch;
  6. ARM64 wakes, notices and placement through `rq::wake`, `wake_or_hold` and `notify_or_hold`, and
     pinned placement through `hold(.., WAKE_PLACE, home)` and `place_held`, with
     `deliver_pinned_wakes_for_this_cpu` and `pinned_wake_is_waiting_here` deleted (§3.2);
  7. `lock_for_context_switch` and `force_unlock_scheduler` deleted.
- **Tests, check-first.**
  - **Independent dispatch, a kernel test in the testing feature.** A test hook stops CPU 1 inside its
    scheduling transaction, after it has taken its dispatch lock and before the pivot, until a counter
    deadline 50 ms later. Meanwhile two kthreads pinned to CPU 2 hand a token back and forth through a
    pair of `Completion`s for 1,000 round trips, each a block and a wake. The check is that CPU 2
    completes them while CPU 1 is held. On the old tree CPU 2 cannot
    take `SCHEDULER` until CPU 1 releases, so it completes none in the window: red. The hook is a named
    fixed point, so the outcome is decided by the state, not by timing. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - **A peer stopped on this CPU's lock.** A second hook stops CPU 2 for 50 ms of counter time, with
    interrupts masked, as a paused vCPU appears. CPU 2 stops inside a remote section that has try-locked
    CPU 1's run-queue lock, or on the old tree inside a section that holds `SCHEDULER`. CPU 1 runs a pinned kthread that increments a counter. The check is that
    during the hold CPU 1 keeps taking its timer interrupts and its kthread's counter keeps advancing.
    Once CPU 2 releases, CPU 1 dispatches at the next kick. The first independence test holds only the
    stopped CPU's own lock, so it does not reach this case. Red on the old tree: CPU 1's next
    interrupt-return dispatch waits for `SCHEDULER` with interrupts masked. It is also red on §3.5 as
    first written, where `rq::local` waits for its own lock with interrupts masked (`rq.rs:602-615`).
    Green on §3.5's `try_local` with an owed dispatch. The second model guards the owed flag, a mechanism
    this PR adds, so it cannot be red on the old tree.

    ```pycon
    >>> def local_dispatch(own_lock_holder, me):
    ...     if own_lock_holder not in (None, me):
    ...         return 'keep the current thread, interrupts enabled, dispatch owed'
    ...     return 'dispatch'
    >>> local_dispatch(own_lock_holder=2, me=1)
    'keep the current thread, interrupts enabled, dispatch owed'
    >>> LOCAL = ['set_owed', 'retry_lock']
    >>> HOLDER = ['unlock', 'read_owed']
    >>> def owed_dispatch_lost(order):
    ...     held, owed, retried_ok, kicked = True, False, False, False
    ...     for step in order:
    ...         if step == 'set_owed': owed = True
    ...         if step == 'retry_lock': retried_ok = not held
    ...         if step == 'unlock': held = False
    ...         if step == 'read_owed': kicked = owed
    ...     return not retried_ok and not kicked
    >>> sum(owed_dispatch_lost(order) for order in interleavings(LOCAL, HOLDER))
    0

    ```
  - **The ownership protocol, host tests** in a new `tools/sched-host-tests` that `#[path]`-includes the
    architecture-neutral `rq.rs` with a test `RqArch`:
    - a wake that reads owner A, while the thread moves to B before A's lock is tried, revalidates, is
      held to B's inbox, and is applied once by B's drain;
    - a thread whose stack another CPU is leaving is never selected by a steal, and is selected after the
      mark clears; <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
    - a remote try-lock that fails never waits, and the wake is published; <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
    - `lock_all` then `refresh` leaves every queued and current thread owned by its CPU. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

    These are characterisation checks: green on today's x86 logic, which shows the harness reaches the
    production code, and green on the shared module. They are not red, because ARM64 has no such module
    on the old tree.
  - **Pinned placement reads only what it holds.** A host test holds a pinned wake for home CPU 0. The
    thread's owner is CPU 2, and CPU 1 pushes onto its own queue during CPU 0's pass. CPU 0's pass must
    read no queue or CPU state whose lock it does not hold or has not try-locked. It places the thread
    once, after it revalidates the owner under the owner's lock. Red on the design as first written,
    because §3.2 moved `deliver_pinned_wakes_for_this_cpu` under the home CPU's lock alone. Its
    `pinned_wake_is_waiting_here` reads every CPU's queue and `cpu_state` (`scheduler.rs:7316-7386`).
    Green on §3.2's R17 placement through `place_held`.

    ```pycon
    >>> def pinned_delivery_reads(home, cpus, owner):
    ...     tried = [] if owner == home else [(owner, 'try-locked, owner rechecked')]
    ...     return [(home, 'held')] + tried
    >>> [read for read in pinned_delivery_reads(0, [0, 1, 2, 3], owner=2) if read[1] == 'unlocked']
    []

    ```
  - **Custody registration against the clear.** A host model races a selecting CPU that finds a thread
    marked in `switch_save` against the CPU that clears the mark. In no interleaving may the selector
    pass the thread over and then go unkicked without seeing the mark clear. A tickless idle CPU would
    otherwise leave the thread queued with nobody to pick it. Red on §3.4 and §3.5 as first written:
    they skip, then register a waiter, with no recheck after the registration. Green on §3.4's
    register-then-recheck order.

    ```pycon
    >>> SELECTOR = ['see_mark', 'register_waiter', 'recheck_mark']
    >>> LEAVER = ['clear_mark', 'take_waiters']
    >>> def stranded(order):
    ...     mark, waiting, saw_set, kicked, saw_clear = True, False, None, False, False
    ...     for step in order:
    ...         if step == 'see_mark': saw_set = mark
    ...         if step == 'register_waiter': waiting = True
    ...         if step == 'recheck_mark': saw_clear = not mark
    ...         if step == 'clear_mark': mark = False
    ...         if step == 'take_waiters': kicked = waiting
    ...     return saw_set and not kicked and not saw_clear
    >>> sum(stranded(order) for order in interleavings(SELECTOR, LEAVER))
    0

    ```
  - **Release by the acquired identity.** A seam test acquires CPU 1's run-queue lock and forgets the
    guard before the pivot. It then makes `CpuId::current_checked` answer 2 at the pivot. Both the
    retraction fallback and `release_after_pivot` must release CPU 1's lock and leave CPU 2's untouched.
    Red on §3.5 as first written, which names the lock only by the CPU identity read at release. Green
    on §3.5's `PivotToken`.

    ```pycon
    >>> def released_lock(acquired_cpu, cpu_at_release):
    ...     token = acquired_cpu
    ...     return token
    >>> released_lock(acquired_cpu=1, cpu_at_release=2)
    1

    ```
  - **A structure test**, `tests/arm64_dispatch_lock_structure.rs`, that no ARM64 dispatch entry point
    (`check_need_resched_and_switch_arm64`, `schedule_from_kernel`, `schedule_terminated_from_exit`) reaches
    `lock_for_context_switch`, `force_unlock_scheduler` or `Scheduler::get_thread`. Red on the old tree.
  - The IRQ-shape census in `tests/teardown_structure.rs` is kept: each run-queue acquisition is masked.
- **Risk retired.** R18: one CPU's switch no longer serialises its peers or lengthens their masked time.
- **Rows.** §12: the compile rows, `process.fork-exec`, `memory.malloc-4t`. Neutral checks on
  `process.getpid`, `ssh.round-trip` and `memory.copy-bytes`.
- **Boot.** One ARM64 QEMU boot with the repo's standard boot test, running the independent-dispatch kernel
  test and the boot path. The stack pivot and the exception return are hardware effects the host tests
  cannot reach.

### E9.2a: fixed and background classes

- **Scope.** §4.1 for ranks 0 and 2 to 101, intrusive links, `queued_on`; fair threads stay in a FIFO list
  for this PR. x86 stack custody does not change (§3.4). x86 dispatch moves from `rq::local` to
  `rq::try_local` with an owed dispatch (§3.5), under E9.1's stopped-peer kernel check, which it reaches
  through the shared module; its x86 evidence comes at the flash.
- **Tests.**
  - Host tests of the queue structures: insert, remove by handle, highest-rank selection, FIFO order within
    a rank, SCHED_RR rotation at quantum end, and a boosted thread moving between ranks.
  - A seam test of selection over the production `rq` with a test scheduler. A queue of 64 fair threads
    and one rank-50 thread at the tail selects the rank-50 thread with one bitmap read. The test counts
    entries visited: 1 against 65 on the old `promote_best_of`. Red.
  - The x86 custody seam is a characterisation check, not a red one. Selection that finds a stack-marked
    thread skips it, records the waiter and rechecks, and the clear kicks the waiter. That is already
    today's behaviour, so the check is green on the old tree and green after the queue change. It shows
    that the new queues keep the protocol. ARM64's red custody check is E9.1's.

    Checked against today's x86 sequence. `wait_for_stacks_x86` registers in
    `LEAVING_STACK_WAITERS`, then rechecks `LEAVING_STACK` (`scheduler.rs:2615-2626`).
    `release_leaving_stack` clears the mark, then takes the waiters and kicks each one
    (`scheduler.rs:2224-2236`).

    ```pycon
    >>> X86_TODAY = ['see_mark', 'register_waiter', 'recheck_mark']
    >>> any(stranded(order) for order in interleavings(X86_TODAY, LEAVER))
    False

    ```
- **Rows.** `process.fork-exec`; compile rows. **Boot.** Not needed on ARM64: the structures are pure and
  host-tested, and dispatch is unchanged from E9.1. x86 at the flash.

### E9.2b: the fair class and placement

- **Scope.** §4.2-§4.3.
- **Tests.**
  - The weight table, pinned.
  - Host fixed sequences over the production fair queue:
    - two always-runnable threads at nice 0 and nice 5 get CPU time in the ratio `w(0)/w(5)` within one
      slice over 1,000 slices;
    - a sleeper that wakes after 10 seconds runs within one slice but gets no more than one slice of
      credit;
    - migration keeps a thread's lag.
  - Placement seams: a wake whose previous CPU is idle goes there; a synchronous wake stays on the waker;
    a balancing pass try-locks one peer and moves at most one thread.
  - The nice ratio check is red on the old tree, where nice gives no share (§1.3).
- **Rows.** The compile rows (cargo with four jobs), `process.fork-exec`, `memory.malloc-4t`.
- **Boot.** One ARM64 QEMU boot: preemption at slice ends is a timing effect.

### E9.3: deferred work and the nesting word (B3)

- **Who writes it.** A lane that reads B3, this section, the specifications B3 names, and the public
  signatures and callers in §1.8. It does not read the old bodies (§0).
- **Scope.** §5. The five classes, the nesting word, `DeferHold`, the runner and the worker thread. Every
  caller moves in the same PR: `in_softirq`, `in_serving_softirq` and `bh_disable` become
  `in_deferred`, `cpu_nest::deferring` and `DeferHold`. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Tests,** host tests of the runner over fixed sequences (B3's list), through a seam in which the runner
  takes the nesting word and the pending word as arguments:
  - raise during a run: the class runs again in the same run;
  - raise while held off, then release: the class runs at the release, once;
  - a storm past the budget: exactly the remainder is handed to the worker, and the interrupt path
    returns;
  - a nested interrupt during a run: no re-entry;
  - NMI: never runs; <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - a field overflow is caught in testing builds;
  - on ARM64, a model of the update with a migration point between the address read and the store:
    the masked bracket keeps the word on one CPU.
  - **NMI against the idle check.** A host model of the idle entry sequence places an NMI after each
    step. No placement may halt the CPU with deferred work pending. On x86 an NMI is taken with
    interrupts masked (SDM Vol. 3A, "Nonmaskable Interrupt (NMI)"). An NMI after the final masked
    check therefore returns straight to the halt. Red on §5.3 as first written: there a raise is
    allowed "any context", an NMI never runs the work, and nothing notifies the CPU afterwards. Green
    on §5.3's rule that an NMI never raises, which the testing-build receipt and the structure test
    below enforce in the implementation.

    ```pycon
    >>> IDLE = ['mask', 'check_pending', 'halt']
    >>> NMI_MAY_RAISE = False
    >>> def halts_with_work(nmi_after):
    ...     pending = False
    ...     for i, step in enumerate(IDLE):
    ...         if step == 'check_pending' and pending: return False
    ...         if step == 'halt': return pending
    ...         if i == nmi_after and NMI_MAY_RAISE: pending = True
    >>> [halts_with_work(i) for i in range(len(IDLE))]
    [False, False, False]

    ```

  The guest-side cases in `kernel/src/task/softirq_tests.rs` are kept and reworded to the new names (B3).
  Red results:
  - The storm case is red if today's runner has no budget hand-off. The rewriting lane cannot know that
    without reading the old body, so the check is written against the new seam and states that it guards
    the new mechanism.
  - The layout check is red on today's tree: a structure test that no file outside `cpu_nest` names a
    raw nesting constant. The same file checks that no path from the NMI handler reaches
    `defer::raise`. That half guards a rule and is green on today's tree, whose NMI handler raises
    nothing.
- **Rows.** `network.tcp-guest-to-host` and `network.tcp-host-to-guest` (NetRx budget and hand-off),
  `network.scp-*`. Neutral checks on `process.getpid` and `process.fork-exec`.
- **Boot.** One ARM64 QEMU boot: interrupt-exit behaviour is a hardware exception return.

### E9.4: registry and retire queues

- **Scope.** §6.
- **Tests.**
  - Host tests of the registry:
    - lookups during a directory grow see every published thread; <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
    - a TID is not reused before its grace;
    - admission fails cleanly, with nothing published, when a leaf allocation fails. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - A quiescent-state model: a retired thread is not freed while any CPU holds a pin taken before the
    unlink, and is freed after every CPU reports. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - **Grace with a tickless idle CPU.** The same model keeps CPU 3 in tickless idle from before the
    unlink to the end of the case, while CPUs 0 to 2 enter the scheduler. The grace period must
    complete. A second case has CPU 3 leave idle for an interrupt whose handler holds a pin, and there
    the grace must wait. Red on §6.2 as first written, which counts a quiescent state only at an entry
    to the scheduler or the idle loop. An idle CPU makes neither, so retired threads pile up until
    admission hits backpressure. Green on §6.2's idle counter: odd at the snapshot, or changed since,
    counts. The second case guards the even-on-exit step that the counter adds.

    ```pycon
    >>> def grace_completes(after_unlink):
    ...     passed = {'enter_scheduler', 'enter_idle', 'stay_idle'}
    ...     return all(passed & set(events) for events in after_unlink.values())
    >>> grace_completes({0: ['enter_scheduler'], 1: ['enter_scheduler'], 2: ['enter_scheduler'], 3: ['stay_idle']})
    True
    >>> grace_completes({0: ['enter_scheduler'], 3: ['irq_from_idle_holding_pin']})
    False

    ```
  - A churn fixture through the admission seam: 10,000 create-and-exit cycles with a reclamation batch
    of one per pass never fail admission and never panic. Red on the old tree: fork and clone get EAGAIN
    once 3,840 slots are indexed or reserved (`rq.rs:127-133`), and an insert that finds no slot panics
    (`rq.rs:270`). That is #1712's class. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->

    The red claim, checked against the old admission limit. `reserve` refuses once `TAKEN` reaches
    `SLOTS - KERNEL_RESERVE`, and `TAKEN` falls as threads are reclaimed (`rq.rs:116`, `:127-133`). So
    the old tree refuses only when more threads are live, or reserved, at once than that limit.

    ```pycon
    >>> OLD_LIMIT = 4096 - 256
    >>> CHURN_PEAK_LIVE = 2
    >>> CHURN_PEAK_LIVE > OLD_LIMIT
    True

    ```
  - A structure test that nothing outside the registry scans threads to find dead ones. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Rows.** `process.fork-exec` and the compile rows (cargo spawns many short processes).
- **Boot.** Not needed: the registry and reclamation are host-tested, and dispatch does not change.

### E9.5: CPU identity and routing

- **Scope.** §7.
- **Tests.**
  - Host tests of descriptor building from fixtures: a MADT with type 0 and type 9 entries and APIC IDs
    above 255; a MADT with GICC entries over two clusters; a device tree `/cpus` with 16 CPUs over two
    clusters.
  - `ICC_SGI1R_EL1` encoding for targets across clusters and above Aff0 15, with and without RSS.
  - Redistributor matching by `GICR_TYPER` affinity over a fixture of frames in an order that does not
    match the logical index.
  - All red on today's tree, which clamps to eight and encodes zero affinity. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - A structure test whose allowlist of `[_; MAX_CPUS]` arrays shrinks to zero across the migration PRs. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Rows.** None on the 4-vCPU rig; a neutral check on all rows. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Boot.** One ARM64 QEMU boot with the standard script (`-smp 4`), for bring-up by hardware ID. A boot
  with more than eight CPUs needs a VM shape the standard script does not use (§15, Q1).

### E9.6a: one-shot deadlines

- **Scope.** §8.1-§8.2.
- **Tests.**
  - Host tests of the deadline queue: arm, cancel, re-arm, earliest-first, no refusal at any count. A
    65th sleeper's deadline is kept, where today's fine heap refuses it (red).
  - A seam test that after every queue change the programmed compare equals the head. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - **Compare in the armed timer's counter domain.** A seam test with a nonzero virtual offset (a
    hypervisor's `CNTVOFF_EL2`) arms a deadline 24,000 counts ahead on the physical timer and on the
    virtual timer. Each must fire 24,000 counts later in its own counter's time. `CNTVCT_EL0` reads
    `CNTPCT_EL0` minus the offset, `CNTP_CVAL_EL0` compares against `CNTPCT_EL0`, and `CNTV_CVAL_EL0`
    against `CNTVCT_EL0` (Arm ARM, "The Generic Timer in AArch64 state"). Red on §8.2 as first written,
    which makes `CNTVCT_EL0` the clock for both compares. Green on §8.2's per-timer counter domain.

    ```pycon
    >>> CNTVOFF = 5_000_000
    >>> def counter(name, physical_now):
    ...     return physical_now if name == 'CNTPCT' else physical_now - CNTVOFF
    >>> def compare_for(timer, physical_now, delta):
    ...     return counter('CNTPCT' if timer == 'CNTP' else 'CNTVCT', physical_now) + delta
    >>> def fires_at(timer, cval):
    ...     return cval if timer == 'CNTP' else cval + CNTVOFF
    >>> fires_at('CNTP', compare_for('CNTP', 10_000_000, 24_000)) - 10_000_000
    24000
    >>> fires_at('CNTV', compare_for('CNTV', 10_000_000, 24_000)) - 10_000_000
    24000

    ```
  - A host test that `get_ticks()` advances with the counter when no interrupt arrives.
  - **Deadlines across migration.** A host test arms a sleeper's deadline while CPU A owns the thread,
    then moves the thread to CPU B before A runs again. The deadline must end up in B's queue only, and
    B's compare must cover it. A second case lets a deadline from the thread's earlier sleep expire
    during a later sleep, which must not wake the later sleep. Red on §8.1 as first written. That text
    posts a remote deadline to the owner's inbox, and the R17 inbox carries only wake bits
    (`publish_wake(tid, bits, ..)`, `rq.rs:1263`), so a migrated thread's deadline stays with A. Green
    on §8.1's owner-only arming, node migration under `set_owner`, and sleep sequence numbers.

    ```pycon
    >>> def deadline_queue_after(owner_at_post, owner_at_drain):
    ...     return owner_at_drain
    >>> deadline_queue_after(owner_at_post='A', owner_at_drain='B')
    'B'
    >>> def expiry_wakes(deadline_sleep, current_sleep):
    ...     return deadline_sleep == current_sleep
    >>> expiry_wakes(deadline_sleep=4, current_sleep=5)
    False

    ```
  - A kernel test in the testing feature: `nanosleep` of 3 ms on an otherwise idle CPU returns within the
    one-shot's resolution plus the wake path, and not at the next 1 ms or 5 ms boundary. The fixed
    point is the deadline itself.
- **Rows.** `ssh.round-trip`, `network.*`, `disk.fsync` (timeouts and wake latency).
- **Boot.** One ARM64 QEMU boot: the generic timer's compare is hardware. x86's TSC-deadline path gets its
  evidence at the flash (§15, Q2).

### E9.6b: tickless idle

- **Scope.** §8.3.
- **Tests.**
  - A host model of the idle protocol: a wake, a deadline post and a deferred-work raise at each point
    of the idle entry sequence always leave the CPU running or kicked, never halted with work pending. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
  - Owed kicks: a refused send is retried by the bounded poll, then at the sender's next unlock, and
    lands within two `KICK_POLL_BOUND`s.
  - **Kick and drain latency.** A host model sends a kick that finds the xAPIC ICR busy for 2 µs. The
    sender has no further unlock or interrupt exit for a second, and the woken thread's CPU has a
    deadline 100 µs away. The kick must land before that deadline. A second case leaves a drain
    remainder past the R17 budget (`DRAIN_BUDGET`, `wake_inbox.rs:8`) on a CPU about to idle tickless
    with no deadline armed. The remainder must be drained before the CPU halts. Red on §8.3 as first
    written, which retries an owed kick on a 1 ms housekeeping deadline. That text also lets an idle CPU
    halt with only its queue, deferred work and owed kicks checked. Green on §8.3's bounded poll and
    the idle recheck that reads the inbox. `KICK_POLL_BOUND_NS` here is a fixture value; the PR sets the
    real one from measurement.

    ```pycon
    >>> KICK_POLL_BOUND_NS = 20_000
    >>> def kick_latency_ns(icr_busy_ns, next_unlock_ns):
    ...     if icr_busy_ns <= KICK_POLL_BOUND_NS:
    ...         return icr_busy_ns
    ...     return min(next_unlock_ns, 2 * KICK_POLL_BOUND_NS)
    >>> kick_latency_ns(icr_busy_ns=2_000, next_unlock_ns=10**9) < 100_000
    True
    >>> IDLE_RECHECK_READS = {'run queue', 'wake inbox', 'deferred work', 'resched request', 'owed kicks'}
    >>> 'wake inbox' in IDLE_RECHECK_READS
    True

    ```
  - The liveness seam: a tickless idle CPU is not reported stalled before its deadline plus the bound.
    Red on today's tick-based rule.
  - A kernel test: an idle CPU takes no timer interrupts over 100 ms with no deadlines armed, and takes
    one at an armed deadline.
- **Rows.** All QEMU rows as a neutral-or-better check: idle vCPUs stop taking 1,000 interrupts a second,
  which the host scheduler may turn into time for the busy vCPUs. The series decides. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Boot.** One ARM64 QEMU boot: halting and waking are hardware effects.

### E9.7: x86 PCIDs and IPI shootdowns

- **Scope.** §9. x86 only.
- **Tests.**
  - Host tests of the PCID cache: hit, miss, LRU eviction, and a stale generation forcing a flush on the
    next switch.
  - A host model of the shootdown protocol over the address space's `CpuSet`: no frame is freed while a
    CPU in the set can hold a translation (S9). Two simultaneous initiators complete. A target in an
    `IrqSpinLock` section acknowledges after its section ends.
  - A structure test that no shootdown wait is reachable under an `IrqSpinLock`, which E2.6's validator
    also checks at run time.
  - The S9 model is red on a seam over today's protocol with PCIDs enabled and no generation check.
  - **Activation against target selection.** A host model races one CPU switching to address space A
    against an initiator that has edited A's page tables. No interleaving may leave the CPU both
    untargeted by the shootdown and running A on a cached PCID at the old generation. If it could, the
    CPU would keep translating to a frame the initiator then frees. Red on §9.2 as first written. That
    text targets "the CPUs that have A loaded now", and it gives no order between the activating CPU's
    generation read and its entry into A's loaded set. Green on §9.2's ordering rule: set `loaded`, then
    read the generation, against increment the generation, then read `loaded`.

    ```pycon
    >>> ACTIVATE = ['set_loaded', 'read_gen']
    >>> CHANGE = ['bump_gen', 'read_loaded']
    >>> def stale_and_untargeted(order):
    ...     gen, loaded, seen_gen, targeted = 0, False, None, None
    ...     for step in order:
    ...         if step == 'read_gen': seen_gen = gen
    ...         if step == 'set_loaded': loaded = True
    ...         if step == 'bump_gen': gen += 1
    ...         if step == 'read_loaded': targeted = loaded
    ...     return seen_gen == 0 and not targeted
    >>> sum(stale_and_untargeted(order) for order in interleavings(ACTIVATE, CHANGE))
    0

    ```
  - **The free waits only for CPUs it interrupted.** A host model unmaps a page of A while CPU 1 has A
    loaded and CPUs 2 and 3 sit idle on the kernel root, caching A's PCID. The frame must be freed once
    CPU 1 acknowledges. No wait may depend on CPUs 2 and 3 ever running A again. Red on §9.2 as first
    written. Its lifetime rule waits for every CPU in the set to acknowledge or flush, yet it sends no
    IPI to the CPUs that only cache the PCID.

    ```pycon
    >>> def free_waits_for(loaded_at_selection, cached_only):
    ...     return loaded_at_selection | cached_only
    >>> sorted(free_waits_for({1}, cached_only={2, 3}))
    [1]

    ```
- **Rows.** None on QEMU (ARM64). On the R240: `process.fork-exec`, the compile rows and `ssh.round-trip`. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Boot.** x86 only. Its evidence is the coordinator's batched kernel-only flash (DEV-31(2), DEV-32).

## 11. Test catalogue

| Check | Where | First red in |
|---|---|---|
| Independent dispatch under a held peer | testing-feature kernel test | E9.1 |
| Ownership protocol (revalidation, hold, custody, `refresh`) | `tools/sched-host-tests` | E9.1 (characterisation) |
| No ARM64 dispatch path takes the global lock or scans threads | `tests/arm64_dispatch_lock_structure.rs` | E9.1 |
| Queue structures; selection visits one entry; x86 custody (characterisation) | `tools/sched-host-tests` | E9.2a |
| Weights, nice shares, sleeper credit, lag on migration, placement | `tools/sched-host-tests` | E9.2b |
| Deferred-work runner sequences, nesting-word fields, ARM64 locality model | `tools/defer-host-tests`; `kernel/src/task/softirq_tests.rs` reworded | E9.3 |
| No raw nesting constants outside `cpu_nest`; no raise reachable from NMI | `tests/cpu_nest_structure.rs` | E9.3 |
| Registry growth, TID grace, admission failure, churn | `tools/sched-host-tests` | E9.4 |
| Quiescent-state reclamation | `tools/sched-host-tests` | E9.4 |
| MADT, GICC and device-tree descriptors; SGI encoding; redistributor match | `tools/cpu-topology-host-tests` | E9.5 |
| `[_; MAX_CPUS]` allowlist | `tests/per_cpu_storage_structure.rs` | E9.5 |
| Deadline queue, compare programming, counter ticks; 3 ms nanosleep | `tools/sched-host-tests`; testing-feature kernel test | E9.6a |
| Idle protocol, owed kicks, tickless liveness; no interrupts while idle | `tools/sched-host-tests`; testing-feature kernel test | E9.6b |
| PCID cache and shootdown model; no wait under `IrqSpinLock` | `tools/pagetable-host-tests`; structure test | E9.7 |

The existing structure tests that pin scheduler shape are kept, or updated only where they pin a name that
changes: `tests/teardown_structure.rs`, `tests/durable_irq_wakes_test.py`,
`tests/timer_wake_dispatch_structure.rs` and `tests/preempt_bracket_structure.rs`. E9 weakens no check in
them.

## 12. bbench rows (DEV-31)

The baseline is the ARM64 QEMU series `qemu-20261127-6d95fac4` (Breenix and Linux, same VM shape, 5
samples per row, 4 vCPUs). Ratio above 1 means slower than Linux (DEV-28(1)). The earlier
`qemu-20261127-ecf00bcc` series read `process.fork-exec` at 12.28x. Each PR's before-series is recorded on
the rig as it stands when the PR lands.

| Row | Breenix | Linux | Ratio | E9 PRs that may move it |
|---|---|---|---|---|
| `process.fork-exec` (ms per child) | 2.658 | 0.243 | 10.93 | E9.1, E9.2a, E9.2b, E9.4 |
| `compile.yes-dev-box` (s) | 4.578 | 0.132 | 34.57 | E9.1, E9.2b, E9.4 |
| `compile.yes-release` (s) | 16.497 | 1.088 | 15.16 | E9.1, E9.2b, E9.4 |
| `compile.cargo-noop` (s) | 0.403 | 0.00583 | 69.13 | E9.1, E9.4 |
| `compile.hello-object` (s) | 0.459 | 0.00912 | 50.31 | E9.1 |
| `memory.malloc-4t` (ms) | 228.7 | 9.00 | 25.41 | E9.1, E9.2b (contended futex wakes) |
| `network.tcp-guest-to-host` (MB/s) | 55.3 | 349.5 | 6.32 | E9.3, E9.6a |
| `network.tcp-host-to-guest` (MB/s) | 67.6 | 822.5 | 12.16 | E9.3, E9.6a |
| `ssh.round-trip` (s) | 0.0453 | 0.0590 | 0.77 | E9.6a (neutral check elsewhere) |
| `process.getpid` (ms per 1M) | 506.9 | 112.8 | 4.49 | Neutral check: E9.3 changes the nesting update on ARM64 |

Notes:

- **Predictions, not results.** Each prediction comes from reading the code. The compile rows are also
  dominated by file mmap (R6), the missing page cache (R5) and the allocator (R9), which E3, E1 and E7
  own. So E9's share of them is expected to be a fraction of the gap, and the series decides how much. A
  PR that predicts a move and does not show it says so in its body. That does not fail the PR, but it
  re-opens the attribution.
- **Attribution.** E9.1 adds run-queue lock wait and hold histograms, and E9.2 adds runnable-to-running
  delay, to `/proc/trace/counters`. They are always on and are written out only on failure (operator,
  2026-11-05). They are read with each series, so a moved row can be attributed to scheduling or not. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Compile rows `compile.bobj`, `bld`, `bmake`, `bcc`** are deferred in both series. They are read when
  they are measured. bcc's 254 s on the earlier ARM64 QEMU run is the lane's suggested place to look for
  R18 (lane findings §2).
- **Rows no QEMU run can show.** E9.5 (more than eight CPUs) and E9.7 (x86). E9.7 is read on the R240
  after a batched flash.

## 13. Interfaces with other work

- **E2 (sleepable contexts).**
  - E2's park commit takes the owner's run-queue lock. On ARM64 that is `SCHEDULER` until E9.1 and
    `rq::local` after it. Whichever of E2.2 and E9.1 lands second converts the other's call site.
  - E2.3 changes the ARM64 interrupt-exit decision in `context_switch.rs`, and E9.1 changes the
    transaction around it. Whichever lands second merges the other's change and keeps both properties.
  - E2 reaches the count through `preempt::depth()` and guards. E9.3 keeps those names (§5.2) and adds
    `cpu_nest::{in_hard_irq, in_deferred, in_nmi}`, which `park::context()` reads.
  - E2.6's validator gains the deferred-context class record (§5.3).
  - E9.7 needs E2.6 and E2.7.
- **E1 (kernel memory).** E9.5's per-CPU areas come from the boot frame pool (E1-1, E1-2, merged). E9.4's
  leaves use the fallible allocation E1 provides, and its system thread limit comes from E1's memory
  report.
- **E3 (page cache), E4 (block), E10 (network).** They raise deferred work through E9.3's five classes:
  E4 `Block`, E10 `NetRx` and `NetTx`. E10's per-socket and timer-wheel work arms deadlines through §8.1's
  queue rather than its own timer.
- **E8 (process and VM ownership).** E9.7's TLB generation lives in E2.8's `AddressSpace`, which E8 builds
  on.
- **R17 (#3238, merged).** The durable inbox every remote wake falls back to (S5). <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **#3243 (merged).** ARM64 ASIDs. E9 leaves them as they are.

## 14. Deviations log (DEV-39(2))

Builders record each design-detail deviation here, in the PR that makes it, with one sentence of reason.
Two resolutions were made while writing this design, where the code and the brief pulled different ways:

- **Stack custody follows x86's protocol on both architectures** (§3.4). The first draft read
  `wait_for_stacks_x86` as a wait. The code registers, rechecks and is kicked (`scheduler.rs:2615-2626`,
  `:2224-2236`), so x86 is unchanged and E9.1 brings ARM64 to the same protocol, because ARM64's
  wait-free skip relies on a tick that E9.6b removes.
- **The ARM64 owner index in E9.1 sits on the chained wake index, not on x86's 4,096-slot table** (§3.3),
  so that E9.1 adds no thread cap to ARM64. E9.4 replaces both.

## 15. Decisions for the coordinator

- **Q1. A boot with more than eight CPUs.** The standard ARM64 boot script runs `-smp 4`. E9.5's host
  tests settle descriptor parsing, SGI encoding and redistributor matching. A real boot of more than
  eight CPUs needs a different VM shape. The proposal is one ARM64 QEMU boot at `-smp 16` in the
  program's final run, where the coordinator sets the shapes. The alternative is to add it to E9.5.
- **Q2. x86 boot evidence** for E9.2a's move to `try_local`, E9.6a's TSC-deadline path, E9.6b's xAPIC change
  and all of E9.7. As in E2 §12 Q1, the proposal is that each merges on its host tests and review, and
  that its boot evidence is the coordinator's next batched kernel-only flash. <!-- claim-lint:ok: normative requirement of docs/plans/e9-scheduler.md under ruling DEV-41(2), not a measured result. -->
- **Q3. A timeout-overshoot bbench row.** No current row measures wake latency on its own; `ssh.round-trip`
  mixes several layers. The proposal is a `process.nanosleep-overshoot` row (median and p99 of 10,000
  1 ms sleeps), added by the bench lane before E9.6a so that E9.6 has a before-series.
- **Q4. E9.3's lane.** E9.3 needs no E9.1 and no E2. The proposal is that it runs in its own lane now,
  beside E9.1.

## 16. References

- Intel 64 and IA-32 Architectures Software Developer's Manual:
  - Vol. 2A, CPUID: leaf 01H (ECX[17] PCID, ECX[24] TSC-deadline), leaf 07H (EBX[10] INVPCID), leaves 0BH
    and 1FH (x2APIC topology), leaf 80000007H (EDX[8] invariant TSC);
  - Vol. 3A, "Paging": "Process-Context Identifiers (PCIDs)", "Invalidation of TLBs and Paging-Structure
    Caches", INVPCID, and MOV to CR3 with CR4.PCIDE=1;
  - Vol. 3A, "Advanced Programmable Interrupt Controller (APIC)": the APIC timer (one-shot, periodic and
    TSC-deadline modes), the ICR, x2APIC;
  - Vol. 3A, "Interrupt and Exception Handling": NMI handling and the atomicity of an instruction with
    respect to interrupts;
  - Vol. 3A, "Multiple-Processor Management": memory ordering.
- Arm Architecture Reference Manual for A-profile (DDI 0487):
  - D1, the AArch64 exception model: PSTATE.DAIF masking, exception entry and return;
  - "The Generic Timer in AArch64 state": CNTV/CNTP CVAL, CTL and the counters;
  - MPIDR_EL1 affinity fields;
  - B2, the AArch64 memory model.
- Arm Generic Interrupt Controller Architecture Specification, GICv3 and GICv4 (IHI 0069): `ICC_SGI1R_EL1`,
  `ICC_CTLR_EL1.RSS`, `GICR_TYPER`. GICv2 (IHI 0048): `GICD_SGIR`.
- Arm Power State Coordination Interface (DEN0022): CPU_ON.
- ACPI Specification 6.5, "Multiple APIC Description Table (MADT)": Processor Local APIC, Processor Local
  x2APIC and GIC CPU Interface (GICC) structures.
- Devicetree Specification v0.4: the `/cpus` node and CPU node properties.
- POSIX.1-2017 XSH 2.8.4, "Process Scheduling" (SCHED_FIFO, SCHED_RR, SCHED_OTHER); the Linux man pages
  sched(7), nice(2), getrlimit(2), fork(2), clone(2).
- C. A. Waldspurger and W. E. Weihl, "Stride Scheduling: Deterministic Proportional-Share Resource
  Management", MIT/LCS/TM-528, 1995.
- C. A. Waldspurger and W. E. Weihl, "Lottery Scheduling: Flexible Proportional-Share Resource
  Management", OSDI 1994.
- K. J. Duda and D. R. Cheriton, "Borrowed-Virtual-Time (BVT) Scheduling", SOSP 1999.
- J. Roberson, "ULE: A Modern Scheduler for FreeBSD", BSDCon 2003 (the paper; no source was read).
- M. L. Fredman, R. Sedgewick, D. D. Sleator and R. E. Tarjan, "The Pairing Heap: A New Form of
  Self-Adjusting Heap", Algorithmica 1(1), 1986.
- G. Varghese and A. Lauck, "Hashed and Hierarchical Timing Wheels", SOSP 1987.
- P. E. McKenney and J. D. Slingwine, "Read-Copy Update: Using Execution History to Solve Concurrency
  Problems", PDCS 1998.
- T. E. Hart, P. E. McKenney, A. Demke Brown and J. Walpole, "Performance of Memory Reclamation for
  Lockless Synchronization", JPDC 67(12), 2007.
- D. L. Black, R. F. Rashid, D. B. Golub, C. R. Hill and R. V. Baron, "Translation Lookaside Buffer
  Consistency: A Software Approach", ASPLOS 1989.
- N. Amit, "Optimizing the TLB Shootdown Algorithm with Page Access Tracking", USENIX ATC 2017.
- A. T. Clements, M. F. Kaashoek, N. Zeldovich, R. T. Morris and E. Kohler, "The Scalable Commutativity
  Rule", SOSP 2013.
- A. Baumann et al., "The Multikernel: A New OS Architecture for Scalable Multicore Systems", SOSP 2009.
- J. H. Salim, R. Olsson and A. Kuznetsov, "Beyond Softnet", USENIX ALS 2001.
- R. H. Arpaci-Dusseau and A. C. Arpaci-Dusseau, *Operating Systems: Three Easy Pieces*: "Scheduling:
  Proportional Share", "Multiprocessor Scheduling", "Locks" and "Condition Variables".
