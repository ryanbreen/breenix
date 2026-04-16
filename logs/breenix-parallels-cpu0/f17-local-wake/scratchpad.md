# F17 Scratchpad

## 2026-04-16 — Setup

Starting from exact requested base `4bd74caa` in worktree
`/Users/wrb/fun/code/breenix-worktrees/f17-local-wake` on branch
`diagnostic-fix/f17-local-wake`.

The primary checkout is dirty on an unrelated branch, so this run is isolated
in a worktree to avoid overwriting unrelated local changes.

Read:
- F16 exit verdict from `breenix-worktrees/f16-idle-scan-fix/.../exit.md`
- F16 scheduler audit from the branch history/input artifacts
- `scheduler.rs`, `exception.rs`, `context_switch.rs`, AHCI ring code
- Linux v6.8 `ttwu_queue`, `sched_ttwu_pending`, and wake-list comments

Initial audit:
- `isr_unblock_for_io()` writes to `ISR_WAKEUP_BUFFERS[cpu]`, then calls
  `set_need_resched()`.
- `schedule()` and `schedule_deferred_requeue()` drain all ISR wake buffers at
  the top of the scheduling decision.
- `handle_irq()` itself calls `check_need_resched_on_irq_exit()`, but the real
  `check_need_resched_and_switch_arm64()` call is in `boot.S` after
  `handle_irq()` returns.
- The suspected ordering is therefore: AHCI IRQ handler pushes wake buffer,
  sets need_resched, `boot.S` calls `check_need_resched_and_switch_arm64()`,
  then `schedule_deferred_requeue()` should drain and select the woken task.

Next: add AHCI-ring breadcrumbs only; no GIC changes, no broadcast scan.

## 2026-04-16 — Breadcrumb Patch

Added AHCI-ring sites:
- `TTWU_LOCAL_ENTRY`
- `TTWU_LOCAL_SET_RESCHED`
- `IRQ_TAIL_CHECK_RESCHED`
- `RESCHED_CHECK_ENTRY`
- `RESCHED_CHECK_DRAINED_WAKE`
- `RESCHED_CHECK_SWITCHED`
- `RESCHED_CHECK_RETURN`

Encoding:
- ring `cpu_id` remains the actual emitting CPU from MPIDR
- `slot_mask` stores current CPU wake-buffer depth, or ready-queue length for
  `RESCHED_CHECK_DRAINED_WAKE`
- `token` stores current `need_resched` value at the breadcrumb point
- `waiter_tid` stores the target/woken tid where meaningful; for drain it stores
  the drained wake count

Next: build and commit breadcrumbs, then run one Parallels probe to classify
H1/H2/other before applying a fix.

## 2026-04-16 — Diagnostic Classification

`diagnostic-run2` preserved the local wake sequence:

- AHCI ISR on CPU0 for port1 command token 1241
- `TTWU_LOCAL_ENTRY` tid=11 depth=0
- `WAKEBUF_AFTER_PUSH`
- `TTWU_LOCAL_SET_RESCHED` tid=11 depth=1 need=1
- `IRQ_TAIL_CHECK_RESCHED` depth=1 need=1
- `RESCHED_CHECK_ENTRY` depth=1 need=1
- `RESCHED_CHECK_DRAINED_WAKE` drained=1 ready_queue_len=1
- `RESCHED_CHECK_SWITCHED` new_tid=11

Determination:
- H1 is not confirmed: IRQ return does call the scheduler check.
- H2 is not confirmed: wake buffer is drained and the scheduler switches to
  the woken task.
- The remaining timeout is a later AHCI command (`cmd#=1243`) with
  `completion_done=0`, while the last completed command in the ISR ring is
  token 1241. That points outside the local wake ordering and back toward the
  still-active SPI34/AHCI interrupt state, which this F17 contract forbids
  fixing by touching GIC code.

Next: run the required 5x sweep and document FAIL/other with F18 direction.

## Final 5-run sweep

- run1: PASS criteria met; bsshd=1, ahci_timeouts=0, corruption=0.
- run2: PASS criteria met; bsshd=1, ahci_timeouts=0, corruption=0.
- run3: FAIL; bsshd=1, ahci_timeouts=2, failed exec of bsh with EIO, soft lockup dumps, corruption=0.
- run4: PASS criteria met; bsshd=1, ahci_timeouts=0, corruption=0.
- run5: PASS criteria met; bsshd=1, ahci_timeouts=0, corruption=0.

F17 determination remains Other. H1 and H2 are falsified by the preserved sequence: TTWU local entry -> wake buffer push -> IRQ tail check with need_resched=1 and depth=1 -> resched check entry -> drain wake -> switch to tid 11. The later timeout is for a later AHCI command while SPI34 is pending+active and Port1 IS=0x1.
