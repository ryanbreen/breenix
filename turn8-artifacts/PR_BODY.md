# Summary

Restores interrupt-driven AHCI completion on AArch64 Parallels while preserving the narrow Turn 5 fix for level-triggered SPIs.

The key behavior change is in the AArch64 IRQ path: regular nested IRQ reopening is deferred for level-triggered SPIs until the device handler has serviced and acknowledged the level source. This matches Linux's `handle_fasteoi_irq` ordering and prevents Parallels/HVF from starving CPU0's virtual timer after AHCI completions.

# Root Cause

Parallels AHCI is wired to GIC SPI 34 as a level-triggered interrupt. Breenix was doing priority drop and reopening the regular nested IRQ window before the AHCI handler had cleared the level source. Under AHCI completion load, CPU0 could re-enter regular IRQ handling too early and eventually stop receiving healthy virtual timer progress.

Linux handles this differently: the IRQ handler runs the device action first, and only then performs the fasteoi tail. Turn 4's Linux ftrace/GIC comparison localized the ordering difference; Turn 5 applied the narrow Breenix fix.

# Fix Summary

- Restored Parallels AHCI to CPU0 interrupt-driven completion instead of falling back to polling.
- Deferred nested regular IRQ reopening for level-triggered SPIs while preserving the existing nested window for edge-triggered IRQs.
- Added memory-only AHCI polling attribution counters.
- Added a one-shot serial attribution line after the scheduler/timer path is ready:

```text
[ahci-poll-attrib] total=72 post_reg=70 pre_sched=70 post_sched=0
```

The runtime success criterion is `post_sched == 0`: no AHCI polling after the kernel can sleep on interrupt-driven completions.

# Evidence

- Turn 1: Linux probe showed AHCI uses interrupt-driven runtime I/O on the same Parallels platform.
- Turn 4: Linux instruction-level IRQ tail showed device handler before fasteoi/deactivation tail.
- Turn 5: Narrow level-SPI fix produced a healthy single boot: SPI 34, 1125 AHCI ISRs, CPU0 timer healthy.
- Turn 7: Polling attribution proved all post-registration polls were pre scheduler-ready boot probes; `post_sched=0`.
- Turn 8: 5 fresh Parallels boots all passed the serial gate.

# Turn 8 5-Boot Gate

| Boot | Status | Total | Post-reg | Pre-sched | Post-sched | CPU0 % |
|------|--------|-------|----------|-----------|------------|--------|
| 1 | pass | 72 | 70 | 70 | 0 | 99.23 |
| 2 | pass | 72 | 70 | 70 | 0 | 99.36 |
| 3 | pass | 72 | 70 | 70 | 0 | 98.99 |
| 4 | pass | 71 | 69 | 69 | 0 | 99.64 |
| 5 | pass | 72 | 70 | 70 | 0 | 99.71 |

Aggregate:

```text
overall: pass
post_sched: max across all boots = 0
pre_sched: distribution = 70,70,70,69,70
```

Every boot also had:

- AHCI SPI 34 discovered and enabled on CPU0.
- Userspace syscall path verified.
- 0 AHCI timeout markers.
- 0 panic / synchronous exception / data abort markers.
- 0 CPU0 regression alarm markers.

# Validation

```bash
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Build completed with zero warnings.

5-boot gate:

```bash
./turn8-artifacts/run_5boot_serial_gate.sh
```

Result: 5/5 pass.
