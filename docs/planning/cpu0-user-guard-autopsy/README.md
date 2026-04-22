# CPU0 User-Guard Autopsy

**Status: CLOSED. Fix landed in PR #334 (commit `9da897f4`, 2026-04-22).**

This document is the definitive post-mortem on the "CPU0 regression" that
burned approximately one week of engineering time across factories F32i,
F32j, F33, F34, and several ad-hoc investigations in early- to mid-April
2026. Read this *first* before touching any CPU0-adjacent code.

---

## TL;DR

The bug was not an HVF/Parallels vtimer behavior. It was a
**self-referential requeue loop** in a CPU0-specific dispatch guard inside
`kernel/src/arch_impl/aarch64/context_switch.rs`. Every theory about "HVF
kills CPU0 vtimer when guest ERETs to EL0" was empirically false.

Removing the guard produced a fully healthy CPU0 on the same Parallels
hypervisor, on the same commit, in the same VM configuration. CPU0 now runs
EL0 indistinguishably from CPUs 1-7.

If you ever add another CPU0-specific EL0 dispatch guard, re-read this
document first.

---

## The symptom

On any fresh Parallels boot with SMP > 1 CPU online (F29 merge onward):

- CPU0's `TIMER_TICK_COUNT[0]` would advance to ~10-300 then flat-line.
- Other CPUs' counters would reach tens of thousands over the same interval.
- Symptoms that follow:
  - Cursor doesn't move (HID polling runs from CPU0 timer ISR; dead CPU0 timer ⇒ stale `mouse_pos`).
  - `[timer] cpu0 ticks=5000` serial marker never appears.
  - Global tick counter (`crate::time::get_ticks()`) stops advancing.
  - If a fork/exec landed the spawned thread on CPU0, it would loop forever
    without reaching userspace.
  - AHCI commands targeting CPU0 for completion work could stall.

The T0-T9 serial breadcrumbs (raw UART writes for the first 10 CPU0 timer
ticks) always fired, leading to repeated false conclusions that CPU0 timer
was "dying at tick 10." **Those markers only print when `count <= 10` —
their absence beyond tick 10 tells us nothing about timer health.** Many
investigations were derailed by this misreading.

---

## The root cause

`context_switch.rs` contained a block that fired during dispatch of any EL0
candidate whenever the current CPU was CPU0 and SMP had > 1 CPU online:

```rust
if cpu_id == 0 && crate::arch_impl::aarch64::smp::cpus_online() > 1 {
    // ... trace ...
    trace_dispatch_redirect(thread_id, TRACE_REDIRECT_CPU0_USER_GUARD);
    if let Some(thread) = sched.get_thread_mut(thread_id) {
        thread.state = ThreadState::Ready;
    }
    setup_idle_return_locked(sched, frame, cpu_id);
    let idle_id = sched.cpu_state[cpu_id].idle_thread;
    sched.cpu_state[cpu_id].current_thread = Some(idle_id);
    // Requeue the thread being dispatched to a non-CPU0 queue.   <-- LIE
    sched.requeue_thread_after_save(thread_id);
    sched.set_need_resched_inner();
    // ... self-IPI ...
    return;
}
```

The comment **lied**: `requeue_thread_after_save()` does not route the
thread to a non-CPU0 queue. It places the thread back on CPU0's own ready
queue. On the next scheduler iteration CPU0 picks the same thread, hits
the guard, requeues, loops. The loop runs at roughly 24 kHz per CPU0 timer
tick. Timer interrupts still fire; they just feed the spin. CPU0 never
makes forward progress; userspace never runs on CPU0.

The guard's stated justification:

> "On Parallels/HVF, ERET to EL0 kills CPU 0's vtimer PPI 27 delivery.
> Without the timer, CPU 0 cannot preempt and any thread dispatched here
> monopolises the CPU. Redirect to idle and requeue the thread on CPUs 1-7
> where the timer works."

This claim is **empirically unsupported** as of PR #334. With the guard
removed, CPU0 ERETs to EL0 and the vtimer keeps firing indefinitely.

---

## The definitive evidence

From `cpu0-trace-dump-probe` branch (30s uptime, one-shot `dump_all_buffers()`
triggered from any non-CPU0 timer ISR):

**Per-CPU tick_count snapshot at 30s on unmodified `main` (pre-fix):**
```
[probe] tick_count per cpu: 372 29676 29679 29091 30000 29551 29549 28341
                            ^CPU0^CPU1 ...                        ^CPU7
```

CPU0's trace buffer (1024 events) showed a single repeating pattern:

```
CPU0 USER_DISPATCH_STAGE  stage=1 tid=11
CPU0 USER_DISPATCH_ELR
CPU0 USER_DISPATCH_SPSR
CPU0 USER_DISPATCH_TTBR0
CPU0 USER_DISPATCH_STAGE  stage=3 tid=11
CPU0 USER_DISPATCH_ELR
CPU0 USER_DISPATCH_SPSR
CPU0 USER_DISPATCH_TTBR0
CPU0 DISPATCH_REDIRECT   reason=6 (TRACE_REDIRECT_CPU0_USER_GUARD), tid=11
CPU0 CTX_SWITCH_ENTRY    new_tid=11
CPU0 DEFER_REQUEUE_STAGE tid=11
... (loop, ~42µs per iteration)
```

Never breaks out. Never reaches EL0. No anomaly in the trace is consistent
with vtimer masking — the dispatch returns immediately via the guard, not
through any ERET that could interact with HVF.

**Per-CPU tick_count snapshot at 30s on `9da897f4` (post-fix):**
```
[probe] tick_count per cpu: 32855 29639 29649 29510 29120 30000 29745 28209
                            ^CPU0
```

CPU0 is now **ahead of** every sibling. `cpu0 ticks=5000, 10000, ..., 65000`
markers continue at normal cadence. Cursor tracks mouse. bsshd, bounce, and
every other process spawn normally.

---

## Why it took a week

The guard's comment confidently asserted "HVF kills CPU0 vtimer on ERET to
EL0." Every factory that investigated used that as a premise rather than
treating it as a hypothesis.

Theories chased and rejected:

| Hypothesis | Factory | Why it was wrong |
|---|---|---|
| ISB before ERET missing | F31, earlier | Already in place at dispatch site; not the issue |
| DAIF mask uses `#0xf` instead of Linux's `#3` | ad-hoc | Cosmetic divergence, no effect |
| Idle-loop DAIF state inconsistency (A+D masked) | ad-hoc | Not the cause |
| `return_to_userspace` missing ISB | ad-hoc | Not the cause; never affected tick count |
| Idle-loop `rearm_timer()` removed by F20e | ad-hoc | Irrelevant; handler-level re-arm still works |
| PCI MSI programming order | F32t | Fixed a real bug, but unrelated to CPU0 |
| xHCI state.irq = 0 | F32p/n | Unrelated sub-bug |
| HVF vtimer death on IMASK transition | F31 | Empirically fine |
| SGI admission (ISPENDR without ISENABLER) | F32i/F32j | Fixed a real bug but did not cause CPU0 regression |
| F32j idle gate spin | F32o | Real bug (caused 800% CPU), but not CPU0 timer death |
| Per-CPU `need_resched` divergence | F32q/F32r | Unrelated |
| PCI MSI order breaks CPU0 | F32t | No, unrelated |
| `ret-based` idle dispatch bypasses HVF-required ERET ISB | F34 | Could not reproduce in its environment |

The actual answer — "the guard has a bug where `requeue_thread_after_save`
puts threads back on CPU0's own queue" — was never considered until the
cpu0-trace-dump-probe (2026-04-22) forced a look at CPU0's own trace buffer
at 30s uptime and revealed the infinite loop.

---

## Detection

Boot-time metric that reliably identifies the regression within 30 seconds:

```
[probe] tick_count per cpu: [CPU0] [CPU1] [CPU2] [CPU3] [CPU4] [CPU5] [CPU6] [CPU7]
```

Healthy: all values roughly uniform, no value < 10% of the max.

Regressed: CPU0 value is 1-2 orders of magnitude below every other CPU.

A `panic!` alarm now runs at 30 seconds of uptime and fails boot if CPU0
is < 10% of the max. Any future regression will surface as a deterministic
boot failure rather than a cascade of "cursor doesn't work" symptoms.

See `kernel/src/arch_impl/aarch64/timer_interrupt.rs` for the alarm
implementation.

---

## Commit timeline

| Commit | Date | Role |
|---|---|---|
| (guard introduced) | ~2026-03-xx | Author believed HVF kills CPU0 vtimer on EL0. Wrote guard. |
| `68dc0be6` | 2026-03-xx | Added IMASK-based timer protocol — legitimate fix for a real issue |
| `26bfcea9` | 2026-03-xx | IMASK=1 before first arm — legitimate HVF protocol handshake |
| `e4e16b68` | 2026-03-29 | arm_timer at top of handler — legitimate fix for IMASK-death if handler hangs |
| `aeb3e989` | 2026-03-29 | ISB before ERET — added at 6 sites |
| `aade0871` | 2026-03-29 | Narrowed to dispatch site only — ISBs at IRQ/syscall return caused separate timer death at ~10K ticks |
| `66ecc316` (F29) | 2026-04-18 | Re-enabled Parallels SMP bringup. After this, guard actively fired and began causing the loop. |
| `bff1d92a` (F20e) | 2026-04-17 | DSB before idle WFI — legitimate |
| `946b2812` (F32j) | 2026-04-19 | GIC `ISENABLER` for SGI_RESCHEDULE + SGI_TIMER_REARM — legitimate fix |
| **`9da897f4`** (PR #334) | **2026-04-22** | **Guard removed. CPU0 heals.** |

---

## Rules for the future

If you believe CPU0 needs special handling that CPUs 1-7 do not:

1. **Reproduce the problem with evidence**, not with a theory. Use
   `cpu0-trace-dump-probe` (or re-port it) to get a 30-second trace buffer.
   Per-CPU `tick_count` parity is the load-bearing signal.

2. **Verify on the Linux probe VM** (10.211.55.3). Linux runs on the same
   Parallels hypervisor. If Linux works without the behavior you think CPU0
   needs, Breenix can work without it too. No theory about HVF behavior is
   acceptable without this validation step.

3. **Inspect your proposed guard's requeue path.** If it calls
   `requeue_thread_after_save()` or anything that can land the thread on
   the guarded CPU's own ready queue, you will re-create this bug. The
   proposed path must demonstrably enqueue onto a **different** CPU's ready
   queue.

4. **Expect the detection alarm to fire** if you break this. The panic
   message will reference this document.

5. **PR signoff from the project owner is required** for any change to
   the gold-master code regions that reference this autopsy.

---

## Files of record

- This document: `docs/planning/cpu0-user-guard-autopsy/README.md`
- Fix commit: `9da897f4` (PR #334)
- Probe branch: `origin/cpu0-trace-dump-probe`
- Gold-master markers in:
  - `kernel/src/arch_impl/aarch64/context_switch.rs` (dispatch site)
  - `kernel/src/arch_impl/aarch64/context_switch.rs` (`idle_loop_arm64`)
  - `kernel/src/arch_impl/aarch64/gic.rs` (`init_gicv3_redistributor`
    SGI-enable block)
- Regression alarm: `kernel/src/arch_impl/aarch64/timer_interrupt.rs`
  (panic if CPU0 tick_count < 10% of max peer at 30s uptime)
