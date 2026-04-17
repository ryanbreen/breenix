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

## Fixed-State Five-Run Comparison

To avoid overfitting one race artifact, the current ARM64 kernel image was held
fixed and run five consecutive times on the same Parallels VM:

- kernel sha1: `303c91eb99a7a1ff446f666e9165f88fe5007da6`
- git commit base: `ebf15238`
- artifact root:
  - [fixed-state-303c91eb](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-303c91eb)

Per-run summary:

| Run | Artifact | Abort tid | Stable abort value | Stable pre-abort sequence |
|-----|----------|-----------|--------------------|---------------------------|
| 1 | [run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-303c91eb/run1) | 13 | `ELR=x30=0x3b9aca00` | `EINTR s1/s2/s3/s5/s6 -> WAIT s0 -> 13->idle -> idle->13 -> timer -> abort` |
| 2 | [run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-303c91eb/run2) | 11 | `ELR=x30=0x3b9aca00` | `EINTR s1/s2/s3/s5/s6 -> WAIT s0 -> 11->idle -> timer -> idle->11 -> timer -> abort` |
| 3 | [run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-303c91eb/run3) | 11 | `ELR=x30=0x3b9aca00` | `EINTR s1/s2/s3/s5/s6 -> WAIT s0 -> 11->idle -> idle->11 -> timer -> abort` |
| 4 | [run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-303c91eb/run4) | 11 | `ELR=x30=0x3b9aca00` | `EINTR s3/s5/s6 -> WAIT s0 -> 11->idle -> timer*3 -> idle->11 -> timer -> abort` |
| 5 | [run5](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-303c91eb/run5) | 13 | `ELR=x30=0x3b9aca00` | `EINTR s2/s3/s5/s6 on tid 11 -> WAIT s0 on tid 11 -> 11->idle -> timer*2 -> idle->13 -> timer -> abort` |

Cross-run commonality:

- all 5 runs soft-lock the system
- all 5 runs still show CPU 0 timer progress reaching `"[timer] cpu0 ticks=5000"`
- all 5 runs fault in EL1 with the same poisoned value:
  - `ELR=0x3b9aca00`
  - `x30=0x3b9aca00`
- all 5 runs preserve the same suspended caller LR:
  - `saved_lr=0xffff000040169df8`
- the trace window before the fault is always:
  - `check_signals_for_eintr()` completes
  - `Completion::wait_timeout()` reaches `WAIT_TIMEOUT stage 0`
  - the thread is switched away to idle
  - one or more timer ticks happen
  - a resumed blocked-in-syscall thread faults before the next successful
    post-resume `WAIT_TIMEOUT stage 1`

The thread ID is not stable (`11` in three runs, `13` in two runs), which is
strong evidence that the race is not tied to one specific task identity. The
stable part is the return corridor: the resumed thread faults with the same
`0x3b9aca00` return target after the same `wait_timeout()`/idle/resume pattern.

The preserved addresses are also now concrete:

- `0xffff000040169884` = `Completion::wait_timeout()` start
- `0xffff000040169bc8` = traced `check_signals_for_eintr()` return corridor
- `0xffff000040169be8` = traced `WAIT_TIMEOUT stage 0` corridor
- `0xffff000040169df8` = suspended caller LR preserved at fault time

Those offsets keep the investigation centered in the `Completion::wait_timeout()`
post-wake path rather than in timer delivery or generic signal checking.

Follow-up repro after adding faulting-CPU/all-CPU trace dumps:

- artifact:
  - [20260403-faulting-cpu-dump](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-faulting-cpu-dump)
- result:
  - the faulting CPU in that run was CPU 0
  - the fault still showed `ELR=x30=0x3b9aca00`
  - the faulting CPU buffer still did **not** contain `RET_DISPATCH_*`

That means "the decisive ret-dispatch event was merely hidden on another CPU"
is not sufficient to explain the missing markers. In at least one canonical
repro, the fault happens on CPU 0 without traversing the currently instrumented
`saved_by_inline_schedule` ret-dispatch branch.

## Decision Log

This section is the living control point for the investigation. Update it
whenever a branch is opened, closed, or intentionally revisited.

### Current Decision Point

We have two observable families:

- canonical family:
  - `ELR=x30=0x3b9aca00`
  - `wait_timeout() -> idle/resume -> abort`
  - CPU 0 continues taking timer ticks
- older/noisier family:
  - `INLINE_SAVE_OVERWRITE`
  - occasional AHCI timeout / `exec bsh: EIO`

Current operating rule:

- treat the canonical family as the primary root-cause corridor
- treat the overwrite/AHCI family as either:
  - an upstream race that can perturb the canonical path, or
  - a second manifestation of the same bad publication/resume bug
- do not switch full-time to the overwrite/AHCI branch unless the canonical
  family stops reproducing or the overwrite detector identifies the first bad
  writer directly

Latest high-value evidence:

- probe artifact:
  - [20260403-eret-publish-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-eret-publish-probe)
- result:
  - `CTX_PUBLISH` for `tid=11` recorded sane blocked-in-syscall state:
    - `sp=0x54231480`
    - `elr=x30=0x4016422c`
    - `flags=0b111`
  - immediately after that, `ERET_DISPATCH` for the same `tid=11` published:
    - `elr=idle_loop_arm64 (0x40165f74)`
    - `x30=0`
    - `spsr=0x5`
  - the run then hit an `ELR=0` idle-side abort before later falling through
    to the canonical `0x3b9aca00` abort on another thread

Interpretation:

- the first trustworthy divergence is now inside or immediately around
  `dispatch_thread_locked()` / ERET frame construction
- at least one blocked-in-syscall thread is not simply resuming with poisoned
  saved state; it is being actively redirected into an idle-style frame after a
  sane publication

Follow-up branch probe:

- probe artifact:
  - [20260403-dispatch-redirect-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-dispatch-redirect-probe)
- result:
  - the same blocked-in-syscall victim path records an explicit
    `DISPATCH_REDIRECT reason=3 tid=11`
  - reason `3` is `TRACE_REDIRECT_TTBR_PM_LOCK_BUSY`
  - immediately after that redirect, the ERET frame for the same tid is
    rewritten to idle-style state
  - the same run also records `DISPATCH_REDIRECT reason=6 tid=13`, which is
    the existing CPU 0 EL0 guard firing on a different user thread

Updated interpretation:

- the first concrete semantic divergence is no longer just "some code inside
  dispatch rewrote the frame to idle"
- we now know at least one canonical blocked-in-syscall failure reaches the
  `set_next_ttbr0_for_thread()` -> `TtbrResult::PmLockBusy` fallback, and that
  fallback is what rewrites the victim's dispatch frame to idle on CPU 0
- this does not yet prove `PmLockBusy` is the original bug; it may still be a
  downstream manifestation of holding `PROCESS_MANAGER` across the wrong
  corridor

Counterexample probe:

- probe artifact:
  - [20260403-pm-lock-owner-probe-long](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-pm-lock-owner-probe-long)
- result:
  - the canonical `ELR=x30=0x3b9aca00` abort reproduced on CPU 0
  - this run emitted **no** `PM_LOCK_BUSY_*` markers
  - this run emitted **no** `DISPATCH_REDIRECT` markers
  - the last ERET dispatch for `tid=11` recorded:
    - stage 2 / stage 3 `elr=x30=0x400f672c`
    - `0x400f672c` is inside `check_need_resched_and_switch_arm64`
  - a timer tick then fires, and only after that do we hit
    `EL1_INLINE_ABORT x30=0x3b9aca00`

Refined interpretation:

- `PmLockBusy` is now a confirmed branch, but not a universal explanation for
  the canonical abort
- the stronger invariant is now:
  - canonical crash happens after a blocked-in-syscall resume returns through
    the scheduler/kernel resume corridor
  - `x30` becomes `1_000_000_000` after the last traced ERET dispatch, not
    before it
- that means the next probe must stay on the post-ERET kernel return corridor,
  and `PmLockBusy` must be treated as a secondary branch unless it becomes
  repeatable again

Schedule-resume probe:

- probe artifact:
  - [20260403-schedule-resume-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-schedule-resume-probe)
- result:
  - good wake cycles now show the full healthy sequence:
    - `WAIT_TIMEOUT stage 0`
    - `SCHEDULE_RESUME stage 1`
    - `SCHEDULE_RESUME stage 2`
    - `WAIT_TIMEOUT stage 1`
    - `WAIT_TIMEOUT stage 2`
  - the failing wake cycle shows:
    - `WAIT_TIMEOUT stage 0`
    - context publication for the sleeping blocked-in-syscall thread
    - `ERET_DISPATCH stage 2/3 elr=x30=0x4014d83c`
    - one timer tick
    - `EL1_INLINE_ABORT x30=0x3b9aca00`
  - `0x4014d83c` is inside `schedule_from_kernel()`
  - there is **no** `SCHEDULE_RESUME stage 1` on the failing cycle

Current best interpretation:

- the bad cycle is no longer "somewhere after wake"
- it is specifically:
  - after the thread is re-dispatched to the resumed `schedule_from_kernel()`
    return address
  - before `schedule_from_kernel()` reaches its first traced post-switch
    instruction on that cycle
- that makes the immediate resumed `schedule_from_kernel()` tail the tightest
  current target

### Active Branches

#### Branch A: ERET Resume Corridor

- Question:
  - what mutates the resumed kernel-mode return path after the last sane ERET
    dispatch and before `x30` becomes `0x3b9aca00`?
- Why active:
  - the fixed-duration repro shows the canonical abort without any redirect
    branch
  - the last traced state before the abort is a normal ERET dispatch back into
    `check_need_resched_and_switch_arm64`
- Next evidence needed:
  - the first instruction window in resumed `schedule_from_kernel()` on the bad
    cycle
  - whether the corruption happens:
    - before the first `SCHEDULE_RESUME` trace point
    - in the earliest resumed instructions of `schedule_from_kernel()`
    - via nested interrupt/exception before that first trace point can execute

#### Branch B: TTBR / `PmLockBusy` Secondary Path

- Question:
  - when `PmLockBusy` does occur, is it a secondary amplification of the main
    bug or a separate independent race?
- Why active:
  - [20260403-dispatch-redirect-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-dispatch-redirect-probe)
    proved this branch exists
  - [20260403-pm-lock-owner-probe-long](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-pm-lock-owner-probe-long)
    proved it is not required for the canonical abort
- Next evidence needed:
  - repeatable repros that actually emit `PM_LOCK_BUSY_*`
  - owner-CPU evidence when they do
  - correlation, if any, between `PmLockBusy` and later canonical aborts

#### Branch C: Control-Field Publication

- Question:
  - which writer first publishes the bad `sp` / `elr_el1` / `x30` state for a
    blocked-in-syscall thread?
- Why active:
  - the fault preserves a stable suspended caller LR but not a sane return
    target
- Next evidence needed:
  - ordered publication trace from:
    - exception save path
    - inline schedule save path
    - ERET restore path

Current status:

- publication tracing is now in place
- for at least one canonical repro, publication looked sane before the ERET
  frame was redirected to idle
- a later fixed-duration repro reproduced the same canonical abort with no
  redirect markers at all
- `PmLockBusy` stays tracked, but it is no longer the primary branch
- Branch A remains the strongest immediate target

### Closed Branches

These are not dead forever, but we do not return to them without the stated
trigger.

#### Closed 1: Platform Timer Death

- Status:
  - closed
- Why:
  - Linux disproves it
  - Breenix keeps ticking after lockup
- Reopen only if:
  - a future repro shows timer counters or timer-marked trace progress stopping
    before scheduler failure

#### Closed 2: Generic `check_signals_for_eintr()` Failure

- Status:
  - closed
- Why:
  - trace repeatedly reaches the expected signal-check stages and post-drop
    stages before the abort
- Reopen only if:
  - a future repro shows the canonical crash before those stages complete

#### Closed 3: Hidden Ret-Dispatch On Another CPU

- Status:
  - closed as the primary explanation
- Why:
  - the faulting-CPU dump reproduced the canonical crash on CPU 0 and still did
    not emit `RET_DISPATCH_*`
- Reopen only if:
  - a future repro shows a non-CPU0 fault with decisive ret-dispatch markers on
    the owning CPU

#### Closed 4: Unknown Idle Rewrite Branch

- Status:
  - closed
- Why:
  - [20260403-dispatch-redirect-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-dispatch-redirect-probe)
    identifies a concrete branch:
    `TRACE_REDIRECT_TTBR_PM_LOCK_BUSY`
- Reopen only if:
  - a future canonical repro shows the blocked-in-syscall victim redirected to
    idle without any `PmLockBusy` marker in the owning CPU trace

#### Closed 5: `PmLockBusy` As Universal Root Cause

- Status:
  - closed
- Why:
  - [20260403-pm-lock-owner-probe-long](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-pm-lock-owner-probe-long)
    reproduced the canonical abort with no `PM_LOCK_BUSY_*` or
    `DISPATCH_REDIRECT` markers
- Reopen only if:
  - repeated fixed-state runs show every canonical abort traversing
    `PmLockBusy` before the fault

### Loop Prevention Rules

- Every experiment must name:
  - the branch it serves
  - the specific question it answers
  - the exact observation that would cause us to change branches
- We do not revisit a closed branch just because a new crash looks unfamiliar.
  We revisit only if the branch's explicit reopen condition is met.
- If a new build changes the failure mix, compare it against the last clean
  fixed-state baseline before drawing architectural conclusions.
- When a redirect branch is identified, move the next probe one layer earlier
  in the same corridor instead of reopening older symptom branches.

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
| H4 | A blocked-in-syscall or kernel-mode resume path corrupts `elr_el1` by setting bit 0 before ret-based kernel dispatch | The failing thread will show an odd EL1 resume PC that resolves to a valid kernel symbol address plus `+1`, and the PC alignment fault will precede scheduler liveness collapse | Superseded by newer evidence |
| H5 | The stable fault is a poisoned return target in the `Completion::wait_timeout()` post-wake corridor, not timer loss and not generic signal-check logic | Repeated fixed-state runs will keep producing `ELR=x30=0x3b9aca00` after `EINTR stages -> WAIT stage 0 -> idle/resume`, while `saved_lr` remains the same suspended `wait_timeout()` caller site | Supported |
| H6 | The race is not CPU0-specific and can land on whichever blocked-in-syscall thread resumes next, so CPU0-only dumps can hide the decisive trace | Fixed-state runs will vary the victim tid while preserving the same `wait_timeout()`/idle/resume/abort corridor; the next decisive trace must come from the faulting CPU rather than always CPU 0 | Supported |
| H7 | The current `RET_DISPATCH_*` probes are on the wrong branch for at least one canonical crash, so the live failing path is likely the ERET-based resume corridor rather than `saved_by_inline_schedule` ret-dispatch | A repro with explicit faulting-CPU dump will still omit `RET_DISPATCH_*` even when CPU 0 is the faulting CPU | Supported |
| H8 | `dispatch_thread_locked()` is actively rewriting at least one sane blocked-in-syscall resume target into an idle-style ERET frame on CPU 0 | A run with publication + ERET frame probes will show sane `CTX_PUBLISH` for a target tid, followed by `ERET_DISPATCH elr=idle_loop_arm64, x30=0, spsr=0x5` for that same tid before the downstream aborts | Supported |
| H9 | The poisoned `0x3b9aca00` return target is already present in the sleeping blocked-in-syscall frame before ERET dispatch | Immediately before dispatch, `ERET_RESUME_SLOT20` for the victim thread will already equal `0x3b9aca00` or some other bad target | Falsified |
| H10 | The canonical corruption happens after resume on the live kernel stack, not in the dormant sleeping frame before dispatch | Repeated runs will show sane `ERET_RESUME_SLOT20` immediately before dispatch, then later `EL1_INLINE_ABORT x30=0x3b9aca00` after successful resume markers or a later post-wake cycle | Supported |

Add new hypotheses only when they are falsifiable and tied to a predicted trace
difference.

## Recent Results

### 2026-04-03: ERET Resume Slot Probe

- Build:
  - kernel SHA1 `734b864e30d2b4fda8e6b9bc88d6f33d6e36239f`
- Artifacts:
  - [20260403-eret-slot20-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-eret-slot20-probe)
  - [20260403-eret-slot20-probe-2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-eret-slot20-probe-2)
- Question:
  - Is the sleeping blocked-in-syscall frame already poisoned before ERET
    dispatch, or does the corruption happen only after resume?
- Result:
  - In both runs, the canonical abort still reproduced with
    `EL1_INLINE_ABORT x30=0x3b9aca00`.
  - Immediately before dispatch, the victim thread's `ERET_RESUME_SLOT20`
    remained sane:
    - `tid=11`: `ERET_RESUME_SLOT20 = 0x400d7ff0` in both runs
    - `tid=13`: `ERET_RESUME_SLOT20 = 0x400d7ff0` in the second run
  - After that sane pre-dispatch state, the resumed thread still reached the
    canonical abort corridor.
- Disposition:
  - `H9` falsified
  - `H10` supported
- Consequence:
  - Stop treating dormant pre-dispatch frame corruption as the primary cause
    of the canonical `0x3b9aca00` abort. The next probe must target the live
    post-resume kernel stack corridor.

### 2026-04-03: Caller-Frame LR Probe

- Build:
  - kernel SHA1 `3c99fd7c40a5f21ee29b5b8c4cf053e53b95f50e`
- Artifact:
  - [20260403-callerlr-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-callerlr-probe)
- Question:
  - Can a frame-pointer-chain probe follow the resumed caller LR without
    perturbing the canonical failure shape?
- Result:
  - No. This build regressed to a noisier `queue_empty stuck_tid=10` /
    soft-lockup shape and did not produce a useful canonical abort trace.
- Disposition:
  - probe rejected as too intrusive / not trustworthy as reference evidence
- Consequence:
  - Do not use the frame-pointer-chain caller-LR probe as the main evidence
    path for this investigation. Keep the lower-perturbation `ERET_RESUME_*`
    probe and move the next instrumentation closer to the live post-resume
    helper chain.

### 2026-04-03: Register-Clobber Signal And Probe Refinement

- Build:
  - clean redeployed repro kernel SHA1 `b04353e8a8684ca056142b69abfc5d448e53d93b`
- Artifact:
  - [20260403-redeployed-clean-repro](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-redeployed-clean-repro)
- Result:
  - the canonical abort reproduced again on CPU 0 with
    `EL1_INLINE_ABORT x30=0x3b9aca00`
  - at abort time:
    - `x29=0xffff000040af7afc`
    - `slot20=0xffff000040b5e310`
  - those resolve to real globals, not stack addresses:
    - `x29` = `timer::TIMER_INITIALIZED`
    - `slot20` = `ahci::PORT_DMA`
- Interpretation:
  - this is a stronger signal for register clobber across an EL1 interrupt /
    return corridor, not just a dormant-frame LR overwrite theory
  - the preserved `saved_lr` still points to the blocked `wait_timeout()`
    caller site, but the live aborting frame pointer and nearby saved slot now
    look like unrelated global-state addresses

### 2026-04-03: Scheduler-Entry Probe Was Too Late

- Build:
  - kernel SHA1 `a7db2b970ee6e099cc3127effb07a0081f4ecdf0`
- Artifact:
  - [20260403-kernel-resume-irq-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-kernel-resume-irq-probe)
- Question:
  - does the timer tick immediately before the canonical abort reach
    `check_need_resched_and_switch_arm64()` while the blocked thread is still
    in the resumed `schedule_from_kernel()` window?
- Result:
  - the canonical `0x3b9aca00` abort still reproduced
  - the trace showed:
    - `ERET_DISPATCH stage 2/3 -> elr=x30=schedule_from_kernel resume`
    - one timer tick
    - `EL1_INLINE_ABORT`
  - but there were no `KERNEL_RESUME_IRQ_*` markers from the scheduler-entry
    probe
- Interpretation:
  - the decisive timer interrupt either:
    - does not reach the scheduler path before the corruption manifests, or
    - corrupts the frame earlier than `check_need_resched_and_switch_arm64()`
      can observe it
  - that moves the next meaningful probe one layer earlier, to raw timer / IRQ
    entry with direct access to the saved exception frame

### 2026-04-03: Raw Timer-Frame Probe Follow-Up

- Builds:
  - verbose raw-frame probe kernel SHA1 `c3e5bfde7ca798030016c3475cdaa5cbb3da19ff`
  - quieter follow-up kernel SHA1 `01246c95f0cc13867db4cc2d53a7f976073aeaf3`
- Artifacts:
  - [20260403-raw-timer-frame-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-raw-timer-frame-probe)
  - [20260403-raw-timer-frame-probe-quiet](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-raw-timer-frame-probe-quiet)
- Result:
  - the verbose raw-frame build still reproduced the canonical abort, but CPU 0
    dropped `4295` trace events before the fault dump, so absence of
    `KERNEL_RESUME_IRQ_*` markers is not trustworthy there
  - the quieter follow-up did not reach the old failure corridor within the
    collector window and is therefore not yet a valid replacement baseline
- Consequence:
  - do not draw architectural conclusions from the quiet follow-up yet
  - the active branch remains "capture the raw timer-frame state on a canonical
    repro without overflowing the trace buffer"

### 2026-04-03: Raw IRQ Classification Recovered The Missing Corridor

- Builds:
  - broadened IRQ classifier kernel SHA1 `536bf1232cfac1ace284f74bc593d475fbf090f7`
  - timer entry/exit probe kernel SHA1 `c3ca78b4da9bd75a6eeb6409931c2d1c16a1cac4`
  - resched-slot probe kernel SHA1 `fb150e5a9e217bdace63701422328dbd4d772b94`
- Artifacts:
  - [20260403-broadened-irq-classifier](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-broadened-irq-classifier)
  - [20260403-timer-entry-exit-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-timer-entry-exit-probe)
  - [20260403-resched-slotx30-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-resched-slotx30-probe)
- Result:
  - the canonical soft-lockup / `EL1_INLINE_ABORT x30=0x3b9aca00` corridor
    still reproduced cleanly with `dropped=0`
  - the previously missing raw timer-frame markers now appear and classify as
    `kind=3`, i.e. the nested timer IRQ is hitting the
    `check_need_resched_and_switch_arm64()` tail, not the
    `schedule_from_kernel()` body
  - the captured raw frame is:
    - `ELR=0x4010cf78` / `0x4010cf74` region
    - `x30=0x4010cc68`
  - disassembly resolves that to:
    - the nested timer lands at the `isb` immediately after
      `msr daifclr, #3` in `check_need_resched_and_switch_arm64()`
    - the interrupted `x30` still points at the earlier in-function return site
      after `rearm_timer()`
- Entry/exit conclusion:
  - the timer-handler exit probe (`kind=0x103`) shows the same
    `ELR/x30` pair as timer-handler entry (`kind=0x003`)
  - so the canonical corruption does **not** happen inside
    `timer_interrupt_handler()` or raw IRQ handling
- Suspended-slot conclusion:
  - the resched-tail slot probe records the interrupted stack's saved caller LR
    slot as stable across timer entry and timer-handler exit
  - that slot resolves to `irq_handler + 136`, which is the expected caller
    return target for `check_need_resched_and_switch_arm64()`
  - so the nested timer IRQ is **not** poisoning the suspended caller LR slot
    during raw IRQ handling
- Interpretation:
  - the active bug is now much narrower:
    - outer `check_need_resched_and_switch_arm64()` opens the `daifclr/isb`
      window
    - a nested timer IRQ lands in that window
    - the nested timer returns with a sane saved frame and sane suspended caller
      slot
    - corruption happens only after control re-enters the resched tail and
      before the final control transfer out of that corridor
  - the strongest remaining architectural hypothesis is re-entrant / nested
    `check_need_resched_and_switch_arm64()` activity on IRQ return, not timer
    death, not pre-dispatch frame poison, and not timer-handler stack clobber

### 2026-04-03: Outer Resched Tail Slot Mutates After Nested IRQ Return

- Build:
  - live resched-tail probe kernel SHA1 `bd5a0a50294141fd519b8a1027bd777a138847b6`
- Artifact:
  - [20260403-resched-tail-live-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-resched-tail-live-probe)
- Result:
  - the canonical abort reproduced again on CPU 0
  - immediately before the failing nested timer IRQ:
    - `KERNEL_RESUME_IRQ_X30 = 0x401a351c`
    - `KERNEL_RESUME_IRQ_SLOTX30 = 0x40172700`
  - `0x40172700` resolves to `irq_handler + 136`, which is the sane caller LR
    slot for the outer `check_need_resched_and_switch_arm64()` frame
  - after the nested IRQ returns, the first outer-tail probe shows:
    - `RESCHED_TAIL_X30 = 0x401a351c` (still sane)
    - `RESCHED_TAIL_SLOTX30 = 0x3b9aca00` (now poisoned)
  - only after that does the outer path hit:
    - `EL1_INLINE_ABORT x30=0x3b9aca00`
- Important implication:
  - the poison lands in the outer resched frame's saved `x30` slot **after**
    raw timer handling and **before** the outer epilogue reloads `x29/x30`
  - this rules out:
    - raw timer-handler register clobber
    - immediate register corruption at nested IRQ entry
    - dormant blocked-thread frame poison as the active branch
- Additional signal:
  - the same run emitted:
    - `INLINE_SAVE_OVERWRITE tid=11 ... elr=0x4010cf90 x30=0x401a507c`
  - that indicates a nested save path is still touching stack memory in the
    same resched-tail corridor and remains a prime candidate for the slot
    mutation mechanism
- Consequence:
  - the next justified target is the nested save / nested
    `check_need_resched_and_switch_arm64()` return interaction that can write
    into the outer frame between IRQ return and final `ret`

### 2026-04-03: Hypothesis Under Test - Nested IRQ Return Restores The Wrong EL1 SP

- Source-level observation:
  - in [boot.S](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/boot.S),
    `irq_handler` preserves `user_rsp_scratch` when `PREEMPT_ACTIVE` is set,
    but still uses `user_rsp_scratch` as the EL1 return SP later in the same
    path
- Trace correlation from
  [20260403-resched-tail-live-probe](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-resched-tail-live-probe):
  - before the nested timer IRQ:
    - `KERNEL_RESUME_IRQ_SLOTX30 = 0x40172700`
    - `RESCHED_TAIL_SP = 0x41fffe50`
  - after the nested IRQ returns:
    - `RESCHED_TAIL_SP = 0x542313a0`
    - `RESCHED_TAIL_SLOTX30 = 0x3b9aca00`
  - `0x542313a0` matches the blocked thread's published kernel resume SP, not
    the interrupted outer resched-tail SP
- Working hypothesis:
  - the slot did not actually mutate in place
  - instead, the nested IRQ returned to the outer resched tail with the wrong
    EL1 `sp`, so the outer epilogue reloaded `x29/x30` from the blocked
    thread's resume stack instead of the real outer frame
- Minimal test:
  - while `PREEMPT_ACTIVE` is still set in `irq_handler`, nested EL1 returns
    should restore `sp` from the interrupted frame (`sp + 272`) rather than
    from `user_rsp_scratch`

### 2026-04-03: Nested EL1 Resume-SP Scratch Removes The Canonical CPU0 Abort

- Change under test:
  - `irq_handler` now uses a dedicated per-CPU nested EL1 resume-SP scratch
    instead of overloading `user_rsp_scratch`
  - nested EL1 IRQ returns restore `sp` from that dedicated scratch and clear
    it after use
- First clean artifact after fixing the temporary `x17` restore regression:
  - [20260403-nested-slot-fix-test2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-nested-slot-fix-test2)
- Result:
  - the canonical CPU0 failure disappeared in this run:
    - no `EL1_INLINE_ABORT x30=0x3b9aca00`
    - no `queue_empty`
    - no soft lockup in the collector window
  - the system progressed materially further:
    - `bsshd: listening on 0.0.0.0:2222`
  - but `INLINE_SAVE_OVERWRITE` still fired repeatedly
- Interpretation:
  - the wrong-nested-SP bug was real and fixing it removed the original
    control-flow corruption
  - the remaining live signal shifted to the blocked-I/O wake path and
    dormant inline-save bookkeeping / resume semantics

### 2026-04-03: Inline-Saved Kernel Contexts Were Still Being Resumed Through ERET

- Source-level observation:
  - `schedule_from_kernel()` already honored `saved_by_inline_schedule` and
    used ret-based restore for those contexts
  - the general interrupt-driven dispatch path in
    [context_switch.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/context_switch.rs)
    restored blocked-in-syscall kernel contexts generically and ignored
    `saved_by_inline_schedule`
- Consequence:
  - a thread saved by the inline scheduler path could still be redispatched
    later through the ERET path, which violated the contract that inline-saved
    kernel contexts must resume through `aarch64_ret_to_kernel_context`
- Supporting evidence:
  - after clearing stale inline-save metadata at consumption/supersession, the
    remaining `INLINE_SAVE_OVERWRITE` reports still required
    `saved_by_inline_schedule == true`
  - that meant the flag survived until later exception-save time, which could
    only happen if the thread was resumed without passing through the existing
    ret-dispatch branch in `schedule_from_kernel()`

### 2026-04-03: General Ret-Dispatch Fix Establishes A New Baseline

- Change under test:
  - both dispatch corridors now honor `saved_by_inline_schedule`
  - the interrupt-driven `check_need_resched_and_switch_arm64()` path can now
    ret-dispatch inline-saved kernel contexts instead of forcing them through
    the generic ERET restore path
- Primary validation artifact:
  - [20260403-inline-ret-dispatch-fix-test1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-inline-ret-dispatch-fix-test1)
- Fixed-state follow-up sweep:
  - [fixed-state-inline-ret-dispatch/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-inline-ret-dispatch/run2)
  - [fixed-state-inline-ret-dispatch/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-inline-ret-dispatch/run3)
  - [fixed-state-inline-ret-dispatch/run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-inline-ret-dispatch/run4)
- Cross-run result:
  - `INLINE_SAVE_OVERWRITE` disappeared from all 4 post-fix runs
  - the old canonical failure signatures did not recur:
    - no `EL1_INLINE_ABORT x30=0x3b9aca00`
    - no `queue_empty`
  - three runs completed cleanly through userland bring-up:
    - `bsshd: listening on 0.0.0.0:2222`
    - `[init] Boot script completed`
    - `[syscall] exit(0) pid=3 name=/bin/bsh`
    - CPU0 reached `[timer] cpu0 ticks=5000`
  - one run regressed into a distinct new failure:
    - [run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-inline-ret-dispatch/run4)
    - kernel `DATA_ABORT` on CPU0 in `sys_fstat`
    - `FAR=0xfffffeffe130`
    - `ELR=0xffff00004016608c`
    - faulting thread: `tid=13 name=init_child_3_main`
    - boot script still completed before the later soft lockup
- Interpretation:
  - the old scheduler/control-flow corruption is no longer the dominant bug
  - the investigation has crossed a real decision point:
    - branch A is now effectively closed unless the old `0x3b9aca00` /
      `INLINE_SAVE_OVERWRITE` corridor reappears
    - branch B is a new, narrower post-fix bug: a kernel-mode `sys_fstat`
      user-pointer abort / deferred-fault cleanup path

### 2026-04-03: `sys_fstat` / `newfstatat` Copyout Bug

- Source-level observation:
  - both `sys_fstat()` and `sys_newfstatat()` were bypassing
    `copy_to_user()` and performing raw `ptr::write()` into userspace
  - that means an in-range-but-unmapped userspace pointer would trigger a
    kernel `DATA_ABORT` instead of returning `-EFAULT`
- Failure artifact that exposed it:
  - [fixed-state-inline-ret-dispatch/run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-inline-ret-dispatch/run4)
  - fault site:
    - `ELR=0xffff00004016608c`
    - `FAR=0xfffffeffe130`
    - resolves to `sys_fstat` storing into the user `statbuf`
- Minimal fix:
  - `Stat` is now trivially copyable
  - both `sys_fstat()` and `sys_newfstatat()` now use `copy_to_user()`
- First validation artifact:
  - [20260403-fstat-copyout-fix-test1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/20260403-fstat-copyout-fix-test1)
- Initial result:
  - returned to the clean post-ret-dispatch baseline:
    - no `DATA_ABORT`
    - no `INLINE_SAVE_OVERWRITE`
    - no soft lockup in the collector window
    - `bsshd: listening on 0.0.0.0:2222`
    - `[timer] cpu0 ticks=5000`
- Remaining caution:
  - this is only the first rerun of the copyout fix
  - the next confirmation step should be another fixed-state sweep to ensure
    the `run4` failure mode does not recur under the new build

### 2026-04-03: Ret-Dispatch For Blocked Syscalls Was Missing TTBR0 Setup

- Failure that exposed it:
  - [fixed-state-ahci-timeout-trace/run5](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-ahci-timeout-trace/run5)
  - kernel `DATA_ABORT` after `bsshd` starts:
    - `FAR=0x7ffffdf6b010`
    - `ELR=0xffff0000401cf2b0` (`memcpy`)
    - `x30=0xffff0000401560d0` (`copy_to_user`)
    - `AHCI arm port=1 cmd=7014 ... waiter_tid=13`
- Source-level divergence:
  - the normal ERET dispatch path already set up `TTBR0_EL1` for
    `blocked_in_syscall` userspace threads
  - the inline ret-dispatch corridors bypassed that setup completely
  - that meant a resumed blocked-in-syscall thread could return into kernel
    code and hit `copy_to_user()` with the wrong userspace page table active
- Minimal fix:
  - ret-dispatch now only proceeds after the same TTBR0 preparation that the
    ERET path requires
  - if TTBR0 cannot be prepared, the code falls back to the existing ERET
    corridor instead of resuming blindly
- Validation sweep:
  - [fixed-state-inline-ret-ttbr0](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-inline-ret-ttbr0)
- Result:
  - the CPU3 `copy_to_user()` / `memcpy` abort did not recur in the 5-run sweep
  - the active failure branch narrowed back to intermittent AHCI timeout /
    CPU0 liveness, not post-resume kernel copyout faults

### 2026-04-03: Cached TTBR0 Removes PM-Lock Contention From The Timeout Corridor

- New hypothesis:
  - even after the ret-dispatch TTBR0 fix, CPU0 could still hit
    `PmLockBusy` during blocked-in-syscall resume because dispatch still
    required a `try_manager()` lookup for the process page table
- Structural change under test:
  - cache the last known-good live `TTBR0_EL1` on the thread when saving
    userspace or blocked-in-syscall kernel context
  - when dispatch sees `PmLockBusy`, use the cached TTBR0 instead of
    redirecting immediately
- Validation sweep:
  - [fixed-state-cached-ttbr0](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-cached-ttbr0)
- Cross-run result:
  - 4/5 runs reached the good baseline:
    - `bsshd: listening on 0.0.0.0:2222`
    - `[timer] cpu0 ticks=5000`
  - the one remaining bad run still timed out in AHCI:
    - [run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-cached-ttbr0/run2)
  - but the bad corridor changed materially:
    - `PM_LOCK_BUSY_CONTEXT` disappeared from the bad run
    - the only remaining last-live CPU0 redirects were `reason=6`:
      `TRACE_REDIRECT_CPU0_USER_GUARD` for user threads on CPU0
- Interpretation:
  - the cached-TTBR0 change is a real architectural improvement
  - it removed one source of scheduler-side address-space contention from the
    hot resume path
  - the remaining timeout branch is now more tightly coupled to the CPU0 EL0
    redirect workaround itself

### 2026-04-03: Removing The CPU0 EL0 Redirect Guard Is Not Yet Safe

- Hypothesis under test:
  - after the cached-TTBR0 change, the remaining timeout branch might be
    sustained primarily by the CPU0 EL0 redirect workaround
- Experiment:
  - remove the CPU0 user-dispatch guard and rerun the fixed-state sweep
- Validation sweep:
  - [fixed-state-no-cpu0-guard](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-no-cpu0-guard)
- Result:
  - the experiment regressed badly
  - the first three runs all timed out early in AHCI bring-up
  - `PM_LOCK_BUSY_CONTEXT` reappeared on the first bad run
  - the sweep was stopped after 3 runs because the failure mix was clearly
    worse than the cached-TTBR0 baseline
- Disposition:
  - removing the CPU0 EL0 redirect guard is currently falsified as a
    standalone fix
  - keep the guard for now and continue from the cleaner cached-TTBR0
    baseline instead of looping back into the noisier no-guard branch

### Reopen Criteria For The Old Branch

- Reopen the old nested-resume / inline-save branch only if any of the
  following reappear on the post-fix baseline:
  - `EL1_INLINE_ABORT x30=0x3b9aca00`
  - `INLINE_SAVE_OVERWRITE`
  - `queue_empty stuck_tid=...`
  - CPU0-specific timer/scheduler death before `bsshd` comes up
- Otherwise treat the post-fix `sys_fstat` abort as the active bug.
- Update:
  - the `sys_fstat` / `copy_to_user` abort branch is now closed on the current
    baseline
  - the active branch is:
    - intermittent AHCI timeout with CPU0 timer / interrupt liveness collapse
    - on the best current baseline, that bad run no longer needs
      `PM_LOCK_BUSY_CONTEXT`; it dies with the CPU0 EL0 redirect guard still in
      the last-live CPU0 corridor

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

Treat the active bug as the remaining AHCI timeout / CPU0 liveness branch on
the cached-TTBR0 baseline.

1. Use [fixed-state-cached-ttbr0/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-cached-ttbr0/run2)
   as the current reference bad artifact, because it is the cleanest bad run
   with `PM_LOCK_BUSY_CONTEXT` removed.
2. Compare that bad run against the good cached-TTBR0 runs and isolate exactly
   why the last-live CPU0 events are still `TRACE_REDIRECT_CPU0_USER_GUARD`
   before CPU0 stops servicing work.
3. Do not revisit the no-guard branch unless a new hypothesis explains why the
   cached-TTBR0 baseline still depends on the guard in some runs and not others.

### 2026-04-03: CPU0 User-Dispatch Probe Reframed The Remaining Guarded Failure

- Fixed-state probe:
  - [fixed-state-cpu0-user-dispatch](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-cpu0-user-dispatch)
- Result:
  - the first eight runs were clean enough that the guarded branch did not
    reappear
  - the ninth run reproduced the remaining timeout branch:
    - [run9](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-cpu0-user-dispatch/run9)
  - the decisive new evidence in that bad run was:
    - CPU0 received a real EL0-capable dispatch candidate for `tid=10`
    - `ELR=0x4000f0e0`, `SPSR=0x80000000`, `TTBR0=0x44000000`
    - the CPU0 EL0 redirect guard then fired immediately:
      `TRACE_REDIRECT_CPU0_USER_GUARD`
    - later, the machine still timed out in AHCI with:
      - `cpu0_dispatch: tid=10 elr=0xffff0000400fda64 spsr=0x5`
      - `cpu0_breadcrumb=43`
      - `PPI27_pending=1`
- Interpretation:
  - the remaining guarded failure is not "CPU0 never had an EL0 candidate"
  - CPU0 does reach a real EL0-capable dispatch point, the workaround guard
    redirects it away, and CPU0 still later wedges in a kernel-mode dispatch
    corridor
  - that means the guard is not containing the entire bug; it is only changing
    where the surviving failure is expressed

### 2026-04-03: Unguarded User-Dispatch Probe Was Not A Clean Comparison

- Experiment:
  - temporarily remove the CPU0 EL0 redirect guard on the same probe build
- Artifact:
  - [no-cpu0-guard-user-dispatch-probe/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-cpu0-guard-user-dispatch-probe/run1)
- Result:
  - the machine ran much further and reached:
    - `bsshd: listening on 0.0.0.0:2222`
    - `Welcome to Breenix OS`
  - it still timed out later with:
    - `cpu0_dispatch: tid=13 elr=0xffff00004010b204 spsr=0x82000305`
    - `cpu0_breadcrumb=43`
    - `PPI27_pending=1`
  - the CPU0 trace buffer wrapped heavily:
    - `write_idx=68578`
    - `dropped=67554`
  - no decisive `CPU0_USER_DISPATCH_*` evidence survived in that wrapped trace
- Disposition:
  - this run is not a clean answer to the guarded `tid=10` branch
  - keep the guard in place for now and continue from the quieter guarded
    baseline

### 2026-04-03: Exception-Frame Breadcrumb Probe Collapsed The Live Window

- Probe artifacts:
  - [guarded-asm-breadcrumb-probe/run10](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-probe/run10)
  - [guarded-asm-breadcrumb-probe-batch2/run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-probe-batch2/run4)
- Probe design:
  - add CPU0-only assembly breadcrumbs inside
    `aarch64_enter_exception_frame()`:
    - `107`: after `mov sp, x0`
    - `108`: after `ELR_EL1` / `SPSR_EL1` are programmed
    - `109`: after the target-SP pivot
    - `110`: just before `eret`
- Cross-artifact result:
  - both bad guarded runs now converge on the same terminal breadcrumb:
    - `cpu0_breadcrumb=107`
  - the same two runs end with:
    - `cpu0_dispatch: tid=11 elr=0xffff0000400fda64 spsr=0x5`
      on [run10](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-probe/run10)
    - `cpu0_dispatch: tid=10 elr=0xffff0000400fda64 spsr=0x5`
      on [run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-probe-batch2/run4)
  - `0xffff0000400fda64` resolves to `idle_loop_arm64`
  - the last sampled CPU0 timer PCs differ across the two bad runs:
    - [run10](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-probe/run10):
      `cpu0_last_timer_elr=0xffff0000400c0bf0`
      inside `AhciBlockDevice::read_block()`
    - [run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-probe-batch2/run4):
      `cpu0_last_timer_elr=0xffff0000400fdc68`
      inside `idle_loop_arm64`
  - there are currently no observed bad runs with:
    - `cpu0_breadcrumb=108`
    - `cpu0_breadcrumb=109`
    - `cpu0_breadcrumb=110`
- Interpretation:
  - the remaining guarded timeout branch now clusters in a much smaller place:
    inside `aarch64_enter_exception_frame()`, after the frame pointer is
    adopted as `sp`, but before the path reaches the existing "ELR/SPSR
    programmed" breadcrumb
  - the terminal `cpu0_dispatch` state is consistently an idle-style target
    even when the victim thread ID changes
  - the timer side is still race-shaped, but the handoff-side clustering is now
    stable enough to justify one more minimal assembly probe inside the
    `107 -> 108` corridor

### Active Decision Point

- Current best branch:
  - the live failure window is no longer generic "CPU0 liveness collapse"
  - it is the first half of `aarch64_enter_exception_frame()`, before the
    existing `108` breadcrumb
- Immediate next probe:
  - add one additional CPU0-only breadcrumb between `107` and `108`, after the
    frame `ELR` has been read and normalized but before `msr elr_el1`
  - rerun fixed-state guarded captures
  - use the new terminal breadcrumb to decide whether the next cut should be:
    - frame-read / fallback-normalization
    - `ELR_EL1` programming
    - or `SPSR_EL1` programming

### 2026-04-03: The New Seam Probe Still Dies Before ELR Normalization Completes

- Probe artifact:
  - [guarded-asm-breadcrumb-probe-111/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-probe-111/run1)
- Probe design:
  - add `111` inside `aarch64_enter_exception_frame()` after the frame `ELR`
    has been read / normalized and just before `msr elr_el1`
- Result:
  - the very first fixed-state rerun reproduced the timeout branch
  - the new terminal state is still:
    - `cpu0_breadcrumb=107`
  - there is still no observed `cpu0_breadcrumb=111`
  - this run ended with:
    - `cpu0_dispatch: tid=13 elr=0xffff00004014dfb4 spsr=0x82000305`
    - `0xffff00004014dfb4` is inside `sys_nanosleep()`, immediately after
      `wfi`, at the call to `check_signals_for_eintr()`
    - `cpu0_last_timer_elr=0xffff0000400fdc94`
      inside `idle_loop_arm64`, immediately after `wfi`
- Updated interpretation:
  - the live handoff window is narrower again:
    - after `mov sp, x0`
    - before the code reaches the new "frame `ELR` load/normalize complete"
      marker
  - the next justified cut is no longer `111 -> 108`
  - it is:
    - frame `ELR` load itself
    - the low-address idle fallback compare/branch
    - or the fallback stores that rewrite the frame to `idle_loop_arm64`

### 2026-04-03: Stack-Local Trampoline Frame Was Arriving At The Handoff With `elr_slot=0`

- Probe artifacts:
  - [guarded-asm-breadcrumb-sp/run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-sp/run4)
  - [guarded-asm-breadcrumb-elr-slot/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/guarded-asm-breadcrumb-elr-slot/run3)
- Results:
  - the `107` snapshot showed a sane adopted frame pointer:
    - `sp=0xffff0000431ffea8`
  - that address is inside CPU0's per-CPU scheduler stack:
    - CPU0 stack range:
      `0xffff000043000000..0xffff000043200000`
    - distance from stack top:
      `0x158`
  - the same bad run still died at breadcrumb `107`
  - after adding a raw ELR-slot snapshot, the bad run showed:
    - `elr_slot=0x0`
    - while the timeout still reported a nonzero CPU0 dispatch target:
      - `cpu0_dispatch: tid=13 elr=0xffff000040147894 spsr=0x82000305`
      - `0xffff000040147894` is inside `sys_nanosleep()`
- Interpretation:
  - `dispatch_thread_locked()` was not simply producing a zero `frame.elr`
  - the stronger mismatch was:
    - CPU0 recorded a sane dispatch target before the IRQ window
    - the frame consumed by `aarch64_enter_exception_frame()` later had
      `elr_slot=0`
  - this pointed upstream to the stack-local frame itself being clobbered
    between the pre-ERET `daifclr` window and the handoff call

### 2026-04-03: The Active Root Cause Became The Inline-Trampoline Stack Geometry

- Code-path comparison:
  - `check_need_resched_and_switch_arm64()` reuses the already-live exception
    frame at current `SP`
  - nested IRQs there push a new frame below the existing one, which is why
    that path's comment about frame integrity remains coherent
  - `inline_schedule_trampoline()`, however, allocated a fresh
    `Aarch64ExceptionFrame` as a stack local near the top of the scheduler
    stack, then opened the IRQ window before calling
    `aarch64_enter_exception_frame()`
- Why that geometry is bad:
  - the trampoline-local frame sat only `0x158` below the top of CPU0's live
    scheduler stack
  - nested IRQ/exception entry in the pre-ERET window uses that same stack
  - that created a concrete overwrite path for the stack-local dispatch frame
    before `aarch64_enter_exception_frame()` consumed it
- Root-cause statement:
  - the remaining guarded AHCI timeout branch was not "CPU0 cannot handle ERET"
  - it was "the inline-schedule trampoline enables IRQs while its prepared ERET
    frame still lives on the live IRQ stack"

### 2026-04-03: Moving The Trampoline ERET Frame Off The Live IRQ Stack Eliminated The Timeout Branch In The First Sweep

- Structural change under test:
  - replace the inline-trampoline stack-local `Aarch64ExceptionFrame` with
    per-CPU scratch storage
  - keep the pre-ERET IRQ window intact
  - only change the lifetime / placement of the prepared frame
- Validation sweep:
  - [fixed-state-inline-frame-relocation](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-inline-frame-relocation)
- Cross-run result:
  - 5/5 runs were clean
  - all 5 runs had:
    - `soft_lockup=0`
    - `queue_empty_lines=0`
    - no `AHCI: command timeout`
    - no `cpu0_breadcrumb=...`
    - no `cpu0_dispatch: ...`
    - no `DISPATCH_REDIRECT`
    - no `CPU0_USER_DISPATCH_*`
  - CPU0 timer progress stayed live on every run:
    - `last_cpu0_tick=5000` or `10000`
  - 3/5 runs reached:
    - `bsshd: listening on 0.0.0.0:2222`
- Current interpretation:
  - this is the strongest architectural fix result in the investigation so far
  - it directly matches the identified divergence:
    move the prepared ERET frame off the stack that services the nested IRQ
    window
  - the old `elr_slot=0` / breadcrumb-`107` timeout branch has not recurred in
    the first fixed-state sweep after that change

### 2026-04-03: Removing The CPU0 EL0 Guard After The Frame Fix Still Exposes A New Branch

- Experiment:
  - temporarily remove the CPU0 EL0 redirect guard on top of the relocated-frame baseline
- Validation sweep:
  - [no-cpu0-guard-after-frame-relocation](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-cpu0-guard-after-frame-relocation)
- Result:
  - [run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-cpu0-guard-after-frame-relocation/run1)
    reached `cpu0 ticks=5000` cleanly
  - [run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-cpu0-guard-after-frame-relocation/run2)
    timed out in AHCI, but not via the old `107/elr_slot=0` branch:
    - `cpu0_breadcrumb=50`
    - `ctl=0x5`
    - `sp=0xffff000041ba9460`
    - `elr_slot=0xffff000040191e10`
    - `cpu0_dispatch: tid=0 elr=0xffff000040194e18 spsr=0x5`
  - [run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-cpu0-guard-after-frame-relocation/run3)
    soft-locked after reaching `cpu0 ticks=5000`, with repeated
    `CPU0_USER_DISPATCH_STAGE` markers and no redirect markers
- Disposition:
  - the old stack-local trampoline-frame corruption branch appears neutralized
  - but CPU0 EL0 dispatch is not yet cleanly solved on the new baseline
  - keep the guard in tree for now while treating the post-fix no-guard result
    as a new, separate branch rather than a regression to the old one

### 2026-04-03: The New No-Guard Branch Points At Deferred-Requeue Contract Pressure, Not Generic CPU0 EL0 Failure

- Cross-run comparison:
  - the relocated-frame guarded baseline
    [fixed-state-inline-frame-relocation](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/fixed-state-inline-frame-relocation)
    has no `DEFER_EVICT`
  - the post-fix no-guard sweep
    [no-cpu0-guard-after-frame-relocation](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-cpu0-guard-after-frame-relocation)
    shows `DEFER_EVICT` in all 3 runs
- Stable observations from the no-guard runs:
  - CPU0 is not simply unable to run EL0:
    - in `run3`, CPU0 repeatedly performs real `0 -> 13 -> 0` switches
    - `CPU0_USER_DISPATCH_STAGE` appears only on some of those `0 -> 13`
      handoffs, after `EXEC_ENTRY pid=3`
  - `tid=13` behaves normally on CPUs 1-3 for a long interval before CPU0 joins
    the ownership set
  - the bad collapse in `run3` is:
    - `[DEFER_EVICT] cpu=0 evicted=1 new=11`
    - then `dispatch_thread: bad context tid=11 ... cpu=3`
    - postmortem for `tid=11` shows `sp=0xffff0000431fffb0`, which is on CPU0's
      scheduler stack rather than the thread's own kernel stack
- Interpretation:
  - the current no-guard branch is not best described as
    "CPU0 EL0 dispatch immediately kills timer semantics"
  - the first code path unique to this branch is pressure on the deferred
    requeue mechanism, specifically the eviction path that requeues an older
    switched-out thread before the normal deferred-slot drain point
- Active hypothesis:
  - `DEFER_EVICT` is making a thread runnable again while some live frame or
    stack-ownership assumption is still outstanding
  - if true, the next probe should instrument deferred-slot store, drain, and
    eviction directly rather than adding more EL0 symptom probes

### 2026-04-03: Next Probe Records Deferred-Requeue Store, Drain, And Eviction

- Decision:
  - add low-overhead trace-buffer events for:
    - early deferred drain at IRQ entry
    - normal deferred drain in the consolidated resched path
    - inline-schedule deferred drain
    - deferred-slot store
    - deferred-slot eviction
- Data recorded for each event:
  - `tid`
  - stage id
  - saved `sp`
  - saved `elr`
  - saved `x30`
  - compact flags for thread state / blocked-in-syscall / inline-saved /
    has-started / user-vs-kernel
  - counterpart `tid` where relevant:
    - previous slot occupant on store
    - replacement `old_id` on eviction
- Why this is the right cut:
  - it tests the first branch that is unique to the no-guard failures
  - it keeps the probe on the scheduler contract itself instead of jumping back
    to generic CPU0 EL0 speculation

### 2026-04-03: Re-running The No-Guard Branch Showed The Failure Is Not Specific To `tid=13`

- Probe artifacts:
  - [no-guard-defer-requeue-probe/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-guard-defer-requeue-probe/run1)
  - [no-guard-defer-requeue-probe/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-guard-defer-requeue-probe/run2)
  - [no-guard-defer-requeue-probe/run4-long](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/no-guard-defer-requeue-probe/run4-long)
- Results:
  - `run1` reproduced the bad-context/soft-lockup branch quickly:
    - `DEFER_EVICT cpu=0 evicted=1 new=11`
    - then `dispatch_thread: bad context tid=11 ... cpu=0`
    - `stuck_tid=13`
  - `run2` reproduced the earlier early-abort branch instead:
    - `DEFER_EVICT cpu=0 evicted=1 new=10`
    - then `INSTRUCTION_ABORT FAR=0x0 ELR=0x0`
    - no `CPU0_USER_DISPATCH_STAGE`
  - `run4-long` produced a different but still relevant no-guard lockup:
    - one `DEFER_EVICT cpu=0 evicted=1 new=10`
    - soft lockup with `stuck_tid=11`
    - CPU0 `current=11`
    - postmortem showed `tid=10` published with
      `sp=0xffff0000431fffb0`, which is on CPU0's scheduler stack
    - CPU0 trace buffer showed repeated `0 <-> 11` wait-timeout loops, not a
      `tid=13` ownership problem
- Interpretation:
  - the no-guard failure is broader than
    "CPU0 starts running `tid=13` after `EXEC_ENTRY pid=3` and then wedges"
  - CPU0 can also get stuck in a `0 <-> 11` blocked-syscall loop earlier in
    boot, and another userspace thread (`tid=10`) can still end up published
    with CPU0 scheduler-stack state
  - `DEFER_EVICT` remains the only branch-local mechanism seen across these
    no-guard failures, but the victim thread is not stable

### 2026-04-03: The Next Durable Postmortem Needs Deferred-Requeue Snapshots In Soft-Lockup Dumps

- Problem encountered:
  - the ring buffer did not reliably preserve the new deferred-slot trace events
    long enough to survive into the postmortem dump
  - the bad-context snapshot path only fires on one branch; the long no-guard
    run wedged before hitting that path
- Adjustment:
  - retain the deferred-slot trace events
  - add per-CPU deferred-requeue snapshots to the soft-lockup dump as well
- Why:
  - the current investigation question is no longer "did `DEFER_EVICT` happen?"
  - it is "which thread/control-state did CPU0 last publish into the deferred
    requeue contract immediately before the scheduler-stack contamination shows
    up in another user thread?"

### 2026-04-03: Fresh Parallels Deployment Corrected The Ground Truth For The Current Local Kernel

- Important correction:
  - several earlier local collector runs on this branch were not trustworthy
    for the latest source state
  - `scripts/parallels/collect-breenix-cpu0-traces.sh` reboots and captures the
    existing Parallels VM, but does not rebuild or redeploy the ARM64 EFI image
  - after noticing that new postmortem markers were absent from the timeout
    output, the investigation switched back to the real deployment path:
    - build `parallels-loader` for `aarch64-unknown-uefi`
    - build `kernel-aarch64` for `aarch64-breenix.json`
    - deploy/boot with `./run.sh --parallels --test 20`
- Fresh deployed VM:
  - `breenix-1775259455`
- Consequence:
  - only runs captured after that deployment are authoritative for the current
    local no-guard tree
  - stale-VM local runs remain useful historical context, but not evidence for
    what the current source tree actually does

### 2026-04-03: The Properly Deployed No-Guard Kernel Currently Fails In The AHCI Completion Corridor

- Trustworthy artifacts:
  - [post-deploy-current-kernel-run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-current-kernel-run1)
  - [post-deploy-current-kernel-sweep1/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-current-kernel-sweep1/run1)
  - [post-deploy-current-kernel-sweep1/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-current-kernel-sweep1/run2)
  - [post-deploy-current-kernel-sweep1/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-current-kernel-sweep1/run3)
- Results:
  - `run1`: early AHCI timeout on port 1 during init
  - `run2`: no timeout within the collector window
  - `run3`: same early AHCI timeout on port 1 during init
  - none of these three runs reproduced the older immediate `queue_empty` or
    soft-lockup corridor inside the collector window
- Stable timeout-side observations from the two failing runs:
  - the timeout happens shortly after `[init] Breenix init starting (PID 1)`
  - `[init] bsshd started (PID 2)` does not appear before the timeout
  - hardware AHCI IRQs still arrive:
    - `IS=0x1`
    - `isr_count=1186`
    - `port_isr_hits=[1,1185]`
  - the waiter still times out:
    - `completion_done=0`
    - `complete_hits=[0,4]`
  - the last observed interrupt CPU is consistently CPU 0:
    - `isr_last_hw_cpu=0`
  - the interrupted EL1 PC is consistently inside
    `kernel::arch_impl::aarch64::context_switch::inline_schedule_trampoline`
    rather than AHCI itself:
    - `isr_last_elr=0xffff000040130098`
- Updated interpretation:
  - the active branch on the correctly deployed local no-guard tree is no
    longer best described as generic timer loss or immediate scheduler collapse
  - interrupts are arriving, but the completion/wakeup contract for the AHCI
    wait path is still failing often enough to time out during early init
  - the next evidence cut should stay on the publication chain:
    - `setup_cmd_slot0()` arms the command
    - `handle_interrupt()` swaps `PORT_ACTIVE_CMD_NUM`
    - `Completion::complete()` publishes `done`
    - `isr_unblock_for_io()` pushes the waiter into the wake buffer
    - scheduler drain wakes the blocked thread

### 2026-04-03: Next Probe Narrows To Completion Publication, ISR Wake Push, And Scheduler Drain

- Decision:
  - add minimal snapshot-style instrumentation at exactly these ownership
    boundaries:
    - `Completion::reset()`
    - waiter registration in `Completion::wait_timeout()`
    - `Completion::complete()`
    - `isr_unblock_for_io()`
    - wake-buffer drain in `schedule()` / `schedule_deferred_requeue()`
- Why this is the right cut:
  - the current timeout branch already proves that AHCI IRQs arrive
  - the missing fact is whether the same completion object sees a matching
    `complete(token)` and, if it does, whether the waiter TID is successfully
    pushed and later drained
  - this is lower perturbation than adding new string-heavy live-path logging
    or broad trace-buffer spam, and it directly distinguishes:
    - publication failure
    - wake-buffer push failure
    - scheduler-drain failure

### 2026-04-03: Completion-Chain Snapshots Falsified The Wake-Propagation Branch

- Probe artifacts:
  - [post-deploy-completion-chain-sweep/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-completion-chain-sweep/run1)
  - [post-deploy-completion-chain-sweep/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-completion-chain-sweep/run3)
- Stable result across the timeout runs:
  - the timed-out token is **not** the token seen by `Completion::complete()`
  - instead, the snapshot chain consistently shows:
    - `reset.done_before = previous_token`
    - waiter registration for `current_token`
    - `complete(previous_token)`
    - successful `isr_wake_push`
    - successful wake-buffer drain for the same waiter
  - example:
    - timeout token `1189`
    - `complete(token=1188, waiter_tid=11)`
    - `isr_wake_push tid=11 pushed=1`
    - `isr_wake_drain tid=11 batch=1`
- Interpretation:
  - the previously suspected branch
    "AHCI completion publishes the token but wake propagation loses it"
    is falsified for the current timeout corridor
  - the current command never reaches `complete(current_token)` at all
  - the next branch has to stay earlier, at active-command ownership and the
    delivery of the current AHCI interrupt itself

### 2026-04-03: The Timed-Out Command Remains The Live Active Command At Timeout

- Probe artifacts:
  - [post-deploy-active-cmd-probe/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-active-cmd-probe/run1)
  - [post-deploy-active-cmd-probe/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-active-cmd-probe/run2)
- Stable timeout-side state:
  - `CI=0`
  - `IS=0x1`
  - `SPI34 pend=true act=false`
  - `active_cmd_num=current_token`
  - `active_mask=0x1`
  - `last_armed=current_token`
  - `last_complete=previous_token`
- Examples:
  - `active_cmd_num=1189` and `last_complete=1188`
  - `active_cmd_num=7049` and `last_complete=7048`
- Interpretation:
  - software is **not** losing ownership of the current command by clearing
    `PORT_ACTIVE_CMD_NUM`
  - the command is still published as active, while hardware shows the command
    itself is done (`CI=0`) and the port has a pending D2H interrupt (`IS=0x1`)
  - this means the current branch is:
    - the completion interrupt for the current command exists
    - but it is not being taken / serviced before the waiter times out

### 2026-04-03: Linux Does Not Justify A CPU-Affinity Workaround Here

- Linux probe reference:
  - `/proc/interrupts` on `linux-probe` shows the same Parallels AHCI device on
    `GICv3 34 Level ahci[PRL4010:00]`
  - `/proc/irq/15/smp_affinity_list` is `0-3`
  - `/proc/irq/15/effective_affinity_list` is `0`
- Interpretation:
  - Linux currently handles the AHCI interrupt effectively on CPU 0 on this
    platform
  - so "route the AHCI SPI away from CPU 0" is not justified as a root-cause
    fix, even though Linux allows a broader affinity mask
  - the divergence to explain is still:
    - Linux can service the pending SPI on CPU 0
    - Breenix sometimes leaves the same SPI pending and unserviced on CPU 0

### 2026-04-03: CPU0 Is Not Trivially Masking IRQs At The Sticky Breadcrumb

- Probe artifact:
  - [post-deploy-breadcrumb-daif-probe/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-breadcrumb-daif-probe/run1)
- Result:
  - the timeout reproduces with:
    - `cpu0_breadcrumb=107`
    - `daif=0x300`
    - `pmr=0xf8`
    - `SPI34 pend=true act=false`
    - `active_cmd_num=1189`
  - `isr_last_elr` is still inside
    `inline_schedule_trampoline`
- Interpretation:
  - at the sticky CPU 0 handoff point, live `DAIF` does **not** show IRQ masked
    (`I=0`)
  - `ICC_PMR_EL1=0xf8` should admit the AHCI SPI priority (`0xa0`)
  - so this branch is not explained by a simple "CPU 0 forgot to unmask IRQs"
    or "priority mask still blocks the device interrupt"
  - the next target is now specifically the exception-return / pending-SPI
    window around:
    - `inline_schedule_trampoline`
    - `aarch64_enter_exception_frame`
    - the final ERET handoff to the resumed EL1/EL0 context

### 2026-04-03: Raising SPI34 Priority Above The Timer PPI Did Not Fix The Timeout Corridor

- Hypothesis under test:
  - CPU 0 might be starving AHCI SPI34 behind a continuously pending timer PPI
    because both were running at priority `0xa0`
  - test change:
    - raise `SPI34_priority` to `0x90`
    - leave `PPI27_priority` at `0xa0`
- Probe artifacts:
  - [post-deploy-ahci-priority-test/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-ahci-priority-test/run1)
  - [post-deploy-ahci-priority-test/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-ahci-priority-test/run2)
  - [post-deploy-ahci-priority-test/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-ahci-priority-test/run3)
  - [post-deploy-ahci-priority-test/run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-ahci-priority-test/run4)
  - [post-deploy-ahci-priority-test/run5](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-ahci-priority-test/run5)
- Result:
  - the change did **not** eliminate the AHCI timeout branch
  - the five-run sweep produced:
    - 2 runs that reached the normal early-userland path without a timeout in
      the collector window (`run1`, `run4`)
    - 3 runs that still timed out (`run2`, `run3`, `run5`)
  - the bad runs all confirm the test was live:
    - `SPI34_priority=0x90`
    - `PPI27_priority=0xa0`
  - the canonical bad shape still exists under the priority bump:
    - `active_cmd_num=current_token`
    - `last_complete=previous_token`
    - `CI=0`
    - `IS=0x1`
    - `SPI34 pend=true act=false`
    - `cpu0_breadcrumb=107`
    - `PPI27_pending=1`
  - `run2` even drifted into a noisier branch with:
    - `INSTRUCTION_ABORT`
    - `UNHANDLED_EC`
    - `SPI34 pend=true act=true`
    - the same AHCI timeout afterward
- Interpretation:
  - "equal-priority starvation behind the timer PPI" is **not** a sufficient
    explanation for the current timeout corridor
  - if priority contributes at all, it is not the primary missing invariant
  - compared with the earlier cached-TTBR0 baseline (`4/5` good), this probe is
    not an improvement and may have widened the failure mix
  - this branch is therefore treated as falsified for root-cause purposes, and
    the temporary priority override should not remain in the tree
- Next target:
  - compare Linux and Breenix IRQ-exit semantics more directly, especially
    whether Linux drains additional pending IRQs before the resched/ERET
    corridor while Breenix services only one IRQ and returns

### 2026-04-03: Linux IRQ-Exit Semantics Differ From Breenix At The Critical Window

- Primary-source reference:
  - Linux ARM64 entry path:
    - `arch/arm64/kernel/entry-common.c`
    - official mirror used for this pass:
      `https://android.googlesource.com/kernel/common/+/fee48f3bdd751/arch/arm64/kernel/entry-common.c`
  - Linux GICv3 irqchip path:
    - `drivers/irqchip/irq-gic-v3.c`
    - official mirror used for this pass:
      `https://android.googlesource.com/kernel/common/+/refs/tags/android15-6.6-2024-11_r15/drivers/irqchip/irq-gic-v3.c`
- Linux facts from source:
  - `el1_interrupt()` does:
    - exception entry
    - `do_interrupt_handler(regs, handle_arch_irq)`
    - optional `arm64_preempt_schedule_irq()`
    - exception exit
  - `gic_handle_irq()` reads IAR and, on the normal IRQ-enabled path, does:
    - `gic_pmr_mask_irqs()`
    - `gic_arch_enable_irqs()`
    - then `__gic_handle_irq(...)`
  - this means Linux does **not** rely on a custom "final pre-ERET IRQ window"
    to admit a follow-on normal IRQ on the same CPU
- Breenix facts from current tree:
  - `handle_irq()` acknowledges one IRQ, handles it, EOIs it, runs softirqs,
    and returns
  - the actual context-switch / resumed-return logic then happens in
    `check_need_resched_and_switch_arm64()`
  - Breenix opens a bespoke nested-IRQ window only at the tail of that path:
    - `msr daifclr, #3`
    - `isb`
    - return to `boot.S`
    - final `ERET`
- Inference:
  - this is the first architectural divergence that directly matches the
    current breadcrumb:
    - pending SPI34
    - sticky CPU0 breadcrumb `107`
    - timeout-side `isr_last_elr` in the trampoline / resched corridor
  - this does **not** prove the root cause yet
  - it does justify the next probe:
    - determine whether the AHCI completion SPI is getting stranded because
      Breenix only gives it a chance in the bespoke pre-ERET window, while
      Linux re-opens IRQ handling earlier under irqchip control

### 2026-04-03: Breenix Is Missing Generic HARDIRQ Accounting For Non-Timer IRQs

- Local source facts:
  - generic ARM64 IRQ dispatch lives in:
    - [exception.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/exception.rs)
  - generic hardirq accounting helpers live in:
    - [per_cpu_aarch64.rs](/Users/wrb/fun/code/breenix/kernel/src/per_cpu_aarch64.rs)
    - [percpu.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/percpu.rs)
  - `irq_enter()` / `irq_exit()` are currently called only by:
    - [timer_interrupt.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/timer_interrupt.rs)
- Concrete divergence:
  - `handle_irq()` dispatches AHCI and other SPIs without a generic
    `irq_enter()` / `irq_exit()` wrapper
  - the timer path is special-cased and does maintain HARDIRQ count
  - AHCI completion IRQs therefore run without generic hardirq accounting
- Why this matters:
  - code that checks `in_interrupt()` / `in_hardirq()` gets the wrong answer for
    AHCI and other non-timer IRQs
  - softirq processing and irq-exit logic are then running on a semantic model
    that already diverges from Linux before the scheduler tail even starts
  - this is exactly the class of bug that can hide as a race because the system
    "usually works" until one path depends on correct hardirq nesting state
- Current disposition:
  - not fixed yet
  - strong next hypothesis
- Next target:
  - validate whether moving HARDIRQ entry/exit accounting to the generic
    `handle_irq()` wrapper, and removing the timer-only special-case accounting,
    collapses the AHCI timeout corridor without reintroducing the older CPU0
    resume corruption branches

### 2026-04-03: Generic HARDIRQ Accounting Probe Applied

- Probe change:
  - move `irq_enter()` / `irq_exit()` ownership to the generic ARM64
    `handle_irq()` wrapper
  - remove the timer-only `irq_enter()` / `irq_exit()` calls so the timer path
    no longer double-counts HARDIRQ nesting
- Why this probe is justified:
  - it aligns Breenix more closely with Linux's generic EL1 IRQ entry/exit model
  - it corrects a real semantic mismatch for AHCI and other SPIs
  - it is narrower than changing IRQ-priority policy or inventing another
    special-case CPU0 dispatch rule
- What this probe should answer:
  - whether the AHCI timeout corridor depends on non-timer IRQs falsely running
    as if they were thread context
  - whether `do_softirq()` / resched-on-IRQ-exit behavior becomes more stable
    once AHCI completion IRQs participate in the same HARDIRQ accounting model as
    timer IRQs
- Reopen criteria if this fails:
  - if the timeout reproduces unchanged, the generic HARDIRQ mismatch is not the
    dominant root cause for this branch
  - if old CPU0 resume corruption signatures come back, then the new probe is
    interacting with nested-return assumptions and must be analyzed on that axis

### 2026-04-04: Generic HARDIRQ Accounting Did Not Collapse The AHCI Timeout Corridor

- Probe artifacts:
  - [post-deploy-generic-hardirq-accounting/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-generic-hardirq-accounting/run1)
  - [post-deploy-generic-hardirq-accounting/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-generic-hardirq-accounting/run2)
  - [post-deploy-generic-hardirq-accounting/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-generic-hardirq-accounting/run3)
  - [post-deploy-generic-hardirq-accounting/run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-generic-hardirq-accounting/run4)
  - [post-deploy-generic-hardirq-accounting/run5](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-generic-hardirq-accounting/run5)
- Result:
  - `run1` reached the normal boot corridor and `cpu0 ticks=5000`
  - `run2` was inconclusive in the collector window: no timeout, no soft lockup,
    but it also did not reach the later boot markers before the 25-second stop
  - `run3`, `run4`, and `run5` all reproduced the canonical AHCI timeout branch
- Stable bad-run state:
  - `SPI34_priority=0xa0`
  - `PPI27_priority=0xa0`
  - `active_cmd_num=current_token`
  - `last_complete=previous_token`
  - `CI=0`
  - `IS=0x1`
  - `SPI34 pend=true act=false`
  - `cpu0_breadcrumb=107`
  - `cpu0_last_timer_elr=0xffff0000401abbec`
- Interpretation:
  - generic HARDIRQ accounting is a real semantic correction, but it does **not**
    collapse the currently dominant AHCI timeout corridor by itself
  - the bad branch still points at the same pending-SPI / CPU0 return window as
    before
  - importantly, this probe did **not** reintroduce the older dominant CPU0
    resume-corruption signatures in this five-run sample
- Disposition:
  - the "missing HARDIRQ accounting is the dominant root cause" branch is
    falsified
  - the broader "IRQ admission / return semantics differ from Linux" branch
    remains open
- Next target:
  - return to the Linux/Breenix IRQ-admission divergence:
    - Linux re-opens IRQ handling inside the irqchip path under PMR control
    - Breenix still relies on the bespoke pre-ERET `daifclr` window
  - the next probe should focus on that admission model, not on another token /
    waiter / completion-publication branch

### 2026-04-04: Admission-Model Probe Moved The IRQ Window Into The Timer Handler

- Probe change:
  - after masking the timer source and re-arming it, temporarily re-enable
    IRQ/FIQ inside `timer_interrupt_handler()`
  - only do this when the interrupted context had IRQ/FIQ unmasked in
    `SPSR_EL1`
  - restore masked state before returning to the generic IRQ-exit path
- Why this is the right next cut:
  - it directly tests the still-open Linux/Breenix divergence:
    - Linux admits follow-on IRQ work much earlier under irqchip control
    - Breenix currently tends to admit it only in the bespoke pre-ERET window
  - it changes one variable:
    - *when* a pending SPI can be admitted during the timer corridor
  - it does **not** claim to be the final architecture
- Prediction:
  - if the AHCI timeout branch is primarily caused by “SPI34 waits too long for
    admission while CPU0 is busy in the timer/resched corridor,” timeout rate
    should drop materially on the same five-run sweep
  - if the timeout branch is unchanged, then mere earlier DAIF admission inside
    the timer body is not enough, and the remaining divergence is likely deeper
    in GIC/EOI/return semantics

### 2026-04-04: Wide Timer-Body Admission Window Removed AHCI Timeouts But Exposed A New Corruption Branch

- Probe artifacts:
  - [post-deploy-timer-early-admission/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-timer-early-admission/run1)
  - [post-deploy-timer-early-admission/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-timer-early-admission/run2)
  - [post-deploy-timer-early-admission/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-timer-early-admission/run3)
  - [post-deploy-timer-early-admission/run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-timer-early-admission/run4)
  - [post-deploy-timer-early-admission/run5](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-timer-early-admission/run5)
- Result:
  - in this five-run sample, the canonical AHCI timeout branch did **not**
    reproduce
  - but the wider timer-body admission window was not clean:
    - `run1` and `run4` emitted repeated `DEFER_EVICT` with
      `evicted=18446462599808842688`
    - that value resolves to `0xffff000040227bc0`, i.e.
      `kernel::process::PROCESS_MANAGER`, not a tid
    - `run4` then faulted with `DATA_ABORT ... ELR=0x0`
    - `run5` ended with `UNHANDLED_EC cpu=0 EC=0x0 ELR=0xffff0000542426c8`
  - `run2` was the cleanest sample of the batch
  - `run3` did not hit the old timeout branch within the collector window but
    also did not establish a stronger success marker than "boot progressed"
- Interpretation:
  - earlier interrupt admission during the timer corridor appears to be a
    meaningful lever against the AHCI timeout branch
  - but the *wide* admission window allowed nested IRQs to execute through too
    much unrelated timer bookkeeping and exposed a different corruption path
  - so the honest next step is not "ship this"
  - it is "tighten the admission window and see whether the timeout benefit
    survives without the new deferred-requeue corruption"

### 2026-04-04: Follow-Up Probe Narrows Timer Admission To The Immediate Post-Rearm Block

- Probe refinement:
  - keep the same basic hypothesis
  - reduce the open-IRQ region to the immediate post-rearm register snapshot
    block only
  - re-mask before:
    - wall-clock update
    - input polling
    - soft-lockup logic
    - scheduler-facing timer bookkeeping
- Why this refinement is justified:
  - it preserves the strongest new signal from the prior probe:
    earlier admission seems relevant
  - it removes the part most likely to create unrelated nested-execution
    corruption:
    broad timer-body reentrancy

### 2026-04-04: Fatal Exception Postmortems Now Dump Deferred-Requeue And Trace State

- Probe change:
  - add a one-shot per-CPU fatal postmortem helper in
    [exception.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/exception.rs)
  - wire it into kernel-side:
    - `INSTRUCTION_ABORT`
    - `DATA_ABORT`
    - `UNHANDLED_EC`
- Dump contents:
  - deferred-requeue snapshots
  - last user-context-write snapshots
  - trace buffers
- Why this was necessary:
  - the narrowed timer-admission probe had exposed early
    `DEFER_EVICT` / `UNHANDLED_EC` branches, but those fatal paths were still
    printing only the headline and then redirecting to idle
  - soft-lockup dumps already had the right ownership evidence; early fatal
    runs did not
- Guardrail:
  - the dump is one-shot per CPU so repeated exception storms do not flood the
    serial stream and destroy the artifact

### 2026-04-04: Five Short-Window Reruns Did Not Reproduce The Earlier Early-Fatal Branch

- Probe artifacts:
  - [post-deploy-narrow-admission-fatal-postmortem/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-narrow-admission-fatal-postmortem/run1)
  - [post-deploy-narrow-admission-fatal-postmortem/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-narrow-admission-fatal-postmortem/run2)
  - [post-deploy-narrow-admission-fatal-postmortem/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-narrow-admission-fatal-postmortem/run3)
  - [post-deploy-narrow-admission-fatal-postmortem/run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-narrow-admission-fatal-postmortem/run4)
  - [post-deploy-narrow-admission-fatal-postmortem/run5](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-narrow-admission-fatal-postmortem/run5)
- Fixed-state image:
  - freshly rebuilt and redeployed via `./run.sh --parallels --test 20`
  - VM: `breenix-1775292409`
- Result:
  - `run1`, `run2`, `run4`, and `run5` all reached `[init] bsshd started (PID 2)`
    without:
    - `DEFER_EVICT`
    - `UNHANDLED_EC`
    - `DATA_ABORT`
    - `FATAL_POSTMORTEM`
    - soft lockup
  - `run3` reproduced the plain AHCI timeout corridor instead:
    - `[ahci] read_block(491526) wait failed: AHCI: command timeout`
- Stable timeout-side state in `run3`:
  - `SPI34 pend=true act=false`
  - `active_cmd_num=1189`
  - `last_complete=1188`
  - `completion_done=0`
  - `waiter_tid=0` at timeout snapshot, after the prior-token completion/wake
  - `timeout_cpu=5`
  - `cpu0_breadcrumb=107`
- Interpretation:
  - the new fatal-postmortem hook did **not** itself perturb the system back
    into the earlier early-fatal branch in this five-run sample
  - but the surviving bad run is still the familiar AHCI timeout corridor, not
    a solved system

### 2026-04-04: Longer Dwell Shows The Current Narrowed-Admission Branch Still Times Out

- Probe artifacts:
  - [post-deploy-narrow-admission-fatal-postmortem-long60/run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-narrow-admission-fatal-postmortem-long60/run1)
  - [post-deploy-narrow-admission-fatal-postmortem-long60/run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-narrow-admission-fatal-postmortem-long60/run2)
  - [post-deploy-narrow-admission-fatal-postmortem-long60/run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/post-deploy-narrow-admission-fatal-postmortem-long60/run3)
- Method:
  - same exact VM image: `breenix-1775292409`
  - same kernel state as the short-window sweep
  - collector dwell increased from `25s` to `60s`
- Result:
  - `run1` timed out later:
    - `[ahci] read_block(497224) wait failed: AHCI: command timeout`
    - then `[init] Boot script completed`
  - `run2` stayed clean within the 60-second collector window
  - `run3` timed out later:
    - `[ahci] read_block(491526) wait failed: AHCI: command timeout`
  - none of the three long runs reproduced:
    - `DEFER_EVICT`
    - `UNHANDLED_EC`
    - `DATA_ABORT`
    - soft lockup
- Stable timeout-side state across the two long-run failures:
  - `SPI34 pend=true act=false`
  - `completion_done=0`
  - `active_cmd_num=current_token`
  - `last_complete=previous_token`
  - `isr_last_hw_cpu=0`
  - CPU 0 still reports breadcrumb `107`
  - timeout CPU is not fixed:
    - `run1`: `timeout_cpu=2`
    - `run3`: timeout side activity concentrated on `CPU6`
- Interpretation:
  - the narrowed timer-admission probe is buying something against the earlier
    early-corruption branch
  - but it is **not** eliminating the dominant long-run AHCI completion timeout
    corridor
  - short collector windows were therefore overstating stability

### 2026-04-04: Branch Disposition After The Short- And Long-Window Sweeps

- Closed branches:
  - "the fatal-postmortem hook itself reintroduced the early-fatal branch"
    is not supported by today’s reruns
  - "the narrowed timer-admission probe made the system genuinely stable"
    is falsified by the 60-second sweep
- Still-open branch:
  - Linux admits follow-on IRQ work earlier in the irqchip/generic IRQ path,
    while Breenix still relies primarily on:
    - the timer-local admission probe
    - later resched / exception-return windows
- Next target:
  - move from the timer-specific admission experiment toward a generic ARM64
    IRQ-admission probe in `handle_irq()` / GIC-facing logic, using Linux’s
    `gic_handle_irq()` semantics as the architectural reference

### 2026-04-15: P1 Generic Post-EOI Admission Sweep Did Not Reach 5/5

- Probe change summary:
  - added a bounded generic post-EOI admission seam in
    [exception.rs](/Users/wrb/fun/code/breenix-worktrees/p1-generic-post-eoi-admission/kernel/src/arch_impl/aarch64/exception.rs)
    immediately after `gic::end_of_interrupt()` and before inline
    `do_softirq()` / the resched tail
  - gated the seam from the saved interrupt-frame `spsr` using the same
    `(spsr & 0xC0) == 0` rule as the narrowed timer-local admission probe
  - kept the narrowed timer-body window in
    [timer_interrupt.rs](/Users/wrb/fun/code/breenix-worktrees/p1-generic-post-eoi-admission/kernel/src/arch_impl/aarch64/timer_interrupt.rs)
    unchanged as baseline
- Artifacts:
  - [run1](/Users/wrb/fun/code/breenix-worktrees/p1-generic-post-eoi-admission/logs/breenix-parallels-cpu0/p1-generic-admission/run1)
  - [run2](/Users/wrb/fun/code/breenix-worktrees/p1-generic-post-eoi-admission/logs/breenix-parallels-cpu0/p1-generic-admission/run2)
  - [run3](/Users/wrb/fun/code/breenix-worktrees/p1-generic-post-eoi-admission/logs/breenix-parallels-cpu0/p1-generic-admission/run3)
  - [run4](/Users/wrb/fun/code/breenix-worktrees/p1-generic-post-eoi-admission/logs/breenix-parallels-cpu0/p1-generic-admission/run4)
  - [run5](/Users/wrb/fun/code/breenix-worktrees/p1-generic-post-eoi-admission/logs/breenix-parallels-cpu0/p1-generic-admission/run5)
- Result:
  - `run1`: FAIL — reached `[init] bsshd started (PID 2)` and later hit
    `[ahci] read_block(172796) wait failed: AHCI: command timeout`
  - `run2`: FAIL — no AHCI timeout, but hit a kernel `DATA_ABORT` before
    `bsshd` came up
  - `run3`: FAIL — reached `[init] bsshd started (PID 2)` and later hit
    `[ahci] read_block(464828) wait failed: AHCI: command timeout`
  - `run4`: PASS — reached `[init] bsshd started (PID 2)` with no AHCI timeout
    and no corruption markers in the 60-second collector window
  - `run5`: FAIL — reached `[init] bsshd started (PID 2)` and later hit
    `[ahci] read_block(299030) wait failed: AHCI: command timeout`
- Stable state from failing runs:
  - dominant timeout branch (`run1`, `run3`, `run5`):
    - `[init] bsshd started (PID 2)` is present
    - later AHCI completion wait still times out in the long dwell window
    - no `DATA_ABORT`, `UNHANDLED_EC`, `FATAL_POSTMORTEM`, or `DEFER_EVICT`
      accompanied those three failures
  - secondary corruption branch (`run2`):
    - `[DATA_ABORT] FAR=0xffff00000200000c ELR=0xffff0000401822f0`
    - `ESR=0x96000010 DFSC=0x10 TTBR0=0x140016000 from_el0=0 cpu=0`
    - `bsshd` never started in that run
- Interpretation:
  - D1 is falsified as a sufficient next cut: adding the generic post-EOI seam
    did **not** collapse the sweep to `5/5`
  - the dominant surviving bad branch is still the AHCI timeout corridor
    (`3/5` runs), so moving generic DAIF admission earlier is not enough by
    itself
  - one run (`run2`) also shifted into corruption, which keeps the PMR-gating
    risk from D3 alive, but that was not the dominant outcome of the sweep
- Next target:
  - D2 should move to rank 1 for F5
  - reason: the dominant failure signature after P1 is still the unchanged
    AHCI timeout corridor, which points next at late priority drop / EOImode0
    semantics rather than at admission timing alone

### 2026-04-15: P2 EOImode1 Split Priority Drop Sweep Did Not Reach 5/5

- Probe change summary:
  - set `ICC_CTLR_EL1.EOImode` bit 1 to `1` in
    [gic.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/gic.rs)
    so the shared GICv3 CPU-interface init path uses split priority-drop /
    deactivate semantics on both primary and secondary CPUs
  - split the old combined EOI helper into `priority_drop_irq()` and
    `deactivate_irq()` in
    [gic.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/gic.rs)
    while keeping `end_of_interrupt()` as a wrapper for any future combined
    path
  - updated
    [exception.rs](/Users/wrb/fun/code/breenix/kernel/src/arch_impl/aarch64/exception.rs)
    so `handle_irq()` now does:
    `acknowledge_irq() -> priority_drop_irq() -> dispatch -> deactivate_irq()`
    without changing the timer-body admission window, PMR policy, or the
    resched / return tail
- Artifacts:
  - [run1](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/p2-eoimode1-split/run1)
  - [run2](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/p2-eoimode1-split/run2)
  - [run3](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/p2-eoimode1-split/run3)
  - [run4](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/p2-eoimode1-split/run4)
  - [run5](/Users/wrb/fun/code/breenix/logs/breenix-parallels-cpu0/p2-eoimode1-split/run5)
- Result:
  - `run1`: PASS — reached `[init] bsshd started (PID 2)` with no AHCI
    timeout and no corruption markers in the 60-second collector window
  - `run2`: FAIL — hit
    `[ahci] read_block(65542) wait failed: AHCI: command timeout` before
    `bsshd` came up
  - `run3`: FAIL — reached `[init] bsshd started (PID 2)` and later hit
    `[ahci] read_block(65542) wait failed: AHCI: command timeout`
  - `run4`: FAIL — hit
    `[ahci] read_block(196614) wait failed: AHCI: command timeout` before
    `bsshd` came up
  - `run5`: PASS — reached `[init] bsshd started (PID 2)` with no AHCI
    timeout and no corruption markers in the 60-second collector window
- Stable state from failing runs:
  - all failing samples stayed on the AHCI timeout branch; none showed
    `DATA_ABORT`, `UNHANDLED_EC`, `FATAL_POSTMORTEM`, or `DEFER_EVICT`
  - the timeout signature remained the same completion-wait failure path:
    `[ahci] read_block(...) wait failed: AHCI: command timeout`
  - two failing runs (`run2`, `run4`) never reached `bsshd`, while one
    (`run3`) did; the sweep therefore still shows the timeout corridor both
    before and after init completes
- Interpretation:
  - D2 is falsified as a sufficient next cut: moving Breenix to EOImode1-style
    priority drop before dispatch did **not** collapse the sweep to `5/5`
  - unlike the P1 sweep, P2 did not shift any sample into a new corruption
    branch; the dominant surviving problem is still the existing AHCI timeout
    corridor
  - that leaves the strongest remaining explanation at D3: early priority drop
    alone is still not Linux-like while PMR remains a static `0xff` gate
- Next target:
  - P3 should move to rank 1 for F6
  - reason: the timeout corridor persisted without new corruption, so the next
    falsifiable cut is to add a live PMR admission gate rather than changing
    the return path

### 2026-04-15: F7 Combined Linux Admission Sweep Improved Stability but Did Not Reach 5/5

- Probe change summary:
  - kept the GICv3 CPU-interface in `EOImode1` and retained the split
    `priority_drop_irq()` / `deactivate_irq()` helpers in
    [gic.rs](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/kernel/src/arch_impl/aarch64/gic.rs)
  - updated
    [exception.rs](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/kernel/src/arch_impl/aarch64/exception.rs)
    so the generic IRQ path now follows the Linux ordering:
    `acknowledge_irq() -> irq_enter() -> priority_drop_irq() -> isb ->
    guarded daifclr -> dispatch -> guarded daifset -> deactivate_irq() ->
    irq_exit() -> do_softirq() -> resched`
  - removed the bespoke context-switch admission/rearm scaffolding from
    [context_switch.rs](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/kernel/src/arch_impl/aarch64/context_switch.rs)
    that F7 replaces, and dropped the timer-local extra HARDIRQ accounting from
    [timer_interrupt.rs](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/kernel/src/arch_impl/aarch64/timer_interrupt.rs)
    so generic `handle_irq()` owns the accounting window
  - kept the F6-style timeout telemetry by adding
    `dump_stuck_state_for_spi(34)` to the AHCI timeout path in
    [ahci/mod.rs](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/kernel/src/drivers/ahci/mod.rs)
- Artifacts:
  - [run1](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/logs/breenix-parallels-cpu0/f7-combined-admission/run1)
  - [run2](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/logs/breenix-parallels-cpu0/f7-combined-admission/run2)
  - [run3](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/logs/breenix-parallels-cpu0/f7-combined-admission/run3)
  - [run4](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/logs/breenix-parallels-cpu0/f7-combined-admission/run4)
  - [run5](/Users/wrb/fun/code/breenix-worktrees/f7-combined-linux-admission/logs/breenix-parallels-cpu0/f7-combined-admission/run5)
- Result:
  - `run1`: FAIL — reached `[init] bsshd started (PID 2)` and later hit two
    AHCI completion timeouts (`ahci_timeouts=2`, `stuck_state_dumps=32`)
  - `run2`: FAIL — reached `[init] bsshd started (PID 2)` and later hit one
    AHCI completion timeout (`ahci_timeouts=1`, `stuck_state_dumps=16`)
  - `run3`: FAIL — reached `[init] bsshd started (PID 2)` and later hit two
    AHCI completion timeouts (`ahci_timeouts=2`, `stuck_state_dumps=32`)
  - `run4`: PASS — reached `[init] bsshd started (PID 2)` with no AHCI
    timeout and no corruption markers in the 60-second collector window
  - `run5`: PASS — reached `[init] bsshd started (PID 2)` with no AHCI
    timeout and no corruption markers in the 60-second collector window
- Stable state from failing runs:
  - all failing samples stayed on the timeout branch; none showed
    `DATA_ABORT`, `UNHANDLED_EC`, `FATAL_POSTMORTEM`, or `DEFER_EVICT`
  - unlike the earlier `pend=true act=false` admission-blocked signature, the
    F7 stuck-state dumps consistently showed `SPI34 pend=true act=true` with
    `ICC_RPR_EL1=0xff`, `ICC_PMR_EL1=0xf8`, `ICC_HPPIR1_EL1=0x3ff`, and
    `DAIF=0x300` outside exception context at timeout time
  - all five runs reached `[init] bsshd started (PID 2)`, so the remaining
    failures happen after init is already live rather than on a pre-init
    corruption branch
- Interpretation:
  - the complete Linux-style admission model is **not** sufficient by itself:
    F7 improved stability to `2/5` clean runs and avoided the corruption branch,
    but it still missed the `5/5` pass criterion
  - the timeout branch remains dominant, so the investigation stays on the
    timeout side rather than moving to D4/return-discipline corruption work
  - the new `act=true` stuck-state signature weakens the original
    "same-priority admission never opens" explanation; by timeout time the AHCI
    SPI is already active, which points more toward post-admission routing /
    level-deassert / completion-state handling than to the old DAIF+EOI gate
- Next target:
  - follow the timeout branch, not the corruption branch
  - next probe should treat this as a D3/routing-style follow-up: verify why
    SPI34 stays `pending+active` after the combined admission window already
    admitted it, rather than spending another cycle on return-path cleanup

### 2026-04-16: F8 AHCI Completion Diagnostic

- Probe change summary:
  - added a fixed-size, per-CPU AHCI completion trace ring in
    [ahci/mod.rs](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/kernel/src/drivers/ahci/mod.rs)
    with `ENTER`, `POST_CLEAR`, and `RETURN` records for port index, `IS`,
    `CI`, `SACT`, `SERR`, waiter TID, slot mask, token, wake result, timestamp,
    and CPU id
  - extended
    [gic.rs](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/kernel/src/arch_impl/aarch64/gic.rs)
    so `dump_stuck_state_for_spi(34)` emits
    `[STUCK_SPI34] AHCI_PORT0_IS=<value>` and flushes recent `[AHCI_RING]`
    records
  - kept the F7 `handle_irq()` ordering unchanged:
    `acknowledge_irq() -> irq_enter() -> priority_drop_irq() -> dispatch ->
    deactivate_irq() -> irq_exit()`
- Artifacts:
  - [run1](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/logs/breenix-parallels-cpu0/f8-ahci-diag/run1)
  - [run2](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/logs/breenix-parallels-cpu0/f8-ahci-diag/run2)
  - [run3](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/logs/breenix-parallels-cpu0/f8-ahci-diag/run3)
  - [run4](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/logs/breenix-parallels-cpu0/f8-ahci-diag/run4)
  - [run5](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/logs/breenix-parallels-cpu0/f8-ahci-diag/run5)
- Result:
  - `run1`: PASS-like, reached `[init] bsshd started (PID 2)` with no AHCI
    timeout in the 60-second window; run command bookkeeping was interrupted
    by the first sweep-loop shell variable bug, but serial output was captured
  - `run2`: FAIL, `ahci_timeouts=2`, `ahci_ring_entries=64`,
    `ahci_port0_is=2`, reached `bsshd`
  - `run3`: FAIL, `ahci_timeouts=2`, `ahci_ring_entries=64`,
    `ahci_port0_is=2`, reached `bsshd`
  - `run4`: FAIL, `ahci_timeouts=2`, `ahci_ring_entries=64`,
    `ahci_port0_is=2`, reached `bsshd`
  - `run5`: FAIL, `ahci_timeouts=2`, `ahci_ring_entries=64`,
    `ahci_port0_is=2`, reached `bsshd`
- Verbatim extract from `run2`:

```text
[ahci] read_block(72711) wait failed: AHCI: command timeout
[STUCK_SPI34] cpu=7 gic_version=3
[STUCK_SPI34] GICD_ISPENDR[1]=0x800004 bit=2 pending=true
[STUCK_SPI34] GICD_ISACTIVER[1]=0x4 bit=2 active=true
[STUCK_SPI34] GICD_ICFGR[2]=0x0 bits=0x0 trigger=level
[STUCK_SPI34] ICC_RPR_EL1=0xff
[STUCK_SPI34] ICC_PMR_EL1=0xf8
[STUCK_SPI34] DAIF=0x300
[STUCK_SPI34] AHCI_PORT0_IS=0x0
[AHCI_RING] nsec=2348570958 site=POST_CLEAR cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1231 wake_success=0 seq=3686
[AHCI_RING] nsec=2348563291 site=ENTER cpu=0 port=1 IS=0x1 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1231 wake_success=0 seq=3685
[AHCI_RING] nsec=2345107041 site=RETURN cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1230 wake_success=1 seq=3684
```

- Additional timeout state from the same run:

```text
[ahci] Port 1 TIMEOUT (5s): CI=0x0 IS=0x1 TFD=0x40 HBA_IS=0x2
[ahci]   GIC: SPI34 pend=true act=true DAIF=0x300 pend_snap=[0x800004,0x0,0x0]
[ahci]   isr_count=1229 cmd#=1232 completion_done=0 PMR=0xf8 RPR=0xff
[ahci]   port_isr_hits=[1,1228] complete_hits=[0,22]
[ahci]   isr_last_hw_cpu=0
```

- Hypothesis ranking:
  - H5 strongest as the observed GIC-level symptom: SPI34 remains active
    because the admitted AHCI dispatch did not reach the handler `RETURN`
    record for the newest serviced token, so `handle_irq()` cannot yet execute
    the F7 `deactivate_irq()` write for that interrupt
  - H3 also strongly supported at the AHCI line level: the handler observed
    port 1 `IS=0x1`, recorded `POST_CLEAR` with `IS=0x0`, and timeout state
    later shows port 1 `IS=0x1`; the level line reasserted after the clear
  - H2 is weak: for every recorded serviced completion, `POST_CLEAR` reports
    `IS=0x0`, so the handler is clearing the `IS` bits it observes
  - H4 is weak: the ring consistently records port 1, matching the ext2 AHCI
    device, while the new `AHCI_PORT0_IS` field is `0x0`
  - H1 is falsified: `[AHCI_RING] ENTER` and `POST_CLEAR` records prove SPI34
    dispatch reaches AHCI code
- Interpretation:
  - F8 narrows the failure from "AHCI interrupt not admitted" to "AHCI
    completion handler does not complete the newest wake path before the next
    level assertion waits behind an already-active SPI"
  - the missing `RETURN` for the latest token is the key new signal; the last
    complete `ENTER/POST_CLEAR/RETURN` triplet is for the previous token, while
    the newest token reaches `POST_CLEAR` but not `RETURN`
- F9 recommendation:
  - probe name: `f9-ahci-complete-return-boundary`
  - touch:
    [ahci/mod.rs](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/kernel/src/drivers/ahci/mod.rs),
    [completion.rs](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/kernel/src/task/completion.rs),
    and optionally
    [exception.rs](/Users/wrb/fun/code/breenix-worktrees/f8-ahci-completion/kernel/src/arch_impl/aarch64/exception.rs)
    for a non-hot-path stuck-state counter showing last `deactivate_irq(34)`
  - add AHCI ring sites immediately before and after `Completion::complete()`,
    plus a minimal atomic breadcrumb at `isr_unblock_for_io()` entry/exit
  - expected signature change: if `BEFORE_COMPLETE` appears without
    `AFTER_COMPLETE`, F9 should move to the completion/scheduler wake path; if
    `AFTER_COMPLETE` and AHCI `RETURN` appear but SPI34 still dumps
    `active=true`, F9 should become a narrow F7 DIR/deactivation audit

### 2026-04-16: F9 Completion Boundary Diagnostic

- Probe change summary:
  - branched from `diagnostic/f8-ahci-completion` to
    `diagnostic/f9-completion-boundary`
  - extended the existing AHCI trace ring in
    [ahci/mod.rs](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/kernel/src/drivers/ahci/mod.rs)
    with `BEFORE_COMPLETE`, `AFTER_COMPLETE`, `WAKE_ENTER`, and `WAKE_EXIT`
    site tags
  - wrapped the existing `AHCI_COMPLETIONS[port][0].complete(cmd_num)` call
    with `BEFORE_COMPLETE` / `AFTER_COMPLETE`
  - wrapped the existing `Completion::complete()` call to
    `scheduler::isr_unblock_for_io(tid)` with `WAKE_ENTER` / `WAKE_EXIT`
  - extended the SPI34 stuck-state dump to emit both
    `[STUCK_SPI34] AHCI_PORT0_IS=<value>` and
    `[STUCK_SPI34] AHCI_PORT1_IS=<value>`
- Validation:
  - clean aarch64 build:
    `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
  - `grep -E '^(warning|error)' /tmp/f9-aarch64-build.log` produced no output
  - `git diff --check` passed
- Artifacts:
  - [run1](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f9-completion-boundary/run1)
  - [run2](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f9-completion-boundary/run2)
  - [run3](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f9-completion-boundary/run3)
  - [run4](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f9-completion-boundary/run4)
  - [run5](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f9-completion-boundary/run5)
- Result:
  - `run1`: FAIL, soft-lockup branch before `bsshd`; no AHCI timeout and no
    AHCI ring dump in the retained serial tail
  - `run2`: FAIL, reached `bsshd`, then hit one AHCI completion timeout with
    `ahci_ring_entries=32`, `ahci_port0_is=1`, `ahci_port1_is=1`,
    `before_complete=7`, `after_complete=5`, `wake_enter=3`, `wake_exit=2`
  - `run3`: PASS-like within the 60-second collector window, reached `bsshd`
    with no AHCI timeout and no AHCI ring dump
  - `run4`: PASS-like within the 60-second collector window, reached `bsshd`
    with no AHCI timeout and no AHCI ring dump
  - `run5`: PASS-like within the 60-second collector window, reached `bsshd`
    with no AHCI timeout and no AHCI ring dump
  - all `run.sh` invocations returned `exit_status=1` because the headless
    screenshot helper could not complete; compile output stayed warning-free
- Run 2 summaries:

```text
exit_status=1
ahci_timeouts=1
ahci_ring_entries=32
ahci_port0_is=1
ahci_port1_is=1
bsshd_started=1
before_complete=7
after_complete=5
wake_enter=3
wake_exit=2
```

- Verbatim extract from `run2`:

```text
[ahci] Port 1 TIMEOUT (5s): CI=0x0 IS=0x1 TFD=0x40 HBA_IS=0x2
[ahci]   GIC: SPI34 pend=true act=true DAIF=0x300 pend_snap=[0x800004,0x0,0x0]
[ahci]   isr_count=1217 cmd#=1221 completion_done=0 PMR=0xf8 RPR=0xff
[ahci]   port_isr_hits=[1,1216] complete_hits=[0,10]
[ahci] read_block(522) wait failed: AHCI: command timeout
[STUCK_SPI34] cpu=6 gic_version=3
[STUCK_SPI34] GICD_ISPENDR[1]=0x800004 bit=2 pending=true
[STUCK_SPI34] GICD_ISACTIVER[1]=0x4 bit=2 active=true
[STUCK_SPI34] ICC_RPR_EL1=0xff
[STUCK_SPI34] ICC_PMR_EL1=0xf8
[STUCK_SPI34] DAIF=0x300
[STUCK_SPI34] AHCI_PORT0_IS=0x0
[STUCK_SPI34] AHCI_PORT1_IS=0x1
[AHCI_RING] nsec=2366781458 site=WAKE_ENTER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1219 wake_success=0 seq=3690
[AHCI_RING] nsec=2366780583 site=BEFORE_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1219 wake_success=0 seq=3689
[AHCI_RING] nsec=2366779666 site=POST_CLEAR cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1219 wake_success=0 seq=3688
[AHCI_RING] nsec=2366771250 site=ENTER cpu=0 port=1 IS=0x1 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1219 wake_success=0 seq=3687
[AHCI_RING] nsec=2364838458 site=RETURN cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1218 wake_success=1 seq=3686
[AHCI_RING] nsec=2364837541 site=AFTER_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1218 wake_success=0 seq=3685
[AHCI_RING] nsec=2364836541 site=WAKE_EXIT cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1218 wake_success=0 seq=3684
[AHCI_RING] nsec=2364808208 site=WAKE_ENTER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1218 wake_success=0 seq=3683
[AHCI_RING] nsec=2364807250 site=BEFORE_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1218 wake_success=0 seq=3682
```

- Case verdict:
  - **Case A by the top-level F9 decision rule**: the stuck token (`1219`)
    has `BEFORE_COMPLETE` without `AFTER_COMPLETE`, so the handler is stuck
    inside `Completion::complete()`
  - the new `WAKE_ENTER` without `WAKE_EXIT` for the same token further
    narrows the in-`complete()` stall to the scheduler wake helper,
    `scheduler::isr_unblock_for_io(tid)`, rather than to the initial
    `done.store`, fence, `sev`, or waiter load
  - the previous token (`1218`) completed the full
    `BEFORE_COMPLETE -> WAKE_ENTER -> WAKE_EXIT -> AFTER_COMPLETE -> RETURN`
    sequence, so the new probe is discriminating the stalled token rather than
    globally breaking the path
- F10 recommendation:
  - probe name: `f10-isr-unblock-boundary`
  - add ring sites inside
    [scheduler.rs](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/kernel/src/task/scheduler.rs)
    `isr_unblock_for_io(tid)` at:
    entry after `current_cpu_id_raw()`, after `ISR_WAKEUP_BUFFERS[cpu].push(tid)`,
    after `set_need_resched()`, before the idle-CPU SGI loop, before each
    `send_sgi()`, after each `send_sgi()`, and final exit
  - keep the probe atomic/ring-only; do not add logging to interrupt or
    syscall hot paths
  - likely F10 split: if the last site is before/inside `send_sgi()`, audit
    SGI delivery/GICR targeting; if it is before `set_need_resched()` or
    buffer push, audit the per-CPU wake buffer atomics

### 2026-04-16: F10 `isr_unblock_for_io()` Internal Breadcrumb Diagnostic

- Probe change summary:
  - branched from `diagnostic/f9-completion-boundary` to
    `diagnostic/f10-isr-unblock-boundary`
  - extended the existing AHCI trace ring in
    [ahci/mod.rs](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/kernel/src/drivers/ahci/mod.rs)
    with `UNBLOCK_ENTRY`, `UNBLOCK_AFTER_CPU`, `UNBLOCK_AFTER_BUFFER`,
    `UNBLOCK_AFTER_NEED_RESCHED`, `UNBLOCK_BEFORE_SGI_SCAN`,
    `UNBLOCK_PER_SGI`, and `UNBLOCK_EXIT`
  - instrumented
    [scheduler.rs](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/kernel/src/task/scheduler.rs)
    `isr_unblock_for_io(tid)` with ring-only breadcrumbs at the requested
    internal boundaries
  - encoded `UNBLOCK_PER_SGI` target CPU in the existing `slot_mask` field;
    non-SGI `UNBLOCK_*` entries leave `slot_mask=0`
- Validation:
  - clean aarch64 build:
    `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
  - `grep -E '^(warning|error)' /tmp/f10-aarch64-build.log` produced no output
  - `git diff --check` passed
  - repo-wide `cargo fmt --check` still fails on pre-existing unrelated
    formatting/trailing-whitespace issues; no broad formatter was applied
- Artifacts:
  - [run1](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f10-isr-unblock/run1)
  - [run2](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f10-isr-unblock/run2)
  - [run3](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f10-isr-unblock/run3)
  - [run4](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f10-isr-unblock/run4)
  - [run5](/Users/wrb/fun/code/breenix-worktrees/f9-completion-boundary/logs/breenix-parallels-cpu0/f10-isr-unblock/run5)
- Sweep summaries:

```text
run1:
exit_status=1
ahci_timeouts=14
ahci_ring_entries=64
ahci_port0_is=2
ahci_port1_is=2
bsshd_started=1
before_complete=10
after_complete=8
wake_enter=2
wake_exit=2
unblock_entry=2
unblock_after_cpu=2
unblock_after_buffer=2
unblock_after_need_resched=2
unblock_before_sgi_scan=2
unblock_per_sgi=6
unblock_exit=2

run2:
exit_status=1
ahci_timeouts=74
ahci_ring_entries=64
ahci_port0_is=2
ahci_port1_is=2
bsshd_started=1
before_complete=10
after_complete=8
wake_enter=2
wake_exit=2
unblock_entry=2
unblock_after_cpu=2
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=12
unblock_exit=2

run3:
exit_status=1
ahci_timeouts=1227
ahci_ring_entries=31
ahci_port0_is=1
ahci_port1_is=1
bsshd_started=1
before_complete=4
after_complete=4
wake_enter=1
wake_exit=1
unblock_entry=1
unblock_after_cpu=1
unblock_after_buffer=1
unblock_after_need_resched=1
unblock_before_sgi_scan=1
unblock_per_sgi=3
unblock_exit=1

run4:
exit_status=1
ahci_timeouts=0
ahci_ring_entries=0
ahci_port0_is=0
ahci_port1_is=0
bsshd_started=1
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0

run5:
exit_status=1
ahci_timeouts=670
ahci_ring_entries=64
ahci_port0_is=2
ahci_port1_is=2
bsshd_started=1
before_complete=6
after_complete=8
wake_enter=2
wake_exit=2
unblock_entry=2
unblock_after_cpu=2
unblock_after_buffer=2
unblock_after_need_resched=2
unblock_before_sgi_scan=2
unblock_per_sgi=12
unblock_exit=2
```

- Primary stalled-token extract from `run1`:

```text
[ahci] Port 1 TIMEOUT (5s): CI=0x0 IS=0x1 TFD=0x40 HBA_IS=0x2
[ahci]   GIC: SPI34 pend=true act=true DAIF=0x300 pend_snap=[0x800004,0x0,0x0]
[ahci]   isr_count=1210 cmd#=1214 completion_done=0 PMR=0xf8 RPR=0xff
[ahci]   port_isr_hits=[1,1209] complete_hits=[0,3]
[ahci]   isr_last_pmr=0xf8 last_port1_IS=0x1 last_port1_cmd_num=1212
[ahci] read_block(522) wait failed: AHCI: command timeout
[STUCK_SPI34] cpu=3 gic_version=3
[STUCK_SPI34] GICD_ISPENDR[1]=0x800004 bit=2 pending=true
[STUCK_SPI34] GICD_ISACTIVER[1]=0x4 bit=2 active=true
[STUCK_SPI34] ICC_RPR_EL1=0xff
[STUCK_SPI34] ICC_PMR_EL1=0xf8
[STUCK_SPI34] DAIF=0x300
[STUCK_SPI34] AHCI_PORT0_IS=0x0
[STUCK_SPI34] AHCI_PORT1_IS=0x1
[AHCI_RING] nsec=2379603791 site=UNBLOCK_PER_SGI cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x2 token=0 wake_success=0 seq=3677
[AHCI_RING] nsec=2379602916 site=UNBLOCK_BEFORE_SGI_SCAN cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3676
[AHCI_RING] nsec=2379602041 site=UNBLOCK_AFTER_NEED_RESCHED cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3675
[AHCI_RING] nsec=2379601166 site=UNBLOCK_AFTER_BUFFER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3674
[AHCI_RING] nsec=2379600291 site=UNBLOCK_AFTER_CPU cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3673
[AHCI_RING] nsec=2379599416 site=UNBLOCK_ENTRY cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3672
[AHCI_RING] nsec=2379598541 site=WAKE_ENTER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1212 wake_success=0 seq=3671
[AHCI_RING] nsec=2379597666 site=BEFORE_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1212 wake_success=0 seq=3670
[AHCI_RING] nsec=2379596750 site=POST_CLEAR cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1212 wake_success=0 seq=3669
[AHCI_RING] nsec=2379589250 site=ENTER cpu=0 port=1 IS=0x1 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1212 wake_success=0 seq=3668
[AHCI_RING] nsec=2378591666 site=RETURN cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1211 wake_success=1 seq=3667
[AHCI_RING] nsec=2378590416 site=AFTER_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1211 wake_success=0 seq=3666
[AHCI_RING] nsec=2378589375 site=WAKE_EXIT cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1211 wake_success=0 seq=3665
[AHCI_RING] nsec=2378588333 site=UNBLOCK_EXIT cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=3664
```

- Last-site verdict:
  - primary sample `run1`: stuck token `1212`, waiter `tid=11`, reached
    `UNBLOCK_PER_SGI` with `slot_mask=0x2` (target CPU 2) and never emitted
    another `UNBLOCK_PER_SGI`, `UNBLOCK_EXIT`, `WAKE_EXIT`, `AFTER_COMPLETE`,
    or `RETURN` for that token
  - corroborating sample `run3`: stuck token `1396`, waiter `tid=13`, reached
    `UNBLOCK_PER_SGI` with `slot_mask=0x1` (target CPU 1) and did not emit
    `UNBLOCK_EXIT` / `WAKE_EXIT` for that token
  - alternate sample `run2`: stuck token `1263`, waiter `tid=13`, reached
    `UNBLOCK_AFTER_CPU` and did not emit `UNBLOCK_AFTER_BUFFER`; this keeps the
    wake-buffer push path on the candidate list, but it was not the majority
    signature in this sweep
- Candidate root-cause ranking:
  1. SGI delivery path: strongest current lead. Two timeout captures stop after
     `UNBLOCK_PER_SGI`, which is emitted immediately before
     `gic::send_sgi(SGI_RESCHEDULE, target)`. The target CPU is visible in
     `slot_mask` (`0x2` in `run1`, `0x1` in `run3`).
  2. Wake-buffer push path: one timeout capture stops after
     `UNBLOCK_AFTER_CPU`, before `UNBLOCK_AFTER_BUFFER`, implying a possible
     stall inside `ISR_WAKEUP_BUFFERS[cpu].push(tid)` or an adjacent memory
     corruption/ring-retention artifact.
  3. `set_need_resched()` / SGI scan predicate: lower probability. The primary
     samples pass `UNBLOCK_AFTER_NEED_RESCHED` and
     `UNBLOCK_BEFORE_SGI_SCAN` before stalling.
- F11 recommendation:
  - primary F11 probe direction: audit SGI delivery by instrumenting
    `gic::send_sgi()` / SGI target construction with ring-only breadcrumbs
    before and after each architectural write, preserving the target CPU in the
    ring entry
  - secondary F11 guardrail: keep one minimal breadcrumb around the
    `IsrWakeupBuffer::push()` CAS loop or add a bounded push-site counter so the
    `run2` `UNBLOCK_AFTER_CPU` signature can be confirmed or eliminated without
    changing scheduler semantics

### 2026-04-16: F11 `send_sgi()` and Wake-Buffer Push Boundary Diagnostic

- Branch: `diagnostic/f11-send-sgi-boundary`
- Code changes:
  - added AHCI ring site tags `SGI_ENTRY`, `SGI_AFTER_MPIDR`,
    `SGI_AFTER_COMPOSE`, `SGI_BEFORE_MSR`, `SGI_AFTER_MSR`,
    `SGI_AFTER_ISB`, `SGI_EXIT`, `WAKEBUF_BEFORE_PUSH`, and
    `WAKEBUF_AFTER_PUSH`
  - instrumented `gic::send_sgi(sgi_id, target_cpu)` with seven ring-only
    breadcrumbs; `slot_mask` encodes `target_cpu`
  - instrumented the existing `ISR_WAKEUP_BUFFERS[cpu].push(tid)` call with
    before/after breadcrumbs
  - widened the SPI34 postmortem AHCI-ring dump from 16 to 64 entries so the
    higher-volume SGI breadcrumbs remain correlatable with the F10 corridor
- Validation:
  - aarch64 kernel build completed with no `warning` or `error` lines
  - 5x `./run.sh --parallels --test 60`
  - final logs: `logs/breenix-parallels-cpu0/f11-send-sgi/run{1..5}/`

Sweep summaries:

```text
run1:
exit_status=1
ahci_timeouts=0
ahci_ring_entries=0
ahci_port0_is=0
ahci_port1_is=0
bsshd_started=1
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0
sgi_entry=0
sgi_after_mpidr=0
sgi_after_compose=0
sgi_before_msr=0
sgi_after_msr=0
sgi_after_isb=0
sgi_exit=0
wakebuf_before_push=0
wakebuf_after_push=0

run2:
exit_status=1
ahci_timeouts=0
ahci_ring_entries=0
ahci_port0_is=0
ahci_port1_is=0
bsshd_started=1
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0
sgi_entry=0
sgi_after_mpidr=0
sgi_after_compose=0
sgi_before_msr=0
sgi_after_msr=0
sgi_after_isb=0
sgi_exit=0
wakebuf_before_push=0
wakebuf_after_push=0

run3:
exit_status=1
ahci_timeouts=4
ahci_ring_entries=130
ahci_port0_is=2
ahci_port1_is=2
bsshd_started=1
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=2
unblock_exit=0
sgi_entry=17
sgi_after_mpidr=18
sgi_after_compose=18
sgi_before_msr=18
sgi_after_msr=18
sgi_after_isb=18
sgi_exit=19
wakebuf_before_push=0
wakebuf_after_push=0

run4:
exit_status=1
ahci_timeouts=0
ahci_ring_entries=0
ahci_port0_is=0
ahci_port1_is=0
bsshd_started=1
before_complete=0
after_complete=0
wake_enter=0
wake_exit=0
unblock_entry=0
unblock_after_cpu=0
unblock_after_buffer=0
unblock_after_need_resched=0
unblock_before_sgi_scan=0
unblock_per_sgi=0
unblock_exit=0
sgi_entry=0
sgi_after_mpidr=0
sgi_after_compose=0
sgi_before_msr=0
sgi_after_msr=0
sgi_after_isb=0
sgi_exit=0
wakebuf_before_push=0
wakebuf_after_push=0

run5:
exit_status=1
ahci_timeouts=4
ahci_ring_entries=138
ahci_port0_is=2
ahci_port1_is=2
bsshd_started=1
before_complete=3
after_complete=2
wake_enter=1
wake_exit=0
unblock_entry=1
unblock_after_cpu=1
unblock_after_buffer=1
unblock_after_need_resched=1
unblock_before_sgi_scan=1
unblock_per_sgi=1
unblock_exit=0
sgi_entry=16
sgi_after_mpidr=17
sgi_after_compose=17
sgi_before_msr=17
sgi_after_msr=16
sgi_after_isb=16
sgi_exit=17
wakebuf_before_push=1
wakebuf_after_push=1
```

- Primary stalled-token extract from `run5`:

```text
[ahci] read_block(494667) wait failed: AHCI: command timeout
[STUCK_SPI34] cpu=5 gic_version=3
[STUCK_SPI34] GICD_ISPENDR[1]=0x800004 bit=2 pending=true
[STUCK_SPI34] GICD_ISACTIVER[1]=0x4 bit=2 active=true
[STUCK_SPI34] ICC_RPR_EL1=0xff
[STUCK_SPI34] ICC_PMR_EL1=0xf8
[STUCK_SPI34] DAIF=0x300
[STUCK_SPI34] AHCI_PORT0_IS=0x0
[STUCK_SPI34] AHCI_PORT1_IS=0x1
[AHCI_RING] nsec=2450015250 site=SGI_BEFORE_MSR cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6170
[AHCI_RING] nsec=2450014333 site=SGI_AFTER_COMPOSE cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6169
[AHCI_RING] nsec=2450013291 site=SGI_AFTER_MPIDR cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6168
[AHCI_RING] nsec=2450012375 site=SGI_ENTRY cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6167
[AHCI_RING] nsec=2449989500 site=SGI_BEFORE_MSR cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6165
[AHCI_RING] nsec=2449988625 site=SGI_AFTER_COMPOSE cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6164
[AHCI_RING] nsec=2449987750 site=SGI_AFTER_MPIDR cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6163
[AHCI_RING] nsec=2449986791 site=SGI_ENTRY cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=0 slot_mask=0x1 token=0 wake_success=0 seq=6162
[AHCI_RING] nsec=2449985791 site=UNBLOCK_PER_SGI cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=0 wake_success=0 seq=6161
[AHCI_RING] nsec=2449984875 site=UNBLOCK_BEFORE_SGI_SCAN cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6160
[AHCI_RING] nsec=2449983958 site=UNBLOCK_AFTER_NEED_RESCHED cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6159
[AHCI_RING] nsec=2449983083 site=UNBLOCK_AFTER_BUFFER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6158
[AHCI_RING] nsec=2449982166 site=WAKEBUF_AFTER_PUSH cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6157
[AHCI_RING] nsec=2449981250 site=WAKEBUF_BEFORE_PUSH cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6156
[AHCI_RING] nsec=2449980000 site=UNBLOCK_AFTER_CPU cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6155
[AHCI_RING] nsec=2449978750 site=UNBLOCK_ENTRY cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=6154
[AHCI_RING] nsec=2449977208 site=WAKE_ENTER cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1239 wake_success=0 seq=6153
[AHCI_RING] nsec=2449975708 site=BEFORE_COMPLETE cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1239 wake_success=0 seq=6152
[AHCI_RING] nsec=2449974375 site=POST_CLEAR cpu=0 port=1 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=1239 wake_success=0 seq=6151
[AHCI_RING] nsec=2449502375 site=ENTER cpu=0 port=1 IS=0x1 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=1239 wake_success=0 seq=6150
```

- Last-site verdict:
  - primary signature: `run5`, token `1239`, waiter `tid=11`, reached
    `UNBLOCK_PER_SGI` with `slot_mask=0x1` (target CPU 1). The contiguous SGI
    breadcrumb sequence for that target reached `SGI_BEFORE_MSR`; no
    `SGI_AFTER_MSR` was emitted for that contiguous call before the trace moved
    on. This points at the `ICC_SGI1R_EL1` write corridor.
  - corroborating but less clean signature: `run3`, waiter `tid=13`, had
    `UNBLOCK_PER_SGI` for targets 5 and 6. The target-6 SGI reached
    `SGI_AFTER_ISB` without a following `SGI_EXIT` in the captured ring, but
    later CPU-0 SGI entries make this weaker as a single-call proof.
  - secondary wake-buffer signature: not reproduced in the decisive sample.
    `WAKEBUF_BEFORE_PUSH` and `WAKEBUF_AFTER_PUSH` both appear for `tid=11`,
    so `IsrWakeupBuffer::push()` did not stall in `run5`.
- Caveat:
  - F11 uses only existing ring fields, so SGI calls are correlated by
    adjacency and `slot_mask`, not by a per-call ID. The `SGI_BEFORE_MSR`
    verdict is the best available last-site result for the primary stuck token,
    but F12 should add a per-SGI diagnostic call ID if it needs to distinguish
    concurrent or later same-target SGI calls with absolute certainty.
- F12 recommendation:
  - primary next action: audit the `msr ICC_SGI1R_EL1, xN` corridor and GICv3
    CPU-interface state for system-register SGI delivery. The current best
    verdict is `SGI_BEFORE_MSR` stops, which means the register write itself or
    its immediate architectural effects are the suspect.
  - verify SRE/ICC state on participating CPUs, especially whether the sender
    and targets have GICv3 system-register access consistently enabled before
    scheduler IPIs.
  - keep wake-buffer push as lower priority for now. The decisive run had
    before/after push markers, eliminating the F10 run2-style secondary
    signature for that sample.
## 2026-04-16 - F12 SGI Linux parity probe

F12 tested the first fix probe after the F11 SGI boundary finding. The probe
matched the Linux v6.8 GICv3 SGI ordering around `ICC_SGI1R_EL1`, added a
per-CPU `ICC_SRE_EL1` audit at CPU-interface initialization, and reran the
5x Parallels CPU0 sweep.

### Linux parity comparison

| Linux reference | Breenix reference | Divergence | Applied fix |
| --- | --- | --- | --- |
| `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1365-1387` | `kernel/src/arch_impl/aarch64/gic.rs:1027-1066` | Linux runs `dsb(ishst)` before emitting SGIs and `isb()` after the `ICC_SGI1R_EL1` writes. Breenix already had the post-MSR `isb`, but did not have the pre-MSR `dsb ishst`. | Added `dsb ishst` before SGI target-list construction and the `msr icc_sgi1r_el1` write. Kept the existing post-MSR `isb`. |
| `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1350-1363` | `kernel/src/arch_impl/aarch64/gic.rs:1040-1061` | Linux composes affinity, routing selector, INTID, and target list before `gic_write_sgi1r(val)`. Breenix composes INTID and a same-affinity target list for the simple CPU0-7 topology. No DAIF/preemption masking and no WFE/SEV pairing appear in this Linux path. | No DAIF, WFE, or SEV changes were made. The write remains `msr icc_sgi1r_el1, <sgir>`. |
| F12 requirement | `kernel/src/arch_impl/aarch64/gic.rs:482-515`, `kernel/src/arch_impl/aarch64/gic.rs:1478-1499` | Breenix had no one-time `ICC_SRE_EL1` audit for each CPU. Secondary CPU-interface init could return before the ICC system-register setup when the GICR range guard rejected the MPIDR-derived CPU ID. | Added `[SRE_AUDIT] cpu=<id> sre=<value> raw=<value>` after `ICC_SRE_EL1` readback, and moved secondary ICC system-register setup before the GICR MMIO range guard. |

### Validation sweep

All runs used:

```text
./run.sh --parallels --test 60
```

Artifacts are in:

```text
logs/breenix-parallels-cpu0/f12-sgi-parity/run{1..5}/
```

| Run | Result | bsshd | AHCI timeouts | Corruption markers | SRE audit lines | Stuck-state signature |
| --- | --- | --- | --- | --- | --- | --- |
| run1 | PASS | 1 | 0 | 0 | 2 | None. Reached `[init] bsshd started (PID 2)`. |
| run2 | PASS with SRE audit anomaly | 1 | 0 | 0 | 4 | None. Reached `[init] bsshd started (PID 2)`. One garbled SRE line did not contain literal `sre=1`, so `sre_unexpected=1`; this appears to be raw UART interleaving, not a confirmed SRE=0. |
| run3 | FAIL | 1 | 2 | 0 | 1 | SPI34 stuck pending+active on cpu=3 and cpu=6. `ICC_PMR_EL1=0xf8`, `ICC_RPR_EL1=0xff`, `DAIF=0x300`, `AHCI_PORT1_IS=0x1`; later `[init] Failed to exec bsh: EIO`. |
| run4 | FAIL | 1 | 2 | 0 | 1 | SPI34 stuck pending+active on cpu=4 and cpu=5, followed by `[init] Failed to exec bsh: EIO` and `SOFT LOCKUP DETECTED`. |
| run5 | PASS | 1 | 0 | 0 | 1 | None. Reached `[init] bsshd started (PID 2)`. |

### SRE audit output

Observed SRE audit lines from the final sweep:

```text
run1:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7
[3@1ABgic]CDE eFG3[SRE_AUDIT] cIpu=3 sre=1 raw=0x7

run2:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7
T1[smp] 1C@PU1A 1:B CDPSCI CPU_ONEe suFGc1[SRE_AUDIT] ccpuess=
[g6@1ABCDEeFG6[SRE_AUDIT] cpu=6i sre=1 rawc=] ICC0_xCTL7
7@1ABCDEeFG7[SRE_AUDIT] cpu=7 sre=1 raw=0x7

run3:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7

run4:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7

run5:
[SRE_AUDIT] cpu=0 sre=1 raw=0x7
```

CPU0 consistently reads `ICC_SRE_EL1=0x7` (`sre=1`). Secondary CPU audit
coverage is incomplete and raw UART lines can interleave with other boot output.
The visible secondary samples also indicate `sre=1`, but F12 did not produce a
clean one-line-per-CPU proof for all CPUs. That is a finding for the next probe.

### Interpretation

The Linux SGI ordering change is not sufficient to close the AHCI timeout
corridor. The original SGI-write hang signature is improved in the sense that
all five runs reached `[init] bsshd started (PID 2)`, but the probe pass criteria
were stricter: all five runs also needed `ahci_timeouts=0` and
`corruption_markers=0`. Runs 3 and 4 still produced AHCI command timeouts with
SPI34 pending+active and no corruption markers.

F12 verdict: FAIL.

Recommended F13 direction: pivot from another SGI-side barrier probe to
per-CPU GIC state and redistributor configuration. Specifically, add a
non-garbled per-CPU audit of `ICC_SRE_EL1`, `ICC_CTLR_EL1`, `ICC_PMR_EL1`,
`ICC_IGRPEN1_EL1`, MPIDR affinity, and the selected GICR frame, then compare the
redistributor wake/config state for CPUs that later report SPI34 stuck
pending+active.

## 2026-04-16 F13: Per-CPU GIC Audit and Idle-Scan Breadcrumbs

Branch: `diagnostic/f13-gic-audit`

F13 extended the GICv3 CPU-interface audit and added three AHCI ring
breadcrumbs around the idle-CPU scan in `isr_unblock_for_io()`. The per-CPU
audit values are captured at each CPU's ICC init and emitted from CPU0 after SMP
bring-up to avoid secondary boot raw-UART bytes corrupting the audit prefix.

### Linux expected values

| Field | Linux reference | Linux expected | F13 Breenix observed | Divergence |
| --- | --- | --- | --- | --- |
| `ICC_SRE_EL1` | `/tmp/linux-v6.8/include/linux/irqchip/arm-gic-v3.h:644-656`; `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1147-1155` | SRE bit set | `0x7` on CPUs 0-7 | No. SRE, DFB, and DIB are set. |
| `ICC_CTLR_EL1` | `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1188-1193` | EOImode drop bit set | `0x40402` on CPUs 0-7 | No for EOImode. Extra implementation bits are present. |
| `ICC_PMR_EL1` | `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1161-1164` | `DEFAULT_PMR_VALUE` = `0xf0` | `0xf8` on CPUs 0-7 | Yes. All CPUs use a stricter mask than Linux default. |
| `ICC_IGRPEN1_EL1` | `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1231-1233` | `1` | `0x1` on CPUs 0-7 | No. |
| Redistributor lookup | `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:978-1011`, `:1017-1045`, `:1274-1299` | Iterate GICR frames and match `GICR_TYPER[63:32]` to MPIDR affinity before enabling the redistributor | F13 diagnostic reads by `target_cpu * 0x20000`; all dumped `GICR_TYPER` values are `0x0` | Yes or diagnostic-map invalid. Linux does not assume logical CPU ID equals redist frame index without an affinity match. |

Linux register offsets used for the GICR dump are from
`/tmp/linux-v6.8/include/linux/irqchip/arm-gic-v3.h:114-125`; WAKER bit meanings
are at `:148-149`.

### Validation sweep

Artifacts:

```text
logs/breenix-parallels-cpu0/f13-gic-audit/run{1..5}/
```

| Run | AHCI timeout | `gic_cpu_audit_lines` | `gicr_waker_lines` | `scan_start` | `scan_cpu` | `scan_done` | Last emitted scan/unblock site |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| run1 | 0 | 8 | 0 | 0 | 0 | 0 | none |
| run2 | 0 | 8 | 0 | 0 | 0 | 0 | none |
| run3 | 2 | 8 | 16 | 0 | 0 | 0 | none in dumped ring; failure was SGI-side only |
| run4 | 2 | 8 | 16 | 1 | 2 | 0 | `UNBLOCK_SCAN_CPU` / `UNBLOCK_PER_SGI` |
| run5 | 2 | 8 | 16 | 0 | 0 | 1 | `UNBLOCK_SCAN_DONE` / `UNBLOCK_AFTER_CPU` |

The sweep is still FAIL: 2/5 runs had no AHCI timeout, 3/5 produced AHCI
timeouts. The new audit coverage criterion is met: every run produced eight
`[GIC_CPU_AUDIT]` rows.

### GIC CPU audit rows

Representative rows from run4:

```text
[GIC_CPU_AUDIT] cpu=0 mpidr=0x80000000 sre=0x7 ctlr=0x40402 pmr=0xf8 igrpen1=0x1 mismatch=1
[GIC_CPU_AUDIT] cpu=1 mpidr=0x80000001 sre=0x7 ctlr=0x40402 pmr=0xf8 igrpen1=0x1 mismatch=1
[GIC_CPU_AUDIT] cpu=2 mpidr=0x80000002 sre=0x7 ctlr=0x40402 pmr=0xf8 igrpen1=0x1 mismatch=1
T3[GIC_CPU_AUDIT] cpu=3 mpidr=0x80000003 sre=0x7 ctlr=0x40402 pmr=0xf8 igrpen1=0x1 mismatch=1
[GIC_CPU_AUDIT] cpu=4 mpidr=0x80000004 sre=0x7 ctlr=0x40402 pmr=0xf8 igrpen1=0x1 mismatch=1
[GIC_CPU_AUDIT] cpu=5 mpidr=0x80000005 sre=0x7 ctlr=0x40402 pmr=0xf8 igrpen1=0x1 mismatch=1
[GIC_CPU_AUDIT] cpu=6 mpidr=0x80000006 sre=0x7 ctlr=0x40402 pmr=0xf8 igrpen1=0x1 mismatch=1
T4[GIC_CPU_AUDIT] cpu=7 mpidr=0x80000007 sre=0x7 ctlr=0x40402 pmr=0xf8 igrpen1=0x1 mismatch=1
```

The `T3`/`T4` prefixes are raw timer breadcrumbs interleaving before the locked
serial line; the `[GIC_CPU_AUDIT]` payload itself is intact. The mismatch flag is
set on every CPU because `ICC_PMR_EL1` is `0xf8`, not Linux's `0xf0`.

### GICR state at timeout

Run4's stuck unblock selected target CPU 1 immediately before entering
`send_sgi()`:

```text
[AHCI_RING] nsec=2388322625 site=UNBLOCK_PER_SGI cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=0 wake_success=0 seq=5749
[AHCI_RING] nsec=2388321750 site=UNBLOCK_SCAN_CPU cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x1 token=0 wake_success=0 seq=5748
[AHCI_RING] nsec=2388320875 site=UNBLOCK_SCAN_CPU cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=5747
[AHCI_RING] nsec=2388319166 site=UNBLOCK_SCAN_START cpu=0 port=0 IS=0x0 CI=0x0 SACT=0x0 SERR=0x0 waiter_tid=11 slot_mask=0x0 token=0 wake_success=0 seq=5745
```

The target CPU 1 GICR dump from the same timeout:

```text
[GICR_STATE] cpu=1 waker=0x0 ctlr=0x0 typer=0x0 syncr=0x0
```

All target-history CPUs dumped in runs 3-5 had the same shape:
`waker=0x0 ctlr=0x0 typer=0x0 syncr=0x0`. `WAKER=0` means
`ProcessorSleep=0` and `ChildrenAsleep=0`, so the dumped frame does not look
asleep. The concerning value is `GICR_TYPER=0x0`: Linux expects redistributor
selection to be validated by matching `GICR_TYPER[63:32]` to CPU affinity. F13's
dump therefore does not prove the redistributor for CPU 1 is healthy; it proves
the current logical-CPU-to-GICR-frame diagnostic map is suspect.

### Interpretation

F13 did not reproduce a CPU-specific SRE/CTLR failure. Every captured CPU has
SRE enabled, EOImode enabled, and Group1 enabled. The only ICC divergence is
global, not CPU-specific: PMR is `0xf8` on every CPU versus Linux's `0xf0`.

The timeout corridor is not purely "before the scan" anymore. Run4 shows the
wake path entered the scan, inspected CPU 0 and CPU 1, emitted
`UNBLOCK_PER_SGI` for target CPU 1, and did not emit `UNBLOCK_SCAN_DONE`. That
narrows this instance to the scan-to-SGI handoff, most likely between
`UNBLOCK_PER_SGI` and the first `SGI_ENTRY` breadcrumb inside `send_sgi()`.

Verdict: not (a) a specific CPU with broken SRE/CTLR. Not proven (b) a sleeping
redistributor, because WAKER is clear, but GICR selection is not trustworthy
while `GICR_TYPER` reads as zero for every target frame. The current best answer
is (c) still inside the scan/SGI handoff logic, with a parallel redistributor
mapping defect to resolve before trusting target-state dumps.

### F14 recommendation

F14 should do two targeted things:

1. Add Linux-style redistributor discovery: iterate GICR frames, read
   `GICR_TYPER`, match affinity bits against MPIDR, and store a per-CPU RD base.
   Use that map for `dump_gicr_state_for_cpu()` first; only change live GICR init
   semantics after the diagnostic map proves the current frame index is wrong.
2. Add two more ring sites around the call in `isr_unblock_for_io()`:
   `UNBLOCK_BEFORE_SEND_SGI` and `UNBLOCK_AFTER_SEND_SGI`. Run4's latest
   sequence stops at `UNBLOCK_PER_SGI`, so F14 needs to distinguish call-entry
   failure from an early `send_sgi()` body failure.

## 2026-04-16 F14: Linux-Style GICR Discovery and Send-SGI Call-Site Breadcrumbs

Branch: `probe/f14-gicr-discovery`

F14 replaced the logical-CPU-index GICR assumption with Linux-style
redistributor discovery. The scan walks RD frames in `0x20000` strides from the
validated GICR base and matches `GICR_TYPER[63:32]` against each CPU's MPIDR
affinity, following Linux's `gic_iterate_rdists()` /
`__gic_populate_rdist()` model in
`/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:978-1011` and `:1017-1045`.
`GICR_TYPER_LAST` is bit 4 per
`/tmp/linux-v6.8/include/linux/irqchip/arm-gic-v3.h:238-249`; the F14 prompt's
bit-31 wording conflicts with the supplied Linux source.

F14 also added `UNBLOCK_BEFORE_SEND_SGI` and `UNBLOCK_AFTER_SEND_SGI` around
the caller-side `send_sgi()` invocation in `isr_unblock_for_io()`. `send_sgi()`
itself was not changed.

### Validation sweep

Artifacts:

```text
logs/breenix-parallels-cpu0/f14-gicr-discovery/run{1..5}/
logs/breenix-parallels-cpu0/f14-gicr-discovery/summary.txt
```

Each `./run.sh --parallels --test 60` invocation completed the 60-second boot
window and captured `serial.log`, but returned exit status 1 because the
screenshot helper could not find the generated Parallels window. The serial log
is the validation source below.

| Run | Reached bsshd | AHCI timeout | Corruption markers | `gic_cpu_audit_lines` | `gicr_map_lines` | `gicr_map_found` | `gicr_map_not_found` | `gicr_state_lines` | `unblock_before_send_sgi` | `unblock_after_send_sgi` | `scan_start` | `scan_cpu` | `scan_done` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| run1 | 1 | 0 | 0 | 8 | 8 | 8 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| run2 | 1 | 2 | 0 | 8 | 8 | 8 | 0 | 16 | 0 | 1 | 0 | 0 | 1 |
| run3 | 1 | 2 | 0 | 8 | 8 | 8 | 0 | 16 | 2 | 1 | 1 | 3 | 0 |
| run4 | 1 | 0 | 0 | 8 | 8 | 8 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| run5 | 1 | 0 | 0 | 8 | 8 | 8 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

Verdict: **FAIL**. All five runs reached `[init] bsshd started (PID 2)`, and
all five had `corruption_markers=0`, but runs 2 and 3 each produced two AHCI
port timeouts.

### GICR map output

Representative run2 output:

```text
T4[GICR_MAP] cpu=0 rd_base=0x2500000 typer=0x0 affinity=0x0
[GICR_MAP] cpu=1 rd_base=0x25e0000 typer=0x100000710 affinity=0x1
[GICR_MAP] cpu=2 rd_base=0x25c0000 typer=0x200000600 affinity=0x2
[GICR_MAP] cpu=3 rd_base=0x2540000 typer=0x300000200 affinity=0x3
[GICR_MAP] cpu=4 rd_base=0x2580000 typer=0x400000400 affinity=0x4
T5[GICR_MAP] cpu=5 rd_base=0x2560000 typer=0x500000300 affinity=0x5
[GICR_MAP] cpu=6 rd_base=0x2520000 typer=0x600000100 affinity=0x6
[GICR_MAP] cpu=7 rd_base=0x25a0000 typer=0x700000500 affinity=0x7
```

The redistributor frame order is not logical-CPU order and changes across
boots under Parallels/HVF. F14 confirms F13's `target_cpu * 0x20000` diagnostic
read was wrong for CPUs 1-7. CPU0's `GICR_TYPER=0x0` is the real first-frame
value: affinity 0, processor number 0, and no LAST bit can encode as zero.

### GICR state before and after

F13 before, target CPU 1:

```text
[GICR_STATE] cpu=1 waker=0x0 ctlr=0x0 typer=0x0 syncr=0x0
```

F14 after, run2:

```text
[GICR_STATE] cpu=1 rd_base=0x25e0000 waker=0x0 ctlr=0x0 typer=0x100000710 syncr=0x0
[GICR_STATE] cpu=2 rd_base=0x25c0000 waker=0x0 ctlr=0x0 typer=0x200000600 syncr=0x0
[GICR_STATE] cpu=3 rd_base=0x2540000 waker=0x0 ctlr=0x0 typer=0x300000200 syncr=0x0
[GICR_STATE] cpu=0 rd_base=0x2500000 waker=0x0 ctlr=0x0 typer=0x0 syncr=0x0
```

Timeout-time dumps now show non-zero `GICR_TYPER` values for target CPUs 1-7.
CPU0 remains zero for the valid first RD frame.

### Interpretation

F14 fixes the GICR map defect but does not close the AHCI timeout corridor. In
run3, the dumped AHCI ring includes `UNBLOCK_BEFORE_SEND_SGI` without a matching
newest `UNBLOCK_AFTER_SEND_SGI` for the same target, while earlier SGI-entry
breadcrumbs are present for other targets. That keeps the scan-to-SGI handoff
in scope.

Recommended F15 direction: with redistributor mapping now trustworthy, target
the remaining Linux divergence (`ICC_PMR_EL1=0xf8` in Breenix versus Linux's
`0xf0`) or inspect the idle-scan/send_sgi handoff logic directly. F14 does not
justify F-final.

## 2026-04-16 - F15 ICC_PMR_EL1 Linux parity

F15 targeted the last isolated ICC hot-path divergence from F13/F14:
Breenix runtime `ICC_PMR_EL1=0xf8` versus Linux's default `0xf0`.

Linux v6.8 defines `DEFAULT_PMR_VALUE` as `0xf0` in
`/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:146`, writes that value to
`ICC_PMR_EL1` in `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1161-1164`,
and defines the GICv2 CPU-interface threshold `GICC_INT_PRI_THRESHOLD` as
`0xf0` in `/tmp/linux-v6.8/include/linux/irqchip/arm-gic.h:23`.

### PMR write audit

`kernel/src/arch_impl/aarch64/gic.rs` had three PMR init writes:

- Primary GICv2 CPU-interface init wrote `GICC_PMR`.
- Secondary GICv2 CPU-interface init wrote `GICC_PMR`.
- GICv3 CPU-interface init wrote `ICC_PMR_EL1`.

No non-init `ICC_PMR_EL1` write was found in `gic.rs`. Wider kernel grep found
PMR reads in diagnostics, but no additional PMR writes. The prior runtime
`0xf8` is consistent with writing `0xff` and reading back only implemented
priority bits. F15 replaced the shared PMR init value with Linux's `0xf0`.

### Validation sweep

Artifacts:

```text
logs/breenix-parallels-cpu0/f15-pmr-parity/run{1..5}/
logs/breenix-parallels-cpu0/f15-pmr-parity/summary.txt
```

Each `./run.sh --parallels --test 60` invocation completed the 60-second boot
window and captured `serial.log`, but returned exit status 1 because the
screenshot helper could not find the generated Parallels window. The serial log
is the validation source below.

| Run | Reached bsshd | AHCI timeout | Corruption markers | `gic_cpu_audit_lines` | `gicr_map_lines` | `gicr_map_found` | `gicr_map_not_found` | `gicr_state_lines` | `unblock_before_send_sgi` | `unblock_after_send_sgi` | `pmr_observed` | `audit_pmr_observed` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| run1 | 1 | 2 | 0 | 8 | 8 | 8 | 0 | 16 | 2 | 2 | `ICC_PMR_EL1=0xf0` | `pmr=0xf0` |
| run2 | 1 | 2 | 0 | 8 | 8 | 8 | 0 | 16 | 2 | 1 | `ICC_PMR_EL1=0xf0` | `pmr=0xf0` |
| run3 | 1 | 0 | 0 | 8 | 8 | 8 | 0 | 0 | 0 | 0 |  | `pmr=0xf0` |
| run4 | 1 | 2 | 0 | 8 | 8 | 8 | 0 | 16 | 0 | 0 | `ICC_PMR_EL1=0xf0` | `pmr=0xf0` |
| run5 | 1 | 0 | 0 | 8 | 8 | 8 | 0 | 0 | 0 | 0 |  | `pmr=0xf0` |

Observed PMR values across CPUs: every run emitted eight `[GIC_CPU_AUDIT]`
lines, one per CPU, and every audit line reported `pmr=0xf0` with
`mismatch=0`. Timeout-time stuck-state dumps in runs 1, 2, and 4 also reported
`ICC_PMR_EL1=0xf0`.

### Verdict

Verdict: **FAIL**. F15 closes the PMR parity gap, but it does not close the
AHCI timeout corridor. All five runs reached `[init] bsshd started (PID 2)` and
had `corruption_markers=0`, but runs 1, 2, and 4 each produced two AHCI
timeouts.

The residual signature is intermittent AHCI timeout after successful boot with
Linux-parity PMR confirmed on all CPUs. F16 should audit the idle-scan loop
logic / scan-to-SGI handoff semantics. F15 does not justify F-final.

## 2026-04-16 - F17 local IRQ-return wake-buffer audit

F17 audited the local-wake path left after F16 removed the broadcast idle-CPU
scan. The target case was AHCI completion on the same CPU as the blocked waiter,
where no SGI should be needed: the ISR should publish the wake, set
`need_resched`, and the same CPU's IRQ-return path should drain the wake buffer
before making the scheduling decision.

### Ordering audit

`isr_unblock_for_io()` now records the local path around the wake publication:
`TTWU_LOCAL_ENTRY` after resolving the current CPU, `WAKEBUF_BEFORE_PUSH` /
`WAKEBUF_AFTER_PUSH` around the per-CPU wake-buffer write, and
`TTWU_LOCAL_SET_RESCHED` after `set_need_resched()`.

The scheduler drain point is at the top of `schedule_deferred_requeue()`: it
drains all `ISR_WAKEUP_BUFFERS`, calls `unblock_for_io()` for each drained TID,
and records `RESCHED_CHECK_DRAINED_WAKE` with the drained count, ready-queue
length, and current `need_resched` value.

On ARM64 IRQ return, `handle_irq()` records `IRQ_TAIL_CHECK_RESCHED` before its
tail resched wrapper, and `boot.S` then calls
`check_need_resched_and_switch_arm64()` before restoring the exception frame.
`check_need_resched_and_switch_arm64()` records `RESCHED_CHECK_ENTRY`,
`RESCHED_CHECK_SWITCHED`, and `RESCHED_CHECK_RETURN`.

### Validation sweep

Artifacts:

```text
logs/breenix-parallels-cpu0/f17-local-wake/run{1..5}/
logs/breenix-parallels-cpu0/f17-local-wake/summary.txt
logs/breenix-parallels-cpu0/f17-local-wake/exit.md
```

| Run | Reached bsshd | AHCI timeout | Corruption markers | Failed exec | Soft lockup | `ttwu_local_entry` | `ttwu_local_set_resched` | `irq_tail_check_resched` | `resched_check_entry` | `resched_check_drained_wake` | `resched_check_switched` | `resched_check_return` |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| run1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| run2 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| run3 | 1 | 2 | 0 | 1 | 2 | 1 | 0 | 3 | 3 | 3 | 1 | 2 |
| run4 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| run5 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

The final sweep therefore passed the explicit AHCI-timeout criteria in four of
five runs and failed run3. All five runs reached `[init] bsshd started (PID 2)`;
no corruption markers were observed.

Run3 preserved the local-wake corridor:

```text
TTWU_LOCAL_ENTRY cpu=0 waiter_tid=11 slot_mask=0x0 token=1
WAKEBUF_AFTER_PUSH waiter_tid=11
IRQ_TAIL_CHECK_RESCHED cpu=0 slot_mask=0x1 token=1
RESCHED_CHECK_ENTRY cpu=0 slot_mask=0x1 token=1
RESCHED_CHECK_DRAINED_WAKE cpu=0 waiter_tid=1 slot_mask=0x1 token=1 wake_success=1
RESCHED_CHECK_SWITCHED cpu=0 waiter_tid=11 token=1 wake_success=1
RESCHED_CHECK_RETURN cpu=0 slot_mask=0x0 token=0
```

The explicit `TTWU_LOCAL_SET_RESCHED` breadcrumb was present in diagnostic-run2
before the final sweep. Final run3's AHCI ring dump has a sequence gap at the
expected slot, but the immediately following `IRQ_TAIL_CHECK_RESCHED` shows
`token=1` and `slot_mask=0x1`, confirming that `need_resched` and the wake
buffer depth were visible at IRQ tail.

### Determination

F17 falsifies H1: `TTWU_LOCAL_ENTRY` is followed by
`IRQ_TAIL_CHECK_RESCHED` and `RESCHED_CHECK_ENTRY`, so the IRQ-return scheduler
check is reached.

F17 also falsifies H2: `RESCHED_CHECK_DRAINED_WAKE` with `wake_success=1`
appears before `RESCHED_CHECK_SWITCHED`, so the wake buffer is drained before
the scheduling decision and the woken TID is selected.

Confirmed bucket: **Other**. The local wake path works through switch. The
remaining failure is later in the AHCI/SPI34 corridor: run3 times out on a later
command (`cmd#=1374`) after the completed local wake (`last_port1_cmd_num=1372`)
and reports `GICD_ISPENDR[1]` pending for SPI34, `GICD_ISACTIVER[1]` active for
SPI34, `AHCI_PORT1_IS=0x1`, and `CI=0x0`.

### Verdict

Verdict: **FAIL**. F17 should not merge as a final fix. No F17 scheduler fix was
applied because both local scheduler hypotheses were disproven and the remaining
suspect area is outside the F17 non-goals.

Recommended F18 direction: investigate why SPI34 remains active/pending with
AHCI Port1 `IS=0x1` and no completion for a later command after the local wake
path has already switched successfully. The next audit should focus on AHCI
interrupt completion / EOI-deactivation ordering or command publication, not on
the local IRQ-return wake-buffer drain.

## 2026-04-16 - F18 AHCI CI-level completion loop

F18 targeted the AHCI completion race isolated by F17: timeout dumps showed
`CI=0x0` and `AHCI_PORT1_IS=0x1` while the waiter was blocked on a command
newer than the last command completed by the ISR. That means hardware had
completed the command, but the software completion path missed the waiter wake.

### Audit summary

The F17 Breenix AHCI handler was edge-sensitive. It read `PORT_IS`, cleared
`PORT_IS`/`HBA_IS`, read `PORT_CI` once, and completed at most one slot from
that single sampled interrupt state. That can miss a completion once the
hardware `PORT_CI` bit has already dropped but no new software completion pass
runs for the active slot.

Linux v6.8 is level-sensitive. In `/tmp/linux-v6.8/drivers/ata/libahci.c`,
`ahci_port_intr()` reads and acknowledges `PORT_IRQ_STAT` at lines 1963-1964,
then calls `ahci_handle_port_interrupt()` at line 1966. The command completion
path runs through `ahci_qc_complete()`; it reads `PORT_SCR_ACT` and
`PORT_CMD_ISSUE` into `qc_active` at lines 1875-1885, then calls
`ata_qc_complete_multiple(ap, qc_active)` at line 1888. The important parity
point is deriving completions from currently active hardware command bits, not
from a single interrupt edge.

### Fix description

`kernel/src/drivers/ahci/mod.rs` now bounds the AHCI completion drain loop at
eight iterations. For each active port, it:

- reads the per-port active software mask and `PORT_CI`;
- computes completed slots as `PORT_ACTIVE_MASK & !PORT_CI`;
- clears completed active bits atomically;
- acknowledges sampled `PORT_IS`;
- re-reads `PORT_IS` and `PORT_CI`, looping if the port reasserted or another
  active slot is now clear.

Because the current driver issues only slot 0, the slot-0 completion token is
recorded during the CI loop and the actual wake is published only after the
port interrupt is stable. This prevents the woken waiter from issuing the next
slot-0 command while the prior AHCI interrupt line remains asserted. The prior
single-active-slot fallback for controllers that signal completion before
`PORT_CI` is observed clear was preserved.

F18 also added AHCI ring site `CI_LOOP`; the loop iteration count is emitted
in the event `token` field. Passing validation runs did not trigger
timeout-time AHCI ring dumps, so visible `ahci_ci_loop_iterations` is `0` in
the final serial logs.

### Validation sweep

Artifacts:

```text
logs/breenix-parallels-cpu0/f18-ahci-ci-loop/run{1..5}/
logs/breenix-parallels-cpu0/f18-ahci-ci-loop/summary.txt
logs/breenix-parallels-cpu0/f18-ahci-ci-loop/exit.md
```

Each `./run.sh --parallels --test 60` invocation exited 1 because the
Parallels screenshot helper could not find the generated VM window. As in
prior F-series sweeps, the serial log is the validation source.

| Run | Reached bsshd | AHCI timeout | Corruption markers | Failed exec | Soft lockup | `ahci_ci_loop_iterations` | Result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| run1 | 1 | 0 | 0 | 0 | 0 | 0 | PASS |
| run2 | 1 | 0 | 0 | 0 | 0 | 0 | PASS |
| run3 | 1 | 0 | 0 | 0 | 0 | 0 | PASS |
| run4 | 1 | 0 | 0 | 0 | 0 | 0 | PASS |
| run5 | 1 | 0 | 0 | 0 | 0 | 0 | PASS |

### Verdict

Verdict: **PASS**. F18 reached the target 5/5 Parallels sweep: every final run
reached `[init] bsshd started (PID 2)` with `ahci_timeouts=0` and
`corruption_markers=0`.

The ARM64 CPU0/AHCI timeout investigation is therefore complete for this
failure signature. Recommended next step: open a cleanup PR to remove the
F8-F17 diagnostic scaffolding and keep only the minimal AHCI CI-level
completion behavior plus any low-cost regression signal needed for future
AHCI timeout triage.

## 2026-04-17 - F19 Phase 0 hello_raw post-exec probe

F19 begins from the post-F18 state where `/sbin/init` can fork and exec
`/bin/bsh` without the prior AHCI `EIO`, but bsh produces no serial output and
init remains blocked in `waitpid`.

### Hypothesis

H0: if a minimal `/bin/hello_raw` binary writes to fd 1 after init execs it and
then exits with code 42, then the broad ARM64 exec path, fd inheritance, raw
write syscall path, and process exit/waitpid path are healthy for a simple
post-exec binary. The next phase should trace bsh userspace runtime startup.

H1: if `/bin/hello_raw` produces no output and init remains blocked in
`waitpid`, then the kernel exec/fd path is suspect and the next phase should
trace the kernel exec path.

H2: if `/bin/hello_raw` produces no output but init observes an exit code other
than 42, then the kernel likely enters userspace but lands at the wrong entry
point or terminates the process early; the next phase should also trace the
kernel exec path.

### Probe

Added `userspace/programs/src/hello_raw.rs`, registered it in
`userspace/programs/Cargo.toml`, installed it from
`userspace/programs/build.sh`, and temporarily changed
`userspace/programs/src/init.rs` to exec `/bin/hello_raw` instead of
`/bin/bsh`.

Validation command:

```text
./run.sh --parallels --test 45
```

The command exited nonzero because the screenshot helper could not find the
generated Parallels VM window. As in prior F-series sweeps, serial output is the
validation source. `/tmp/breenix-parallels-serial.log` contained:

```text
[init] Breenix init starting (PID 1)
F123456789SC[init] bsshd started (PID 2)
F123456789SCT9T0[hello_raw] start
[syscall] exit(42) pid=3 name=/bin/hello_raw
[init] Boot script exited with code 42
```

### Verdict

Verdict: **H0 confirmed**. A minimal post-exec userspace binary runs, can write
to fd 1, exits with code 42, and is observed by init through `waitpid`.

F19 Phase 0 therefore rules out a broad kernel exec/fd-inheritance failure for
the minimal path. The next branch should be **Phase 2**:
`diagnostic/f19-runtime-trace`, focused on bsh's userspace startup path
(`_start` / Rust runtime / `main`) and subsequent shell initialization.
