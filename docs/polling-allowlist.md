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
