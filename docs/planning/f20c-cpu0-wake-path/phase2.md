# F20c Phase 2: Fix Outcome

## Outcome

No Phase 2 fix was committed.

Phase 1 confirmed that CPU 0's timer counter freezes while the other CPUs keep
receiving PPI27, but the observed `main` `--test 45` path did not produce a
CPU0 idle-loop baseline: CPU 0 never updated `idle_arm_tick[0]` or
`post_wfi_count[0]`. That means a WFI-specific fix would be speculative on this
branch.

The most obvious H2 fix direction is also already present in Breenix:

- `timer_interrupt::arm_timer()` programs `CNTV_CVAL_EL0 = CNTVCT + ticks`,
  writes `CNTV_CTL_EL0 = 1`, and issues `isb`.
- `idle_loop_arm64()` already re-arms the current CPU timer before WFI.

Linux comparison points:

- Linux ARM64 idle is minimal: `dsb(sy); wfi();` through `cpu_do_idle()` and
  `arch_cpu_idle()` re-enables local IRQs after idle return.
- Linux arch timer `set_next_event()` enables the timer, clears the interrupt
  mask bit, programs the next event, and writes the control register. Its
  erratum path also supports writing `CNTV_CVAL_EL0 = evt + CNTVCT`.

Sources:

- https://android.googlesource.com/kernel/common/+/0ddc717be4a9dcb42be08ea86796227732fac656/arch/arm64/kernel/process.c
- https://android.googlesource.com/kernel/common/+/e07095c9bbcd296401bec8b6852d258d7c926969/drivers/clocksource/arm_arch_timer.c

## Why No Fix

The data supports "CPU0 PPI27 delivery stops" but not a single safe edit outside
the prohibited paths:

```text
[END_OF_BOOT_AUDIT] tick_count=[9,32107,32107,32109,32112,32117,32107,32138]
[END_OF_BOOT_AUDIT] idle_arm_tick=[0,32090,32107,32101,32107,32113,32101,32133]
[END_OF_BOOT_AUDIT] post_wfi_count=[0,293,300,342,301,263,291,262]
[END_OF_BOOT_AUDIT] timer_ctl=[0x1,0x1,0x1,0x1,0x1,0x1,0x1,0x1]
```

At the AHCI timeout, CPU0's PPI was enabled and pending, `CNTV_CTL_EL0` was
enabled/unmasked, and CPU0's timer deadline was more than 10 seconds expired:

```text
[ahci]   CPU0_GICR_ISENABLER0=0x08000000 PPI27_enabled=1
[ahci]   CPU0_GICR_ISPENDR0=0x08000002 PPI27_pending=1
[ahci]   tick_count=[9,8215,8214,8216,8213,8215,8215,8244]
[ahci]   cpu0_timer: cval=56762305 cntvct_at_arm=56738306 cntvct_now=302180869 delta=245418564 (10225ms)
```

Changing timer interrupt, exception, syscall entry, or GIC code is explicitly
out of scope for F20c, and the current non-hot-path idle re-arm already matches
the suggested Linux-style next-event direction. A fix commit here would be a
guess rather than an evidence-backed correction.

## F20d Probe Recommendation

Add a one-shot, non-timer diagnostic kthread on a nonzero CPU that waits until
`TIMER_TICK_COUNT[0] <= 10` while `TIMER_TICK_COUNT[1] > 1000`, then sends a
single SGI to CPU 0 using the existing GIC send path and records whether CPU 0
takes any interrupt afterward.

Recommended outputs:

- Before SGI: CPU0 tick count, `CPU0_BREADCRUMB_ID`, `CPU0_LAST_TIMER_ELR`,
  `CNTV_CTL`, and `GICR_ISPENDR0`.
- After SGI: the same fields plus a one-shot CPU0-side breadcrumb from the SGI
  handler if an existing handler can be reused without editing prohibited files.

Interpretation:

- If SGI wakes CPU0 and timer ticks resume, the bug is specific to virtual timer
  PPI delivery/ack state after early AHCI activity.
- If SGI does not wake CPU0, the problem is broader than PPI27 and the next step
  should be a GDB stop-all inspection of CPU0's live PC, PSTATE/DAIF, and stack.
