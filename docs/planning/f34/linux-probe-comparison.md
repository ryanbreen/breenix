# F34 Linux-Probe CPU0 vTimer Comparison

Linux-probe collection succeeded using:

```bash
LINUX_PROBE_USER=wrb LINUX_PROBE_PASSWORD=root LINUX_PROBE_SUDO_PASSWORD=root \
  scripts/parallels/collect-linux-cpu0-traces.sh logs/f34/linux-probe-20260422-044652
```

The guest is Ubuntu 24.04.4 LTS running Linux `6.8.0-107-generic` on aarch64.
Artifacts are in `logs/f34/linux-probe-20260422-044652/` (ignored run
artifacts, not committed).

## Runtime Trace Evidence

`cpu0-events.report.txt` shows Linux repeatedly entering and exiting CPU0 idle:

- `power:cpu_idle state=1 cpu_id=0` marks idle entry.
- `power:cpu_idle state=4294967295 cpu_id=0` marks idle exit.
- CPU0 exits idle for IPIs, virtio interrupts, and `irq=10 name=arch_timer`.

`/proc/interrupts` confirms arch_timer progress during the trace window:

- CPU0 arch_timer before: `1335730`
- CPU0 arch_timer after: `1336019`
- Delta: `289`

No trace-cmd errors were reported in `cpu0-events.stderr.txt` or
`cpu0-fg.stderr.txt`.

The trace does not show a continuous 1ms local timer cadence while idle; Linux is
using tickless idle/nohz and programs the next event as needed. This is expected
from the function graph trace, which shows `tick_nohz_idle_stop_tick()` and
`hrtimer_start_range_ns()` in the idle path. The important comparison point is
that Linux's arch_timer interrupts continue progressing across idle transitions
on the same Parallels/HVF host.

## Linux Idle Pattern

Linux's ARM64 low-level idle path is:

- `/tmp/linux-v6.8/arch/arm64/kernel/idle.c:23` defines `cpu_do_idle`.
- `/tmp/linux-v6.8/arch/arm64/kernel/idle.c:29` executes `dsb(sy)`.
- `/tmp/linux-v6.8/arch/arm64/kernel/idle.c:30` executes `wfi()`.
- `/tmp/linux-v6.8/arch/arm64/kernel/idle.c:32` restores interrupt controller
  idle context.

The generic idle loop disables local IRQs before reaching the sleep instruction:

- `/tmp/linux-v6.8/kernel/sched/idle.c:258` loops while `!need_resched()`.
- `/tmp/linux-v6.8/kernel/sched/idle.c:291` calls `local_irq_disable()`.
- `/tmp/linux-v6.8/kernel/sched/idle.c:312` enters `cpuidle_idle_call()`.
- `/tmp/linux-v6.8/kernel/sched/idle.c:314` performs `arch_cpu_idle_exit()`.

This matches Breenix's broad `dsb sy; wfi` idle shape, but Linux also preserves
GIC priority-mask context around WFI via `arm_cpuidle_save_irq_context()` and
`arm_cpuidle_restore_irq_context()`.

## Linux Timer Programming Pattern

Linux timer CVAL/CTL writes are owned by the clockevent path:

- `/tmp/linux-v6.8/drivers/clocksource/arm_arch_timer.c:741` defines
  `set_next_event`.
- `/tmp/linux-v6.8/drivers/clocksource/arm_arch_timer.c:747-757` reads CTRL,
  enables the timer, unmasks IT_MASK, writes CVAL, then writes CTRL.
- `/tmp/linux-v6.8/drivers/clocksource/arm_arch_timer.c:665-674` masks the
  interrupt in the timer IRQ handler before invoking the event handler.

The Linux function-graph trace shows idle/nohz code programming hrtimers before
WFI and restarting them after idle:

- `tick_nohz_idle_stop_tick()`
- `hrtimer_start_range_ns()`
- `tick_program_event()`
- `arch_timer_handler_virt()`

This indicates Linux may program the local timer from idle/nohz clockevent
contexts, not only from the IRQ handler. However, that programming is centralized
through clockevents/hrtimer state, not ad hoc writes in arbitrary wait paths.

## Linux Context Switch Pattern

Linux kernel-thread context switches are ret-based:

- `/tmp/linux-v6.8/arch/arm64/kernel/entry.S:825` starts `cpu_switch_to`.
- `/tmp/linux-v6.8/arch/arm64/kernel/entry.S:829-835` saves callee-saved
  registers, SP, and LR.
- `/tmp/linux-v6.8/arch/arm64/kernel/entry.S:837-845` restores the next task's
  callee-saved registers, SP, and `sp_el0`.
- `/tmp/linux-v6.8/arch/arm64/kernel/entry.S:849` returns with `ret`.

Linux exception returns use `eret` through `kernel_exit`:

- `/tmp/linux-v6.8/arch/arm64/kernel/entry.S:412-413` writes `elr_el1` and
  `spsr_el1`.
- `/tmp/linux-v6.8/arch/arm64/kernel/entry.S:461` executes `eret`.

Therefore, ret-based dispatch to a kernel idle task is not itself a divergence
from Linux. A valid Breenix root-cause claim would need a reproduced Breenix
trace showing a specific timer-control, interrupt-mask, or context-publication
state that diverges from these Linux patterns.

## Comparison Conclusion

Linux-probe is stable enough to use as a reference: CPU0 idle transitions and
arch_timer IRQs continue on Parallels/HVF. But because Breenix did not reproduce
the supplied degradation signature during this run, the Linux data does not
identify a current Breenix divergence to fix.
