# F32i Linux WFI / Idle / IPI Audit

Date: 2026-04-19

Linux tree: `/tmp/linux-v6.8`, commit `e8f897f4a`.

## Source Map

ARM64 idle:

- `/tmp/linux-v6.8/arch/arm64/kernel/idle.c:19-21` warns that if priority masking blocks the wake signal in the interrupt controller, the core will not wake.
- `/tmp/linux-v6.8/arch/arm64/kernel/idle.c:23-32` implements `cpu_do_idle()`: save cpuidle IRQ context, `dsb(sy)`, `wfi()`, restore context.
- `/tmp/linux-v6.8/arch/arm64/kernel/idle.c:38-44` implements `arch_cpu_idle()` by calling `cpu_do_idle()`.
- `/tmp/linux-v6.8/arch/arm64/kernel/process.c:72-75` only contains `arch_cpu_idle_dead()` in this tree. `arch_cpu_idle()` is not in `process.c` for Linux v6.8.

ARM64 barriers:

- `/tmp/linux-v6.8/arch/arm64/include/asm/barrier.h:23` defines `wfi()` as inline assembly with a memory clobber.
- `/tmp/linux-v6.8/arch/arm64/include/asm/barrier.h:29` defines `dsb(opt)`.
- `/tmp/linux-v6.8/arch/arm64/include/asm/barrier.h:56` maps `__mb()` to `dsb(sy)`.
- `/tmp/linux-v6.8/arch/arm64/include/asm/barrier.h:119-121` map SMP barriers to inner-shareable `dmb`.

Generic idle loop:

- `/tmp/linux-v6.8/kernel/sched/idle.c:255-256` sets the idle polling bit and enters tick-nohz idle.
- `/tmp/linux-v6.8/kernel/sched/idle.c:258-259` loops while `!need_resched()` and executes `rmb()` after the check.
- `/tmp/linux-v6.8/kernel/sched/idle.c:261-289` documents the lost-wake/timer race: interrupts must not be re-enabled between the decision to sleep and the sleeping instruction.
- `/tmp/linux-v6.8/kernel/sched/idle.c:291-314` disables local IRQs, enters arch idle, and calls the cpuidle idle path.
- `/tmp/linux-v6.8/kernel/sched/idle.c:317-333` handles the `need_resched()` exit path, including `preempt_set_need_resched()` and `smp_mb__after_atomic()` after clearing polling.
- `/tmp/linux-v6.8/kernel/sched/idle.c:339-340` drains SMP call wake work and calls `schedule_idle()`.

ARM64 IPI path:

- `/tmp/linux-v6.8/arch/arm64/kernel/smp.c:70-77` defines IPI numbers: `IPI_RESCHEDULE` is enum value 0 and `IPI_CALL_FUNC` is enum value 1.
- `/tmp/linux-v6.8/arch/arm64/kernel/smp.c:887-945` dispatches IPIs. `IPI_RESCHEDULE` calls `scheduler_ipi()`; `IPI_CALL_FUNC` calls `generic_smp_call_function_interrupt()`.
- `/tmp/linux-v6.8/arch/arm64/kernel/smp.c:948-957` maps an IRQ to an IPI number and sends it through `__ipi_send_mask`.
- `/tmp/linux-v6.8/arch/arm64/kernel/smp.c:1044-1047` implements `arch_smp_send_reschedule()` as `smp_cross_call(..., IPI_RESCHEDULE)`.
- `/tmp/linux-v6.8/arch/arm64/kernel/smp.c:1050-1057` uses the scheduler IPI as the CPU wake IPI for the parking protocol path.

GICv3 SGI send:

- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1350-1363` builds `ICC_SGI1R_EL1` and writes the SGI.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1365-1376` rejects non-SGI hwirqs and executes `dsb(ishst)` so normal-memory stores are visible before the IPI.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1378-1387` computes target lists, sends SGIs, then executes `isb()` so the ICC_SGI1R writes are executed.

Scheduler wake and resched:

- `/tmp/linux-v6.8/kernel/sched/core.c:900-910` atomically sets `TIF_NEED_RESCHED` and tests `TIF_POLLING_NRFLAG`, avoiding the polling-idle IPI race.
- `/tmp/linux-v6.8/kernel/sched/core.c:912-930` documents the polling-idle promise: if the idle task is polling, it will call `sched_ttwu_pending()` and reschedule soon.
- `/tmp/linux-v6.8/kernel/sched/core.c:1041-1062` implements `resched_curr()`: same-CPU wake sets need locally; remote wake sets need and sends `smp_send_reschedule(cpu)` only if the target is not polling.
- `/tmp/linux-v6.8/kernel/sched/core.c:1135-1159` applies the same non-polling idle check for remote timer enqueue.
- `/tmp/linux-v6.8/kernel/sched/core.c:3930-3944` implements the remote wake-list path: set `rq->ttwu_pending`, queue the task on the target CPU's wake list, and wake the CPU via `__smp_call_single_queue`.
- `/tmp/linux-v6.8/kernel/sched/core.c:4018-4044` chooses that wake-list path when `TTWU_QUEUE` is enabled and the wake condition qualifies.
- `/tmp/linux-v6.8/kernel/sched/core.c:6848-6862` notes a remote wake can arrive before its IPI and handles scheduling from user return in that case.
- `/tmp/linux-v6.8/kernel/sched/core.c:6871-6875` implements `schedule_preempt_disabled()`.

## Answers

A. What exact memory barrier is between "check need_resched" and "enter WFI"?

It is not a single standalone `smp_mb()` immediately between the check and WFI. Linux uses a structured idle protocol:

- `while (!need_resched())` is the sleep gate (`kernel/sched/idle.c:258`).
- `rmb()` follows the check (`kernel/sched/idle.c:259`).
- Linux then disables local IRQs before entering the idle operation (`kernel/sched/idle.c:291`).
- ARM64 `cpu_do_idle()` executes `dsb(sy); wfi()` (`arch/arm64/kernel/idle.c:29-30`).
- The idle polling state is ordered with atomic operations and `smp_mb__after_atomic()` when polling is cleared (`kernel/sched/idle.c:317-333`).

The important race-prevention property is documented in `kernel/sched/idle.c:261-289`: after Linux decides it may sleep, it must not re-enable interrupts before reaching the sleeping instruction. WFI is acceptable because it can be entered with interrupts disabled and still wakes on a pending interrupt.

B. What IPI does Linux send from `try_to_wake_up` to wake an idle CPU?

There are two relevant paths:

- A pure reschedule request uses `IPI_RESCHEDULE` (ARM64 enum value 0). `resched_curr()` calls `smp_send_reschedule(cpu)` when the remote CPU is non-polling (`kernel/sched/core.c:1041-1062`), and ARM64 maps that to `smp_cross_call(..., IPI_RESCHEDULE)` (`arch/arm64/kernel/smp.c:1044-1047`).
- A remote task wake commonly uses the TTWU wake-list path. Linux sets `rq->ttwu_pending`, queues the task on the target CPU's wake list, and calls `__smp_call_single_queue()` (`kernel/sched/core.c:3930-3944`). On ARM64 this is observed as `IPI_CALL_FUNC` (enum value 1), handled by `generic_smp_call_function_interrupt()` (`arch/arm64/kernel/smp.c:70-77`, `887-901`).

Both are backed by GIC SGIs. The GICv3 driver sends hwirq values below 16 through `ICC_SGI1R_EL1` (`drivers/irqchip/irq-gic-v3.c:1350-1387`). The audited send path does not assign a special scheduler priority at send time; priority comes from the interrupt controller configuration.

C. Does Linux rely on WFI exiting on any unmasked interrupt, or on SEV?

Linux relies on WFI and interrupt delivery, not SEV, for scheduler idle wake. ARM64 `cpu_do_idle()` is `dsb(sy); wfi()` (`arch/arm64/kernel/idle.c:29-30`). The generic idle comment explicitly names WFI as a sleep-until-pending-interrupt instruction that can be entered with interrupts disabled (`kernel/sched/idle.c:270-272`). `sev()` exists in `barrier.h:19`, but it is not part of this scheduler idle wake path.

D. How does Linux handle `need_resched` set without an IPI?

Linux has an explicit polling-idle contract:

- Idle sets its polling bit before entering the loop (`kernel/sched/idle.c:255`).
- Remote setters atomically set `TIF_NEED_RESCHED` and test the polling flag (`kernel/sched/core.c:900-910`).
- If the target is polling, Linux can skip the IPI because the idle task promises to call `sched_ttwu_pending()` and reschedule soon (`kernel/sched/core.c:912-930`).
- When `need_resched()` becomes true, the idle loop leaves the WFI loop, propagates preempt need, clears polling with ordering, flushes SMP call function work, and calls `schedule_idle()` (`kernel/sched/idle.c:317-340`).

So Linux does not rely only on an interrupt as the no-IPI path. It also has a polling-idle state machine that makes the no-IPI case safe.
