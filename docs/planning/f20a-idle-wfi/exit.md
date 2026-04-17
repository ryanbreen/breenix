# F20a Secondary CPU Idle WFI Exit

## VERDICT

FAIL. Do not merge.

The only candidate that produced an acceptable single-run host CPU average was not stable. The required sweep failed on run 1 with the original sustained host CPU pin and no init completion. Per the factory rule, validation stopped after that failure.

## Code State

No kernel fix is left staged in the worktree. The attempted idle-loop changes were reverted after the failed sweep so the branch does not carry an unshippable scheduler/idle change.

## Build

ARM64 builds for the tested candidates produced no warning/error lines. The final tested candidate build was clean before the failed sweep.

## Candidate Summary

| Candidate | Result |
|---|---|
| Add `dsb sy; wfi` helper to raw secondary idle only | Booted once, but host CPU stayed high (`avg=595.0%`, `peak=697.4%`). |
| Use Linux-style idle helper in scheduler idle loop | Host CPU dropped to about `105%`, but init stalled and timer breadcrumbs stayed below threshold. |
| Change DAIF/ISB ordering around WFI | Same stall pattern, timer breadcrumbs `11-13`. |
| Remove scheduler-idle timer rearm | Restored original CPU pin (`avg=796.8%`) and stalled. |
| Add post-WFI ISB and compiler memory barrier | Same low-CPU stall (`avg=106.0%`, `timer_tick_count=11`). |
| Force GICD_CTLR VM-exit after idle rearm | Restored CPU pin (`avg=789.1%`) and stalled. |
| Secondary-only scheduler idle helper, CPU0 unchanged | One single run passed average CPU and boot completion, but first sweep run failed hard. |

## Validation Evidence

### Best Single Run, Not Stable

```text
exit_status=1
boot_script_completed=1
timer_tick_count=358
host_cpu_avg=128.5%
host_cpu_peak=176.4%
```

Host CPU samples:

```text
sample 1: 176.4%
sample 2: 116.7%
sample 3: 137.4%
sample 4: 105.5%
sample 5: 106.5%
```

### Required Sweep, Failed Run 1

```text
run=1
exit_status=1
boot_script_completed=0
timer_tick_count=11
host_cpu_avg=795.7%
host_cpu_peak=800.0%
```

Host CPU samples:

```text
sample 1: 795.7%
sample 2: 793.2%
sample 3: 798.6%
sample 4: 791.0%
sample 5: 800.0%
```

## Before/After Host CPU

| State | Host CPU |
|---|---:|
| Reported F20 baseline | ~798% |
| Best single candidate | avg 128.5%, peak 176.4% |
| Required sweep run 1 | avg 795.7%, peak 800.0% |

## Self-Audit

- No changes were made to `kernel/src/arch_impl/aarch64/timer_interrupt.rs`.
- No changes were made to `kernel/src/arch_impl/aarch64/exception.rs`.
- No changes were made to `kernel/src/arch_impl/aarch64/gic.rs`.
- No changes were made to `kernel/src/arch_impl/aarch64/syscall_entry.rs`.
- No polling fallback was introduced.
- No `spin_loop_hint` replacement was introduced.
- PRs #305, #308, #309, and #312 remain present on the branch through base `ba76e841`.

## Next Investigation

The key observation is that making scheduler idle WFI truly sleep either kills the per-CPU timer stream during init, or, when scoped to secondaries, is not stable across runs. The raw secondary bring-up loop can execute the helper and still receive early timer breadcrumbs, but the scheduler idle path plus Parallels/HVF timer rearm behavior remains racey. The next step should inspect scheduler-idle timer admission state at the moment CPUs transition from raw secondary idle into `idle_loop_arm64`, preferably with nonintrusive register capture rather than serial logging in interrupt/timer paths.
