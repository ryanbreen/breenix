# F32i Fix Proposal

Date: 2026-04-19

Scope: design only. Do not implement in F32i. F32j should choose and implement the option supported by review.

## Evidence Summary

F32i reproduced the CPU0 hang twice with lock-free tracing:

- `wake_io_thread_locked` enqueued tid 10 on CPU0 and set the target need bit.
- Breenix sent SGI0 to CPU0.
- CPU0 never recorded an IAR-backed SGI receive.
- CPU0's `GICR_ISPENDR0` showed SGI0 pending, while `ICC_HPPIR1_EL1` returned 1023 and `ICC_RPR_EL1=0xff`.
- `ICC_PMR_EL1=0xf0`, so the evidence does not point to PMR masking as the immediate blocker.
- Run 5 also showed CPU0 re-entering WFI with `need_resched=1`.

Linux on the same Parallels ARM64 hypervisor woke CPU0 from idle in 20/20 trials, with 14-54 us wake-to-switch latency. The equivalent Linux task wake used a target wake list and `IPI_CALL_FUNC`, not a bare scheduler IPI.

## Option 1: Make Breenix Idle Loop Follow Linux's Sleep Gate

Design:

- Give `idle_loop_arm64` a real Linux-style sleep gate:
  - check `need_resched` and CPU0's runqueue before WFI;
  - do not enter WFI if work is already visible;
  - keep the check-to-WFI sequence ordered so there is no re-enable gap;
  - after WFI/interrupt return, re-check and schedule rather than blindly looping.
- Preserve the existing `dsb sy; wfi` sequence in the actual idle instruction path.
- If Breenix keeps a polling-idle fast path, add a Linux-style polling contract so remote wake can safely skip IPIs only when the idle CPU promises to reschedule.

Linux basis:

- `kernel/sched/idle.c:258-259`: idle loops only while `!need_resched()`, followed by `rmb()`.
- `kernel/sched/idle.c:261-289`: Linux documents the lost-wake rule: no interrupt re-enable gap between deciding to sleep and the sleeping instruction.
- `kernel/sched/idle.c:291-314`: Linux disables local IRQs before arch idle.
- `arch/arm64/kernel/idle.c:23-32`: ARM64 idle is `dsb(sy); wfi()`.
- `kernel/sched/idle.c:317-340`: Linux exits idle, orders polling clear, flushes pending SMP wake work, and schedules.

Safety:

- This touches idle behavior, not syscall or timer hot paths.
- It directly addresses run 5, where CPU0 re-entered WFI with `need_resched=1`.
- It does not by itself explain why SGI0 is pending but not acknowledged, so it should be paired with Option 2 or Option 3 unless review finds the idle-loop race sufficient.

Expected impact:

- Prevents CPU0 from sleeping when work is already visible.
- Reduces dependency on the next timer interrupt when the need bit is already set.
- Moves Breenix closer to Linux's idle semantics without adding arbitrary timers or fallbacks.

## Option 2: Adopt Linux-Style Remote Wake List + Function-Call IPI

Design:

- For cross-CPU task wake, stop treating the remote target runqueue enqueue as complete work that only needs a reschedule SGI.
- Queue the wake on a per-target CPU wake list, set a target `ttwu_pending` equivalent, and send a function-call IPI to the target CPU.
- In the target CPU's IPI handler, drain the wake list, activate the tasks locally, set target `need_resched`, and schedule on interrupt exit.
- Keep the SGI send ordering Linux uses: stores visible before SGI, then force the SGI system-register write to execute.

Linux basis:

- `kernel/sched/core.c:3930-3944`: Linux queues the remote wake on the target CPU's wake list and wakes that CPU via IPI.
- `kernel/sched/core.c:4018-4044`: TTWU chooses the wake-list path and returns without direct activation when it applies.
- `arch/arm64/kernel/smp.c:70-77`: `IPI_CALL_FUNC` is ARM64 IPI enum value 1.
- `arch/arm64/kernel/smp.c:887-901`: `IPI_CALL_FUNC` runs `generic_smp_call_function_interrupt()`.
- `drivers/irqchip/irq-gic-v3.c:1365-1387`: Linux executes `dsb(ishst)` before SGI send and `isb()` after writing `ICC_SGI1R_EL1`.
- Linux probe trace lines `15166-15173`: the same Parallels VM wakes CPU0 through `ttwu_queue_wakelist` + function-call IPI and switches to the target task.

Safety:

- This is the closest structural match to Linux's remote wake path.
- It avoids remote CPUs mutating CPU0's runnable state without CPU0 participating in the activation.
- It gives the IPI handler concrete work to drain, instead of relying on a pending reschedule SGI whose receive is currently missing.
- It requires careful ownership of per-CPU wake lists and should be implemented with lock-free or interrupt-safe primitives only.

Expected impact:

- Makes cross-CPU wake completion target-CPU-local, matching Linux's TTWU architecture.
- Provides a natural trace point and invariant: if wake-list pending is set, the function-call IPI must be received and drained.

## Option 3: Audit and Repair CPU0 GIC SGI Admission

Design:

- Investigate why CPU0 has `GICR_ISPENDR0.SGI0=1` while `ICC_HPPIR1_EL1=1023` and no IAR receive occurs.
- Specifically audit:
  - SGI0 group configuration on CPU0's redistributor;
  - SGI0 enable state on CPU0;
  - SGI0 priority relative to `ICC_PMR_EL1=0xf0`;
  - `ICC_IGRPEN1_EL1` / group enable state per CPU;
  - CPU0 redistributor routing and affinity used in `ICC_SGI1R_EL1`;
  - whether EOImode split priority-drop/deactivate state can strand SGIs;
  - whether SGI0 and the virtual timer are configured in the same group expected by the CPU interface.
- Keep `gic.rs` changes highly scoped and avoid logging in interrupt paths.

Linux basis:

- `arch/arm64/kernel/idle.c:19-21`: Linux explicitly warns that PMR/controller masking can prevent the core from waking.
- `drivers/irqchip/irq-gic-v3.c:1350-1387`: Linux sends SGIs through `ICC_SGI1R_EL1` with pre-send `dsb(ishst)` and post-send `isb()`.
- `arch/arm64/kernel/smp.c:948-957`: Linux maps GIC IRQs back to IPI numbers by subtracting `ipi_irq_base`, then dispatches through `do_handle_IPI()`.

Safety:

- This option addresses the strongest F32i anomaly: pending SGI0 in the redistributor but no CPU-interface delivery.
- `kernel/src/arch_impl/aarch64/gic.rs` was listed as read-only in the F32i prompt, but not Tier 1 in AGENTS. Any F32j change should still be treated as high scrutiny because it can break all interrupt delivery.
- Use GDB and in-memory tracing for validation; do not add serial output to interrupt paths.

Expected impact:

- If the root cause is group/enable/priority/routing mismatch, this is the fix that makes both SGI0 reschedule and any future IPI design reliable.
- It should also clarify whether the virtual timer's pending-but-stalled behavior shares the same CPU-interface admission problem.

## Option 4: Add `dsb sy` to `halt_with_interrupts()` Only After Proving That Path Is Active

Design:

- `Aarch64Cpu::halt_with_interrupts()` currently does `msr daifclr, #3; wfi` without the `dsb sy` used by Linux and by `idle_loop_arm64`.
- If F32j proves this helper is an active WFI path for the hang, align it with Linux's `dsb(sy); wfi()` pattern.

Linux basis:

- `arch/arm64/kernel/idle.c:29-30`: `dsb(sy); wfi()`.
- `arch/arm64/include/asm/barrier.h:29` defines `dsb(opt)`.

Safety:

- This is low risk if the path is active, but F32i evidence points at `idle_loop_arm64`, which already has `dsb sy`.
- Do not treat this as the primary fix without proving the helper participates in the failure.

Expected impact:

- Aligns the secondary WFI helper with Linux and avoids a known class of ordering bugs.
- It will not fix a GIC admission problem by itself.

## Recommended F32j Direction

Start with Option 3's GIC admission audit because the decisive F32i artifact is `GICR_ISPENDR0.SGI0=1` with `ICC_HPPIR1_EL1=1023` and no IAR receive. In parallel, implement Option 1's idle sleep gate because run 5 captured CPU0 entering WFI with `need_resched=1` and Linux never permits that through the generic idle loop.

Option 2 is the architectural target if review agrees Breenix should converge on Linux TTWU semantics rather than direct remote runqueue enqueue. It is larger than Option 1 but gives the cleanest parity with the Linux probe trace.

Do not introduce arbitrary timer fallbacks, SEV/WFE substitutions, or hypervisor-specific quirks. Linux validated that Parallels ARM64 wakes CPU0 correctly through GIC SGIs.
