# Polling Allowlist

This document formalizes the **Linux-rigor polling-elimination gate** for cases where a bounded spin is the architecturally-correct primitive (hardware settle, register handshake) rather than event polling that should be converted to IRQ-driven completion.

**Policy:** Allowlisted spins MUST:
1. Be bounded by a maximum iteration count (no infinite spin)
2. Be on a hardware-handshake or hardware-settle code path (not event polling)
3. Have a Linux precedent (Linux uses an equivalent bounded primitive like `udelay()`, `msleep()`, or `readl_poll_timeout()`)
4. Be documented inline with a comment referencing this allowlist

**Allowlisted sites:**

## P15: PCI PM D3hot→D0 settle delay

- **File:** `kernel/src/drivers/pci.rs:551-554` (in `Pci::set_power_state_d0()`)
- **Loop:** `for _ in 0..10_000_000u64 { core::hint::spin_loop(); }`
- **Justification:** PCI spec PM 3.0 §5.4.2 requires a 10ms delay after the D3hot→D0 power-state transition before any device access. This is a hardware-settle delay (the device's internal state machine needs time to re-power), not event polling.
- **Linux precedent:** `drivers/pci/pci.c::pci_set_power_state()` calls `msleep(pci_pm_d3hot_delay)` (default 10ms) after the same transition. Breenix's bounded spin is functionally equivalent; Linux's `msleep()` yields to scheduler, Breenix's `spin_loop` is appropriate at this stage because PCI PM transitions happen during early boot/device probe when scheduler may not be available or device access must serialize with this single CPU.
- **Bounded:** 10M iterations on aarch64 = ~10ms at 1 GHz. Safe upper bound.
- **Frequency:** Once per PCI device that needs PM transition (boot only).
- **Status:** ALLOWLISTED — not subject to polling-elimination conversion.

## P11: VirtIO reset status handshake

- **File:** `kernel/src/drivers/virtio/mod.rs:240-260` (in `VirtioDevice::init()`)
- **Loop:** Outer `loop` polling `read_status()` until 0 with inner `for _ in 0..10000 { spin_loop }` delay; bounded by `reset_attempts >= 100`.
- **Justification:** VirtIO spec §3.1.1 ("Driver Initialization") requires the driver to reset the device and wait for the device to indicate completion by setting `Device Status` to 0. This is a hardware handshake (the device's internal state machine takes a bounded time to clear), not event polling.
- **Linux precedent:** `drivers/virtio/virtio_pci_modern.c::vp_modern_reset()` writes `Device Status = 0` and then loops on `cpu_relax()` reading the same register until it returns 0. Functionally equivalent to Breenix's `spin_loop` pattern. Linux's `vp_modern_reset` is also bounded — it relies on the device behaving correctly per spec.
- **Bounded:** 100 attempts × 10000 spin_loop iterations × ~1ns/iter ≈ 1ms total maximum on aarch64. Safe upper bound for a hardware reset handshake.
- **Frequency:** Once per VirtIO device at driver init (boot only).
- **Status:** ALLOWLISTED — not subject to polling-elimination conversion.

## P16: GICR_WAKER ProcessorSleep / ChildrenAsleep handshake

- **File:** `kernel/src/arch_impl/aarch64/gic.rs:1418-1424` (in `init_gicv3_redistributor()`)
- **Loop:** Bounded `for _ in 0..10_000` polling `GICR_WAKER` for `ChildrenAsleep` (bit[2]) to clear.
- **Justification:** GICv3 spec requires the driver to clear `ProcessorSleep` (bit[1]) and then wait for `ChildrenAsleep` (bit[2]) to clear before the redistributor is usable. This is a CPU-management handshake (the GIC's internal state machine takes bounded time to wake), NOT event polling.
- **Linux precedent:** `drivers/irqchip/irq-gic-v3.c::gic_redist_wait_for_rwp()` polls `GICR_CTLR.RWP` and `GICR_WAKER.ChildrenAsleep` with `cpu_relax()` in equivalent bounded loops. Breenix's `spin_loop` is functionally equivalent.
- **Bounded:** 10,000 iterations × ~1ns/iter ≈ 10µs maximum on aarch64. Safe upper bound for a per-CPU GIC redistributor wake handshake.
- **Frequency:** Once per CPU at boot (`init_gicv3_redistributor` is called per-CPU).
- **Status:** ALLOWLISTED — not subject to polling-elimination conversion.
- **Note:** Location is in a Tier-2 file (`gic.rs`). The inline comment is placed BEFORE the GICR_WAKER spin, OUTSIDE the gold-master SGI enable block (which is later in the same function at the `GICR_ISENABLER0` write). Gold-master constraint preserved.

## P18: Completion::wait_timeout() early-boot polling fallback

- **File:** `kernel/src/task/completion.rs:415-446+` (the `else` branch in `Completion::wait_timeout()` taken when `current_thread_id()` returns `None`)
- **Loop:** Bounded spin on `self.done.load(Acquire) == 0`, exits on `done` set OR CNTPCT deadline exceeded.
- **Justification:** Used ONLY in early boot before the scheduler exists. The IRQ-driven wait-queue path requires `current_thread_id()` to return a thread to park; early boot has no such thread. Linux's equivalent: kernel pre-scheduler-init phase uses `mdelay()`/`udelay()`-style busy-spin for similar handshakes (e.g., serial port readiness, ACPI events) — there is no architectural alternative until threads exist.
- **Linux precedent:** Linux completions (`wait_for_completion_timeout()`) require the scheduler. Pre-scheduler boot phase in Linux uses bounded busy-wait primitives. Breenix's fallback is the same pattern.
- **Bounded:** CNTPCT deadline (matching the interrupt-path's deadline). The caller passes `timeout_ns` which sets the upper bound — typically milliseconds to seconds at most.
- **Frequency:** Limited to early boot (before scheduler is up). Once Breenix's scheduler initializes, `current_thread_id()` returns `Some(tid)` and the IRQ-driven path is taken.
- **Status:** ALLOWLISTED — not subject to polling-elimination conversion. Architecturally necessary fallback for pre-scheduler boot.

## P17: SMP secondary CPU online wait

- **File:** `kernel/src/main_aarch64.rs` (boot-time SMP bring-up wait after PSCI CPU_ON)
- **Loop:** A bounded `loop { ... core::hint::spin_loop(); }` exits immediately once `cpus_online() >= expected` and otherwise observes both the online count and the sum of monotonic per-CPU bring-up stages. The stage sum is sampled every 4,096 iterations while the online count is unchanged; each per-CPU stage occupies its own 64-byte cache line, so diagnostics neither scan all stages on every spin nor make concurrent secondary publishers falsely share a line. An advance of either signal re-arms the no-progress window, and the no-progress verdict always takes a fresh stage sample before failing. Once per second, each still-offline CPU gets a stage-number/name breadcrumb; final diagnostics add its last PSCI status and whether its stage advanced during this wait.
- **Justification:** Boot CPU waits for secondary CPUs to come online after issuing PSCI CPU_ON requests. The secondary CPUs increment `cpus_online` once they reach their entry point. Bounded CPU-management handshake (NOT event polling) — there is no IRQ available for "CPU now online" because the GIC distributor isn't fully wired across CPUs until each is up.
- **Linux precedent:** `kernel/smp.c::__cpu_up()` uses `wait_for_completion_timeout()` for the equivalent transition — scheduler-backed wait that blocks until the secondary CPU sets its online state. Linux's wait is functionally a bounded busy-equivalent (scheduler may park the boot CPU, but the wait itself is on a completion that the secondary CPU triggers). Breenix's busy-spin is appropriate here because the scheduler is partially up at this stage and a CPU-management wait on this specific path doesn't benefit from yielding.
- **Bounded:** In `boot_tests` builds, twenty seconds of CNTVCT time without an online-count or bring-up-stage advance ends the wait; real secondary progress re-arms that window, and a separate forty-second ceiling runs from the wait's start. That local ceiling is also capped by the same 65-second liveness budget used by P20, anchored near kernel entry before initialization, SMP release, or boot tests. Consequently P17 and P20 together can consume at most 65 seconds, leaving 90s - 65s = 25s of the full-test script's Phase-1 deadline for loader/process overhead, kernel initialization, the other subsystem tests, and marker polling. A pathological full forty-second P17 wait leaves P20 no more than twenty-five seconds because earlier initialization also consumes the shared clock; that is the deliberate last-resort backstop. Starvation testing measured roughly eight seconds for the longest individual wait and at most ten seconds from the early shared anchor. Plain kernels use a two-second no-progress window and four-second absolute ceiling. Those values remain twenty times the former 100ms allowance while preserving at least sixteen seconds of the strict harness for the rest of boot and final diagnostics. An unconditional delta check every 10,000,000 loop iterations ends the wait with a distinct `[smp] CNTVCT stalled` diagnostic if the counter made no advance between samples, so a fully frozen time source cannot make the spin unbounded. Deadline deltas use a shared forward-only helper; a backwards CNTVCT read contributes zero elapsed time rather than becoming a huge unsigned timeout. A zero `CNTFRQ_EL0` reading uses the lowest plausible 1MHz fallback and therefore errs toward shorter deadlines. A missing shared anchor ends the boot-test wait immediately. The loop exits as soon as all expected CPUs are online.
- **Frequency:** Once at boot, after PSCI CPU_ON broadcast.
- **Status:** ALLOWLISTED — not subject to polling-elimination conversion.

## P19: PSCI CPU_ON transient-failure retry backoff

- **File:** `kernel/src/arch_impl/aarch64/smp.rs` (`release_cpu()` and `psci_cpu_on_retry_backoff()`)
- **Loop:** At most four CPU_ON attempts. Each attempt preserves the existing HVC64 → HVC32 conduit order; the attempt is retryable when either conduit returns transient `DENIED` or `INTERNAL_FAILURE`, even if the other conduit returns a permanent status such as `NOT_SUPPORTED`. Permanent failures from both conduits end the probe immediately. The 500µs inter-attempt backoff is bounded by both CNTVCT and a 1,000,000-iteration safety cap.
- **Justification:** A transient firmware/hypervisor failure should not permanently reduce the expected CPU count. `SUCCESS` and `ON_PENDING` are accepted; `ON_PENDING` is left to P17's separately bounded online wait. `ALREADY_ON` is returned distinctly and is not counted as a new launch because only Breenix's per-CPU online publication proves that the expected secondary entry path is progressing. Permanent topology/probe errors are returned without retry, and SMC remains excluded.
- **Bounded:** At most four attempts and three 500µs backoffs per CPU. A zero `CNTFRQ_EL0` reading uses the same conservative 1MHz fallback as P17. The last raw status is retained per CPU for P17 timeout diagnostics.
- **Frequency:** Once per probed secondary CPU during boot.
- **Status:** ALLOWLISTED — bounded CPU-management retry.

## P20: Exit-kick boot-test cross-CPU waits

- **File:** `kernel/src/tracing/providers/teardown.rs` (`spin_with_resched()`, `join_with_resched()`, and their existing coordinator-owned call sites)
- **Loop:** The boot-test coordinator polls reservation, completion, worker-ready, and kthread-exit conditions. No wait was moved into a publisher or observer closure.
- **Progress bound:** Each wait has an unconditional eight-second floor from its own entry plus a three-second no-further-progress deadline from the most recent observed advance. The effective no-progress deadline is `min(max(wait_start + 8s, last_advance + 3s), gate_start + 45s, phase_liveness_start + 65s)`, so one early advance in a multi-worker union cannot shorten an unseen sibling's first-schedule allowance and no recovery can extend either aggregate clock. Only genuine worker-owned work/publish transitions or lock-free per-TID exit-stage counters contribute; CPU timer ticks and polling iterations do not. The `workers_ready == 3` wait records all three worker counters individually and uses their saturating union. The observer counter advances only when it starts running, claims a real publication, observes the monotonic publisher-completion state change, or publishes its own completion. Publisher A's join uses the union of publisher A and its observer dependency while A can be blocked on that observer; the 15-second per-wait ceiling still independently proves that A joins. Other storm joins use the awaited worker's own counter. The exit-stage terminal increment occurs after the kthread's `exited` store; the first exit-stage increment occurs before affinity is cleared.
- **Hard bounds and recovery:** Every wait has a distinct 15-second absolute ceiling, and the entire gate invocation has a separate 45-second ceiling measured once before gate work begins. The gate also consumes P17's 65-second Phase-1 liveness budget, whose anchor is published once near kernel entry and is never re-armed. Each wait checks all three bounds, kicks every CPU in its dependency set, and resends the reschedule SGI about every 50ms. Gate entry, wait entry, and every re-kick also advance a boot-test-only relaxed-atomic heartbeat consumed by CPU 0's soft-lockup detector. That prevents the generic five-second detector from preempting the gate's worker-specific eight-second verdict without counting timer ticks as worker progress or changing production watchdog behavior. Every 100,000 coordinator iterations, an unconditional delta check fails if CNTVCT made zero progress. Deadline deltas use the shared forward-only helper, so a backwards read contributes zero elapsed time rather than triggering an immediate ceiling. If `CNTFRQ_EL0` is zero, or if the shared anchor is unavailable, the gate fails immediately instead of guessing or starting a fresh budget. The condition is re-read at a verdict; only a no-progress verdict that raced a late-true condition returns success. Per-wait, gate, Phase-1, and counter failures remain failures.
- **Diagnostics:** At a verdict, one line records wait/cause, elapsed time, effective deadline budget, re-kick count, online CPUs, current condition, aggregate progress start/final values, the three individual worker-progress start/final values, and time since last progress. For a no-progress verdict, `window_budget_ms` is the effective deadline measured from wait entry (at least eight seconds and possibly later after progress); it is 15 seconds for per-wait ceiling expiry, 45 seconds for aggregate gate expiry, 65 seconds measured from the shared kernel-entry anchor for Phase-1 ceiling expiry, and zero for counter/join failures. Per-site no-progress and absolute-wait failures retain the CPU-specific `exit_kick_gate: ... unresponsive` permanent message. Gate-ceiling, shared Phase-1-ceiling, counter-frequency, CNTVCT-stall, exit-progress, and final-join failures use cause-specific non-`unresponsive` permanent messages and therefore cannot be misclassified by the canonical grep. Every wait lasting at least three seconds emits at most one breadcrumb per second.
- **Aggregate boundedness:** P17 and P20 share one 65-second ceiling: `min(P17 local 40s + P20 local 45s, shared 65s) = 65s`, leaving `90s - 65s = 25s` for the rest of Phase 1. A gate whose condition remains false at its local ceiling emits `cause=gate_ceiling` and a permanent gate-budget message that explicitly disclaims a per-CPU stall. One that exhausts the shared remainder emits `cause=phase_one_ceiling` and the analogous shared-Phase-1-budget message. Neither matches `exit_kick_gate:.*unresponsive` or names an innocent worker CPU.
- **Linux precedent:** Linux completion waits pair progress-sensitive wakeups with timeout and retry diagnostics; this boot-test-only poll provides the same bounded liveness property where blocking `kthread_join()` cannot produce an actionable failure.
- **Status:** ALLOWLISTED — boot-test-only bounded cross-CPU handshake.

## P12: AHCI engine + taskfile bounded register handshakes (Sites 3, 4, 5)

- **File:** `kernel/src/drivers/ahci/mod.rs:1075-1106` (Site 3: `PORT_CMD.CR` / `PORT_CMD.FR` command-engine state handshakes in `stop_cmd()` / `start_cmd()`)
- **File:** `kernel/src/drivers/ahci/mod.rs:1119-1128` (Site 4: `wait_ready()` taskfile `BSY` / `DRQ` readiness handshake)
- **File:** `kernel/src/drivers/ahci/mod.rs:2125-2132` (Site 5: platform IRQ probe taskfile `BSY` / `DRQ` readiness handshake)
- **Loop:** Bounded `for` loops polling AHCI/ATA register state bits with `core::hint::spin_loop()` as the `cpu_relax` equivalent.
- **Justification:** These are hardware register-state handshakes required by AHCI/ATA sequencing, not event polling for future completions. The command engine must report `CR`/`FR` clear during stop/start transitions, and the taskfile must report `BSY`/`DRQ` clear before command issue/probe. There is no scheduler-backed alternative during AHCI engine reset, initialization, and platform probe paths.
- **Linux precedent:** `drivers/ata/libahci.c::ahci_stop_engine()` clears the AHCI engine start bit and uses `ata_wait_register()` for the equivalent command-engine state transition. `drivers/ata/libata-core.c::ata_wait_after_reset()` and `ata_wait_ready()` perform bounded ATA device/taskfile readiness waits. See `docs/artifacts/turn53-artifacts/p12-ahci-classification.md` for the full classification and Linux reference notes.
- **Bounded:** Site 3 and Site 4 cap at 1,000,000 iterations per wait. Site 5 caps at 100,000 iterations.
- **Frequency:** Boot/init and platform IRQ probing; Site 4 can also run before runtime command issue as a required device readiness handshake.
- **Status:** ALLOWLISTED — first P12 batch; not subject to polling-elimination conversion.

## P12-Site-2: AHCI early-boot PORT_CI command-completion fallback (P18 analog)

- **File:** `kernel/src/drivers/ahci/mod.rs:815-853` (fallback branch in `wait_cmd_slot0()` when scheduler-backed waiting is unavailable)
- **Loop:** Bounded `loop` polling `PORT_CI` until slot 0 clears, exits on completion, taskfile error, or CNTPCT deadline.
- **Justification:** This is the AHCI-specific analog of P18's `Completion::wait_timeout()` early-boot fallback (`kernel/src/task/completion.rs:415-446`). Runtime AHCI command completion uses the scheduler-backed `Completion::wait_timeout()` path when a thread can park; this branch is only used before that path is available during pre-scheduler boot.
- **Linux precedent:** Runtime AHCI command completion is interrupt-driven through libata/AHCI completions such as `drivers/ata/libahci.c::ahci_port_intr`. Linux pre-scheduler and polling-mode paths use bounded polling primitives when no thread exists to park, matching the P18 fallback pattern.
- **Bounded:** CNTPCT deadline `start + freq * AHCI_TIMEOUT_SECS`, with timeout dumping state and returning `AHCI: command timeout`.
- **Frequency:** Early boot/pre-scheduler fallback only; runtime command completion remains IRQ-driven and scheduler-backed.
- **Status:** ALLOWLISTED — AHCI-specific P18 analog; not subject to polling-elimination conversion.

## P12-Site-7: AHCI ISR PORT_IS/PORT_CI drain loop

- **File:** `kernel/src/drivers/ahci/mod.rs:2413-2466` (ISR drain loop in `handle_irq()` / AHCI interrupt handling)
- **Loop:** Bounded `loop` drains already-observed `PORT_IS` and tracked `PORT_CI` completion state until both are stable.
- **Justification:** This is interrupt-context stabilization, not a wait for a future external event. The loop acknowledges observed `PORT_IS`, detects completed tracked slots, then rechecks `PORT_IS`/`PORT_CI` so a waiter is not woken while the wired level interrupt remains asserted.
- **Linux precedent:** `drivers/ata/libahci.c::ahci_port_intr()` reads and clears `PORT_IRQ_STAT`; `ahci_handle_port_interrupt()` then completes queued commands via `ahci_qc_complete()`.
- **Bounded:** `AHCI_CI_COMPLETION_LOOP_LIMIT` caps the drain at 8 iterations.
- **Frequency:** Runtime AHCI interrupt handling.
- **Status:** ALLOWLISTED — interrupt-context site with deliberately minimal inline comment; not subject to polling-elimination conversion.
