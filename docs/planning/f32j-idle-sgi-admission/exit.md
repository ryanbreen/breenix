# F32j Idle Sleep Gate + GIC SGI Admission Exit

Date: 2026-04-19
Branch: `f32j-idle-sgi-admission`
Base: `f32i-cpu0-wfi-wake`
Issue: `breenix-eis`
Follow-up: `breenix-k16`

## Result

F32j did not pass the Phase 3/4 factory gate. The branch contains the idle sleep
gate and GIC SGI admission fixes, and `wait_stress` passed, but the required
5x 120-second Parallels sweep stopped at run 3 because the exact CPU0
tick/window evidence gate was not satisfied.

Per the factory contract, no PR was opened and no merge was attempted. The next
factory should implement F32i Option 2, the Linux-style target-CPU wake-list /
function-call IPI path.

## Commits Completed

| Phase | Commit | Status |
| --- | --- | --- |
| Phase 1 idle gate | `e7f20323 fix(kernel): F32j Linux idle sleep gate` | Complete |
| Phase 2 GIC admission | `946b2812 fix(kernel): F32j enable GIC SGIs for CPU0 admission` | Complete |
| Phase 3 validation | none | Failed gate at Parallels run 3 |
| Phase 4 PR/merge | none | Not attempted |

## Phase 1 Idle Gate

The idle loop implementation lives in
`kernel/src/arch_impl/aarch64/context_switch.rs`, where `idle_loop_arm64`
actually executes. The factory prompt named `kernel/src/main_aarch64.rs`, but
that file only wires the AArch64 entry path to the context-switch module.

Implemented semantics:

- Mask DAIF before the idle sleep decision.
- Use `dmb ish` ordered `need_resched` reads and ISR wake-buffer depth checks as
  the pre-WFI gate.
- Skip WFI and enter `schedule_from_kernel()` when either `need_resched` or
  pending ISR wake work is visible.
- Preserve the existing `dsb sy; wfi` sequence.
- Re-check after WFI before looping.

Linux parity cited in the commit body:

- `/tmp/linux-v6.8/kernel/sched/idle.c:258-259`
- `/tmp/linux-v6.8/kernel/sched/idle.c:261-289`
- `/tmp/linux-v6.8/kernel/sched/idle.c:291-314`
- `/tmp/linux-v6.8/kernel/sched/idle.c:317-340`

## Phase 2 Root Cause

F32i showed SGI0 pending in CPU0's redistributor while CPU0's CPU interface
reported `ICC_HPPIR1_EL1 = 1023`. The implemented root cause is that Breenix's
GICv3 redistributor initialization configured the interrupt group and priority,
then disabled all SGI/PPI lines and never re-enabled the reschedule SGI.

That state permits `GICR_ISPENDR0.SGI0 = 1` while keeping the interrupt
inadmissible to `HPPIR1/IAR1`, which matches the F32i trace. The fix enables
`SGI_RESCHEDULE` and `SGI_TIMER_REARM` in `GICR_ISENABLER0` after SGI/PPI
configuration, followed by `dsb sy; isb`.

The sweep checked the other prioritized candidates:

| Candidate | F32j finding |
| --- | --- |
| Group assignment | Already Group 1 via `GICR_IGROUPR0 = 0xffff_ffff` |
| Enable bit | Missing for SGI0/SGI1; fixed |
| Interface group enable | Already enabled via `ICC_IGRPEN1_EL1` |
| SGI routing | Existing `ICC_SGI1R_EL1` target-list path retained |
| Send barriers | Existing send path already uses `dsb ishst` and `isb` |
| Priority | Existing SGI/PPI priority `0xa0` is above `PMR=0xf0` |

Linux parity cited in the commit body:

- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1288-1302`
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1350-1387`

## Validation

Clean builds completed before the Parallels gate:

| Command | Result |
| --- | --- |
| `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64` | Pass, no warnings/errors |
| `cargo build --release --features testing,external_test_bins --bin qemu-uefi` | Pass, no warnings/errors |

Stress validation:

| Command | Result |
| --- | --- |
| `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150` | Pass: `WAIT_STRESS_PASS entered=155993 returned=155993 wakes=676185 waiters=0`; no `WAIT_STRESS_STALL`; strict render verdict PASS |

120-second Parallels sweep:

| Run | bsshd | bounce | Window line | CPU0 tick audit | FPS >= 160 | Render verdict | AHCI timeout | Result |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | Pass | Pass | Pass | Pass: max `cpu0 ticks=85000` | Pass: frame 19500 by tick 85000 | PASS | None seen | Pass |
| 2 | Pass | Pass | Pass | Pass: max `cpu0 ticks=80000` | Pass: frame 19000 by tick 80000 | PASS | None seen | Pass |
| 3 | Pass | Pass | Fail: interleaved line split `[bounce] Window mode` | Fail: no `cpu0 ticks=` lines captured | Not proven from tick audit | PASS | None seen | Fail |
| 4 | Not run | Not run | Not run | Not run | Not run | Not run | Not run | Stopped after run 3 |
| 5 | Not run | Not run | Not run | Not run | Not run | Not run | Not run | Stopped after run 3 |

Artifacts:

- `.factory-runs/f32j-idle-sgi-admission-20260419/wait-stress.serial.log`
- `.factory-runs/f32j-idle-sgi-admission-20260419/wait-stress.png`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run1.serial.log`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run1.png`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run1.verdict.txt`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run2.serial.log`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run2.png`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run2.verdict.txt`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run3.serial.log`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run3.png`
- `.factory-runs/f32j-idle-sgi-admission-20260419/parallels-run3.verdict.txt`

## PR

No PR URL. The 5/5 validation requirement was not met, so the factory stop
condition applied.

## Next Investigation

`breenix-k16` tracks F32k: implement F32i Option 2, a Linux-style target-CPU
wake-list / function-call IPI path so remote task wake completion is drained on
the target CPU. F32j improved the local idle gate and SGI admission state, but
the Parallels gate still did not reach the required 5/5 proof.

## Self-Audit

- F32e/F32f wakequeue semantics were not weakened.
- No timer-driven wake, arbitrary polling interval, SEV/WFE substitution, or CPU
  routing workaround was added.
- No Tier 1 files were modified.
- Tier 2 changes were limited to `context_switch.rs` and `gic.rs`.
- No serial breadcrumbs were added to IRQ/syscall/idle paths.
- The GIC fix is one redistributor configuration fix, not a rewrite.
- No PR or merge was attempted after the validation stop.
