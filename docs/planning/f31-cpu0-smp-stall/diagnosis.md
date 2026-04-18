# F31 Phase 2 - CPU0+SMP Diagnosis

## Result

Root cause is F28 / PR #324's compositor waiter ordering change, not F29 / PR
#321's Parallels SMP re-enable.

The failing current-main sample stalls after bwm renders its initial frames and
before bsshd/bounce can be read from AHCI. The AHCI timeout dump shows the same
CPU0 WFI failure signature as the user report:

```text
cpu0_breadcrumb=107
CPU0_GICR_ISPENDR0=0x08000001 PPI27_pending=1
tick_count=[142,8419,8419,8418,8417,8473,8416,8418]
```

The requested predecessor samples do not reproduce:

- `d6aebfa1` (F27+F29+F30, no F28) reaches bsshd without AHCI timeout.
- `25083e23` (F27+F29) reaches bounce and CPU0 ticks 30000.
- `e2fd72e2` (F29 only) reaches bsshd without AHCI timeout.
- `ab351efe` (pre-F29) reaches bounce and CPU0 ticks 85000.

## Root Cause

F28 changed `handle_compositor_wait()` in `kernel/src/syscall/graphics.rs` to
call `block_current_for_compositor()` before publishing
`COMPOSITOR_WAITING_THREAD`. That fixed the conceptual lost-wake race, but it
also changed the early bwm idle path under Parallels SMP: CPU0 can enter the
blocked compositor WFI path before the waiter is visible to producers. In the
bad current-main run, CPU0 then falls into the known Parallels/HVF WFI failure
mode: the virtual timer PPI is pending and enabled, but CPU0 does not service it.

The F29 SMP re-enable is still a prerequisite for seeing the multi-CPU symptom,
but it is not the regressing PR in this sequence. F29 alone booted successfully.

## Attempted Minimal Fix

First attempt: keep F28's post-block readiness re-check, but restore the pre-F28
waiter publication timing:

1. Publish `COMPOSITOR_WAITING_THREAD` before blocking so producers have a
   stable target.
2. Mark the compositor blocked with `block_current_for_compositor()`.
3. Immediately re-check dirty, mouse, and registry state after blocking.
4. If a producer raced through while the compositor was still running, transition
   the still-current blocked syscall back to ready and return without WFI.

That was insufficient. The 120s validation reproduced the same signature with:

```text
tick_count=[144,8435,8436,8434,8435,8485,8434,8439]
CPU0_GICR_ISPENDR0=0x08000001 PPI27_pending=1
cpu0_breadcrumb=107
```

## Rollback Attempt

Second attempt: rollback the F28 `kernel/src/syscall/graphics.rs` behavior to
the last good SMP state from `d6aebfa1`. This removed the CPU0/AHCI timeout, but
it did not satisfy the full validation gate: bsshd listened, then VirGL direct
composite timed out and bounce never started.

## Fallback Attempts

Third attempt: keep the F28 graphics behavior and gate Parallels back to
single-CPU boot. This also failed the full gate: the run reached bwm, then
stopped before the telnetd lifecycle completed. That showed the issue was not
only the secondary CPU bring-up gate.

Fourth attempt: add the same Linux-parity `dsb sy` barrier that F20e added to
the idle loop to the shared ARM64 `halt_with_interrupts()` helper. This also
failed: the run reproduced the AHCI timeout and CPU0 stall under SMP.

Fifth attempt: combine the F28 graphics rollback with the Parallels single-CPU
gate. This also failed before userspace reached bwm creation.

## Fix Status

FAIL. No production code change from this factory passed the required 120s
Parallels gate. The branch currently preserves no kernel code changes from the
failed attempts; only the bisect and diagnosis documents are committed.

Recommended next step: open a rollback PR for F28 / PR #324 if immediate main
recovery is more important than preserving the F28 compositor wake work, then
re-investigate the compositor wait path from a clean branch with a narrower
validation matrix. The requested data does not support reverting F29 / PR #321
as the exact breaker, because F29 alone (`e2fd72e2`) booted successfully in the
Phase 1 sample.

## Evidence

- Phase 1 bisect table: `docs/planning/f31-cpu0-smp-stall/phase1-bisect.md`
- Raw serial artifacts:
  `.factory-runs/arm64-f31-cpu0-smp-stall-20260418-093559/phase1/*.serial.log`
- Failed minimal-fix validation:
  `.factory-runs/arm64-f31-cpu0-smp-stall-20260418-093559/phase4/minimal-fix-failed.serial.log`
- Failed F28 graphics rollback validation:
  `.factory-runs/arm64-f31-cpu0-smp-stall-20260418-093559/phase4/final-120-rollback.serial.log`
- Failed Parallels single-CPU fallback validation:
  `.factory-runs/arm64-f31-cpu0-smp-stall-20260418-093559/phase4/singlecpu-failed.serial.log`
- Failed shared WFI barrier validation:
  `.factory-runs/arm64-f31-cpu0-smp-stall-20260418-093559/phase4/dsb-halt-failed.serial.log`
- Failed conservative rollback combination validation:
  `.factory-runs/arm64-f31-cpu0-smp-stall-20260418-093559/phase4/conservative-failed.serial.log`
