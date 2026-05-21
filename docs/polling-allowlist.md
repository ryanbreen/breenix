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

- **File:** `kernel/src/main_aarch64.rs:967-976` (boot-time SMP bring-up wait after PSCI CPU_ON)
- **Loop:** `while kernel::arch_impl::aarch64::smp::cpus_online() < expected { ... core::hint::spin_loop(); }` with explicit timeout check.
- **Justification:** Boot CPU waits for secondary CPUs to come online after issuing PSCI CPU_ON requests. The secondary CPUs increment `cpus_online` once they reach their entry point. Bounded CPU-management handshake (NOT event polling) — there is no IRQ available for "CPU now online" because the GIC distributor isn't fully wired across CPUs until each is up.
- **Linux precedent:** `kernel/smp.c::__cpu_up()` uses `wait_for_completion_timeout()` for the equivalent transition — scheduler-backed wait that blocks until the secondary CPU sets its online state. Linux's wait is functionally a bounded busy-equivalent (scheduler may park the boot CPU, but the wait itself is on a completion that the secondary CPU triggers). Breenix's busy-spin is appropriate here because the scheduler is partially up at this stage and a CPU-management wait on this specific path doesn't benefit from yielding.
- **Bounded:** Explicit timeout check inside the loop exits with a `[smp] Timeout waiting for CPUs ...` message after a bounded wall-clock interval.
- **Frequency:** Once at boot, after PSCI CPU_ON broadcast.
- **Status:** ALLOWLISTED — not subject to polling-elimination conversion.
