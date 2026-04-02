# ARM64 CPU0 Interrupt-Driven SMP Investigation

Date: 2026-04-02

Primary issue: `breenix-jhh`

## Problem Summary

On ARM64 under Parallels/HVF, the current Breenix branch contains CPU 0
special-case dispatch logic and timer "safety net" logic that assume CPU 0
cannot sustain normal interrupt-driven scheduling semantics. Linux on the same
hypervisor disproves that assumption.

This project exists to replace workaround-mode debugging with a falsifiable,
trace-driven investigation. Linux is the reference model. Breenix only earns a
fix when we can name the first semantic divergence from the Linux path and show
that the fix removes it.

## Non-Negotiable Investigation Contract

1. Linux on Parallels/HVF is the ground truth for platform behavior.
2. CPU 0 must sustain normal timer PPI delivery, SGI/IPI delivery, idle exit,
   EL0 execution, and scheduling without special routing.
3. No workaround-mode closures:
   - no permanent CPU 0 rerouting
   - no "re-arm more often" claims counted as fixes
   - no dispatch forks whose purpose is to avoid the real path
   - no "Parallels limitation" conclusion unless Linux shows the same failure
4. Every behavioral change must be justified by:
   - observed Linux trace
   - observed Breenix trace for the equivalent scenario
   - first semantic divergence
   - minimal fix for that divergence
   - rerun of the same comparison
5. One hypothesis at a time. One experimental variable at a time.

## Observable Invariant

The invariant for this project is:

> On Parallels/HVF, CPU 0 must behave like Linux CPU 0 for interrupt-driven SMP:
> timers continue firing, SGIs continue arriving, idle exits normally, EL0 can
> run on CPU 0, and scheduler-driven wakeup/reschedule behavior remains live.

Anything that violates this invariant is a Breenix bug until disproven.

## Current Branch Risks

The current tree already encodes workaround assumptions in the CPU 0 path:

- [context_switch.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/context_switch.rs#L1203):
  CPU 0 EL0 redirect guard
- [context_switch.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/context_switch.rs#L1696):
  ret-based resumed-thread dispatch used to avoid the "CPU 0 IRQ death" theory
- [context_switch.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/context_switch.rs#L1823):
  explicit pre-dispatch IRQ window and timer re-arm logic
- [context_switch.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/context_switch.rs#L1924):
  resume-side timer re-arm safety net

Those may be temporary investigative scaffolding, but they are not accepted as
architectural truth for this project.

## Canonical Linux Scenarios

These scenarios define the reference matrix. Breenix must be compared against
the same scenarios, not against hand-picked logs.

1. Idle -> timer interrupt -> scheduler activity -> idle
2. CPU1 -> wake CPU0 via IPI/SGI -> CPU0 schedules runnable work
3. CPU0 EL0 -> kernel entry -> EL0 return with timer liveness preserved
4. CPU0 blocked thread wakeup -> CPU0 reschedule path

## Current Linux Evidence

Live traces from `linux-probe` on Ubuntu 24.04.4 ARM64 (`6.8.0-106-generic`)
under Parallels/HVF already establish the following:

Verified host-side bundles:

- [latest-smoke](/Users/wrb/fun/code/breenix/logs/linux-probe-cpu0/latest-smoke)
- [20260402-073933](/Users/wrb/fun/code/breenix/logs/linux-probe-cpu0/20260402-073933)

### Event Trace Evidence

CPU 0 shows normal timer and IPI behavior:

- `irq_handler_entry: irq=10 name=arch_timer`
- `hrtimer_expire_entry: ... function=tick_nohz_highres_handler`
- `softirq_entry: vec=1 [action=TIMER]`
- `softirq_entry: vec=7 [action=SCHED]`
- `sched_switch: swapper/0 ==> kworker/...`
- `irq_handler_entry: irq=2 name=IPI`
- `ipi_entry: (Function call interrupts)`
- `sched_wakeup: ... CPU:000`

This directly disproves the claim that CPU 0 cannot take normal timer or SGI
delivery under Parallels/HVF.

### Function-Graph Evidence

The focused CPU 0 function-graph trace shows the expected Linux flow:

1. `do_idle`
2. `cpuidle_idle_call`
3. timer programming for idle/nohz
4. interrupt arrives
5. `irq_enter_rcu`
6. `gic_handle_irq`
7. `generic_handle_domain_irq`
8. `irq_exit_rcu`
9. `tick_nohz_idle_exit`
10. `flush_smp_call_function_queue`
11. `schedule_idle`

The verified collector bundle also shows direct timer handler execution inside
the GIC per-CPU IRQ path:

- `gic_handle_irq`
- `handle_percpu_devid_irq`
- `arch_timer_handler_virt`
- `gic_eoi_irq`

That is the behavioral reference sequence for Breenix CPU 0 idle-exit and
interrupt-driven scheduling semantics.

## Reproducible Linux Collector

Use the host-side collector script:

```bash
scripts/parallels/collect-linux-cpu0-traces.sh
```

By default it targets:

- host: `10.211.55.3`
- user: `wrb`
- password: `root`

Override via:

- `LINUX_PROBE_HOST`
- `LINUX_PROBE_USER`
- `LINUX_PROBE_PASSWORD`
- `LINUX_PROBE_SUDO_PASSWORD`

Artifacts are written to:

```bash
logs/linux-probe-cpu0/YYYYMMDD-HHMMSS/
```

Expected bundle contents:

- `env.txt`
- `interrupts.before.txt`
- `cpu0-events.report.txt`
- `cpu0-events.stderr.txt`
- `cpu0-fg.report.txt`
- `cpu0-fg.stderr.txt`
- `interrupts.after.txt`

## Current Breenix Evidence

The current branch also now has a reproducible host-side Parallels boot
capture path. The important point from the first live capture is that the
failure has moved past the original "CPU 0 timer died" theory.

Verified host-side bundle:

- [20260402-1454-live](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260402-1454-live)

### Serial Evidence

The reproducible Breenix Parallels bundle shows:

- successful EL0 entry:
  - `EL0_SYSCALL: First syscall from userspace (SPSR confirms EL0)`
- scheduler liveness warnings:
  - `[SCHED] queue_empty stuck_tid=11 count=2`
  - `[SCHED] queue_empty stuck_tid=10 count=3`
  - `[SCHED] queue_empty stuck_tid=11 count=4`
- an EL1 misresume signal before the stall:
  - `[PC_ALIGN] ELR=0xffff000040186801 FAR=0xffff000040186801 from_el0=0`
- soft lockup dump with no runnable queue entries:
  - `Ready queue length: 0`
  - `Ready queue: []`
  - `tid=10 state=C bis user`
  - `tid=11 state=X bis user`
  - `tid=13 state=X user`
- timer progress continuing after the lockup:
  - `Timer IRQ count: 1246`
  - later `"[timer] cpu0 ticks=5000"`

That is not consistent with "CPU 0 lost its timer interrupt stream." It is
consistent with CPU 0 continuing to take timer IRQs while scheduler/dispatch
liveness has already failed.

The misaligned EL1 PC is also now concrete. `0xffff000040186801` resolves
inside `kernel::drivers::ahci::handle_interrupt` with bit 0 set. That makes
"corrupted kernel resume PC in a blocked-in-syscall or kernel-mode resume path"
the strongest current divergence candidate.

## Reproducible Breenix Collector

Use the host-side collector script:

```bash
scripts/parallels/collect-breenix-cpu0-traces.sh
```

By default it targets:

- VM: `breenix-1774799978`
- wait time: `20` seconds

Override via:

- `BREENIX_PARALLELS_VM`
- `BREENIX_PARALLELS_SERIAL`
- `BREENIX_PARALLELS_WAIT_SECS`
- `BREENIX_PARALLELS_RESET_NVRAM`

Artifacts are written to:

```bash
logs/breenix-parallels-cpu0/YYYYMMDD-HHMMSS/
```

Expected bundle contents:

- `README.txt`
- `vm-info.before.txt`
- `start.output.txt`
- `vm-info.after.txt`
- `serial.log`
- `serial.signals.txt`
- `serial.tail.txt`
- `summary.txt`

## Experiment Protocol

Every Breenix session must follow this order:

1. Restate the current known divergence.
2. Pick exactly one hypothesis.
3. State the predicted observable.
4. Run one experiment that can falsify it.
5. Record:
   - build/commit
   - exact command
   - trace/log artifact path
   - result
   - disposition: confirmed, falsified, unresolved
6. Only then decide whether a code change is justified.

## Hypothesis Ledger

| ID | Hypothesis | Predicted Observable | Status |
|----|------------|----------------------|--------|
| H1 | Parallels/HVF fundamentally kills CPU 0 timer delivery during normal interrupt-driven SMP | Linux CPU 0 would also lose timer PPI or SGI-driven scheduling semantics | Falsified |
| H2 | The current failure is still plain CPU 0 timer death | After soft lockup, CPU 0 timer counters and timer-marked serial output would stop advancing | Falsified |
| H3 | The first meaningful Breenix divergence is scheduler/dispatch liveness after EL0 entry, not platform timer delivery itself | Breenix will continue taking timer IRQs after the stall while one or more threads become unreachable from the ready queue/current-thread/deferred-requeue sets | Supported |
| H4 | A blocked-in-syscall or kernel-mode resume path corrupts `elr_el1` by setting bit 0 before ret-based kernel dispatch | The failing thread will show an odd EL1 resume PC that resolves to a valid kernel symbol address plus `+1`, and the PC alignment fault will precede scheduler liveness collapse | Active |

Add new hypotheses only when they are falsifiable and tied to a predicted trace
difference.

## Issue Map

- `breenix-jhh`: ARM64 CPU0 interrupt-driven SMP investigation
- `breenix-jhh.1`: Capture canonical Linux CPU0 traces
- `breenix-jhh.2`: Capture equivalent Breenix CPU0 traces
- `breenix-jhh.3`: Isolate first semantic divergence
- `breenix-jhh.4`: Implement minimal architectural fix
- `breenix-jhh.5`: Remove workarounds and add regression verification

Dependencies:

- `breenix-jhh.3` blocked by `breenix-jhh.1` and `breenix-jhh.2`
- `breenix-jhh.4` blocked by `breenix-jhh.3`
- `breenix-jhh.5` blocked by `breenix-jhh.4`

## Exit Criteria

This investigation is complete only when all of the following are true:

1. Breenix CPU 0 traces match Linux semantically for timer, IPI, idle exit,
   and scheduling behavior.
2. CPU 0 runs EL0 without special routing guards.
3. CPU 0 workaround logic is removed.
4. Verification passes cleanly with the standard project boot path.
5. A repeatable regression check exists so the project cannot silently drift
   back into workaround mode.

## Immediate Next Step

Capture the equivalent Breenix CPU 0 trace set for the same Linux scenarios and
identify the first semantic divergence, not the tenth symptom.
