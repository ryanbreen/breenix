# F32o Performance Diagnosis

Date: 2026-04-20  
Base: `6fbd329d` (`main`, F32k merged)  
Run: Parallels VM `breenix-1776681379`, `./run.sh --parallels --test 65`

## Summary

The 800% host CPU runaway is caused by the ARM64 idle loop repeatedly bypassing
WFI to call `schedule_from_kernel()`.

The trigger is not a persistent ISR wake-buffer entry. The idle gate bypass is
almost entirely from `need_resched`:

- `F32O_IDLE_SCHED_BYPASS`: 13,632,308
- `F32O_IDLE_NEED_RESCHED`: 13,588,154
- `F32O_IDLE_PENDING_ISR`: 91,441
- `F32O_IDLE_BOTH`: 47,287

BWM, bounce, bsshd, telnetd, and init are not spinning at user level. BWM calls
`compositor_wait()` normally after the initial redraw.

## Host Confirmation

During the Parallels run, the host process was sampled at:

```text
prl_vm_app --vm-name breenix-1776681379  819.9% CPU
```

That matches the user-visible 800% host CPU symptom.

## Per-Process And Per-Thread CPU

Temporary `/proc` sampling ran after boot for 50 one-second samples. Successful
TID snapshots showed tiny cumulative user thread CPU counts:

| Process | PID | TID | State in last successful TID snapshot | Cumulative ticks |
| --- | ---: | ---: | --- | ---: |
| init | 1 | 10 | BlockedOnChildExit | 85 |
| bwm | 2 | 11 | Running | 92 |
| telnetd | 3 | 12 | Blocked | 0 |
| bsshd | 4 | 13 | Blocked | 1 |
| bounce | 5 | 14 | BlockedOnIO | 7 |
| f32o_perfdiag | 6 | 15 | Running | 6 |

The process-status samples had the same maxima: `bwm=92`, `init=85`,
`bounce=7`, `f32o_perfdiag=6`, `bsshd=1`, `telnetd=0`.

Conclusion: no ordinary userspace process accounts for the host CPU burn.

## Per-CPU Tick Breakdown

Last sampled `/proc/stat` values:

| CPU | Timer ticks | Idle ticks | Non-idle ticks | Non-idle share |
| ---: | ---: | ---: | ---: | ---: |
| 0 | 323 | 316 | 7 | 2.2% |
| 1 | 20 | 18 | 2 | 10.0% |
| 2 | 7,563 | 758 | 6,805 | 90.0% |
| 3 | 6,929 | 461 | 6,468 | 93.3% |
| 4 | 6,744 | 265 | 6,479 | 96.1% |
| 5 | 6,957 | 539 | 6,418 | 92.3% |
| 6 | 10,096 | 3,922 | 6,174 | 61.2% |
| 7 | 6,889 | 1,112 | 5,777 | 83.9% |

This says CPUs 2-7 are spending most timer ticks marked non-idle, but the
per-thread/process accounting does not show a userspace consumer. That points
to scheduler/idle-loop activity rather than a hot process.

## BWM Loop State

Temporary BWM summaries emitted once per second:

- 49 BWM summary intervals.
- Total interval iterations: 13,171 over 49.163 seconds.
- Effective BWM loop rate: about 268 iterations/second.
- `compositor_wait()` calls: 13,170.
- Wait skips: 1 total, only the initial local-redraw path.
- Last interval: `iter_interval=288`, `wait_calls_interval=288`,
  `wait_skips_interval=0`, `flags_before=0/0/0`, `skipped=0`, `ready=1`.

Conclusion: candidate 1 is ruled out. BWM is not stuck with `full_redraw`,
`content_dirty`, or `windows_dirty` permanently true, and it does not skip the
kernel wait in steady state.

## Idle Gate Behavior

Last sampled trace counters:

| Counter | Total | Interpretation |
| --- | ---: | --- |
| `F32O_IDLE_WFI_ENTER` | 77,083,675 | Idle loop entered WFI path extremely often. |
| `F32O_IDLE_SCHED_BYPASS` | 13,632,308 | Idle loop bypassed WFI and called `schedule_from_kernel()`. |
| `F32O_IDLE_NEED_RESCHED` | 13,588,154 | Nearly every bypass was due to `need_resched`. |
| `F32O_IDLE_PENDING_ISR` | 91,441 | ISR wake-buffer condition was present but two orders smaller. |
| `F32O_IDLE_BOTH` | 47,287 | Both conditions together were rare relative to bypass volume. |

Per-buffer pending observations:

| ISR wake buffer | Observations |
| ---: | ---: |
| 0 | 886 |
| 1 | 0 |
| 2 | 14,593 |
| 3 | 14,936 |
| 4 | 15,336 |
| 5 | 15,771 |
| 6 | 14,636 |
| 7 | 15,283 |

Conclusion: candidate 2 is confirmed, but the specific mechanism is
`need_resched` continuously firing/reappearing in the idle gate. The persistent
ISR wake-buffer sub-hypothesis is not supported.

## Candidate Verdicts

| Candidate | Verdict | Evidence |
| --- | --- | --- |
| 1. BWM spin | Ruled out | BWM called `compositor_wait()` 13,170 times and skipped only once. |
| 2. Idle loop bypass | Confirmed | 13.6M idle `schedule_from_kernel()` bypasses; 13.59M due to `need_resched`. |
| 3. Bounce spin | Ruled out | Bounce was `BlockedOnIO` with only 7 cumulative CPU ticks. |
| 4. Other userspace process | Ruled out | init 85 ticks, bwm 92, bsshd 1, telnetd 0, perfdiag 6. |

## Recommended Follow-Up

Scope the fix around why idle CPUs see `need_resched` continuously. The first
place to inspect is the ARM64 idle/scheduler interaction, especially paths that
set the global or per-CPU resched flag while idle finds no runnable non-idle
work. The follow-up should not focus on BWM frame pacing or bounce frame pacing;
the evidence does not support either as the CPU runaway source.

## Instrumentation Audit

Temporary probes used for this diagnosis:

- ARM64 idle-gate trace counters in `kernel/src/tracing/providers/counters.rs`.
- Idle-gate counter increments in `kernel/src/arch_impl/aarch64/context_switch.rs`.
- Temporary `/proc/f32o_threads` backed by scheduler state.
- Temporary BWM one-second loop summaries.
- Temporary `f32o_perfdiag` userspace sampler launched by ARM64 init.

All of the above were reverted before the final commit. No Tier 1 file was
modified, and no serial breadcrumbs were added to IRQ/syscall hot paths.
