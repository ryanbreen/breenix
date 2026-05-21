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
