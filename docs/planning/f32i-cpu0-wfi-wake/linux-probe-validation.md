# F32i Linux Probe Validation

Date: 2026-04-19

Probe VM: `linux-probe` at `10.211.55.3`, started with `prlctl start linux-probe`.

Kernel:

```text
Linux probe 6.8.0-107-generic #107-Ubuntu SMP PREEMPT_DYNAMIC Fri Mar 13 19:42:33 UTC 2026 aarch64 aarch64 aarch64 GNU/Linux
```

Artifacts:

- Raw trace: `.factory-runs/f32i-cpu0-wfi-wake-20260419/linux-probe-validation/f32i_probe/trace.txt`
- Target PIDs: `.factory-runs/f32i-cpu0-wfi-wake-20260419/linux-probe-validation/f32i_probe/targets.txt`
- Latency CSV: `.factory-runs/f32i-cpu0-wfi-wake-20260419/linux-probe-validation/latency.csv`
- Interrupt snapshots: `.factory-runs/f32i-cpu0-wfi-wake-20260419/linux-probe-validation/f32i_probe/interrupts.before` and `.factory-runs/f32i-cpu0-wfi-wake-20260419/linux-probe-validation/f32i_probe/interrupts.after`

## Test

The test enabled ftrace events:

- `sched:sched_waking`
- `sched:sched_wakeup`
- `sched:sched_switch`
- `sched:sched_migrate_task`
- `power:cpu_idle`
- `ipi:ipi_raise`
- `ipi:ipi_send_cpu`
- `ipi:ipi_send_cpumask`
- `ipi:ipi_entry`
- `ipi:ipi_exit`

For each of 20 iterations:

1. Start a Python process pinned through `taskset -c 0`.
2. Install a `SIGUSR1` handler and block in `signal.pause()`.
3. From CPU1, send `SIGUSR1` with `taskset -c 1 kill -USR1 <pid>`.
4. Capture the CPU0 idle exit, IPI entry, scheduler wakeup, and switch to the target task.

## Representative Trace

Iteration 20 targeted pid 5877. Relevant raw trace lines:

- `trace.txt:15166`: CPU1 `sched_waking` for `python3 pid=5877 target_cpu=000` at `161.576165`.
- `trace.txt:15167`: CPU1 sends IPI to CPU0 from `ttwu_queue_wakelist+0x19c/0x248`.
- `trace.txt:15168`: CPU1 raises `target_mask=...0001 (Function call interrupts)`.
- `trace.txt:15169`: CPU0 exits idle (`cpu_idle: state=4294967295 cpu_id=0`) at `161.576191`.
- `trace.txt:15170`: CPU0 enters the IPI handler for `Function call interrupts`.
- `trace.txt:15171`: CPU0 performs `sched_wakeup` for pid 5877.
- `trace.txt:15172`: CPU0 exits the IPI handler.
- `trace.txt:15173`: CPU0 switches from `swapper/0` to `python3 pid=5877`.

For that iteration, wake-to-switch latency was 54 us.

## Latency Distribution

All 20 trials woke CPU0 and scheduled the target task.

From `latency.csv`:

- Count: 20
- Min wake-to-switch: 14 us
- Median wake-to-switch: 31 us
- p95 wake-to-switch: 43 us
- Max wake-to-switch: 54 us

Wake-to-IPI-send was 0-2 us. Wake-to-CPU0-idle-exit was 6-26 us.

## IPI Type

The signal wake path used Linux's TTWU wake-list machinery, not a bare reschedule IPI:

```text
ipi_send_cpu: cpu=0 callsite=ttwu_queue_wakelist+0x19c/0x248 callback=generic_smp_call_function_single_interrupt+0x0/0x48
ipi_raise: target_mask=00000000,00000001 (Function call interrupts)
```

That matches the Linux source audit:

- `kernel/sched/core.c:3930-3944` queues the remote wake on the target CPU and calls `__smp_call_single_queue()`.
- `arch/arm64/kernel/smp.c:70-77` defines `IPI_CALL_FUNC` as enum value 1.
- `arch/arm64/kernel/smp.c:887-901` handles `IPI_CALL_FUNC` by running `generic_smp_call_function_interrupt()`.
- `drivers/irqchip/irq-gic-v3.c:1350-1387` delivers ARM64 IPIs as GIC SGIs.

The interrupt snapshot also moved in the expected direction. During the 20-iteration test, CPU0's function-call interrupt count rose from 4324 to 4421, and its reschedule interrupt count rose from 1690 to 1714.

## Validation Result

Linux wakes CPU0 from idle reliably on the same Parallels ARM64 hypervisor. The measured behavior is:

1. Cross-CPU wake queues target work.
2. Linux sends a GIC-backed SGI to CPU0.
3. CPU0 exits idle/WFI.
4. CPU0 enters the IPI handler.
5. The wakeup is completed on CPU0.
6. CPU0 switches from `swapper/0` to the target task.

This rules out an unvalidated "Parallels cannot wake CPU0 from WFI" hypothesis. The current F32i evidence should be treated as a Breenix structural bug.

The structural difference observed in the equivalent Linux path is that Linux's remote task wake uses a target-CPU wake list plus function-call IPI, so the target CPU drains and activates the task inside the IPI path. Breenix currently enqueues directly to CPU0's runqueue and sends SGI0 as a reschedule signal; the F32i trace then shows SGI0 pending in CPU0's redistributor but never acknowledged by CPU0.
