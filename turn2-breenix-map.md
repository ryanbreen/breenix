# Turn 2 Breenix Map: AHCI/GIC against Linux baseline

Turn 1 changed the ground truth: Linux on the same Parallels VM uses interrupt-driven AHCI on the ACPI/platform controller `PRL4010:00`, with GICv3 hwirq 34, level trigger, and effective delivery on CPU0. The Breenix polling gate is therefore masking a Breenix-side problem, not a Parallels requirement.

## A. Breenix AHCI enumeration

Breenix is not PCI-only on Parallels. It tries PCI AHCI first, then hardcodes the Parallels platform MMIO controller:

```rust
// kernel/src/drivers/mod.rs:261-271
// First try PCI (standard AHCI), then platform MMIO (Parallels Desktop).
match ahci::init() {
    ...
    Err(_) => {
        const PARALLELS_AHCI_BASE: u64 = 0x0214_0000;
        match ahci::init_platform(PARALLELS_AHCI_BASE) {
```

The PCI path is the standard class/subclass scan:

```rust
// kernel/src/drivers/ahci/mod.rs:2728-2733
let ahci_dev = pci_devices
    .iter()
    .find(|d| d.class == pci::DeviceClass::MassStorage && d.subclass == 0x06)
    .ok_or("No AHCI controller found")?;
```

The platform path does not enumerate ACPI `PRL4010`; it assumes the known Parallels base and maps it through HHDM:

```rust
// kernel/src/drivers/ahci/mod.rs:847-861
fn init_from_mmio(abar_phys: u64) -> Result<Self, &'static str> {
    let abar_virt = HHDM_BASE + abar_phys;
    let controller = Self::init_common(abar_virt)?;
    #[cfg(target_arch = "aarch64")]
    probe_platform_irq(&controller);
    Ok(controller)
}
```

Linux uses the same controller class through the platform driver. In Linux 6.8, `drivers/ata/ahci_platform.c` matches generic AHCI ACPI class devices:

```c
// drivers/ata/ahci_platform.c
{ ACPI_DEVICE_CLASS(PCI_CLASS_STORAGE_SATA_AHCI, 0xffffff) },
```

and `drivers/ata/libahci_platform.c::ahci_platform_init_host()` obtains the IRQ from firmware:

```c
irq = platform_get_irq(pdev, 0);
hpriv->irq = irq;
```

Conclusion: wrong device enumeration is not the primary bug. Breenix reaches the Parallels MMIO AHCI controller, but unlike Linux it does not consume the ACPI interrupt resource. It infers the IRQ by probing the GIC pending registers.

## B. Breenix GICv3 wiring for AHCI

### IRQ number

Breenix discovers the platform AHCI SPI dynamically. `probe_platform_irq()` issues an IDENTIFY command, deliberately leaves `PORT_IS` asserted, snapshots `GICD_ISPENDR`, diffs pending SPIs, then clears the AHCI interrupt line:

```rust
// kernel/src/drivers/ahci/mod.rs:2053-2065
// 1. Snapshot GICD_ISPENDR for SPIs 32-127 (baseline).
// 2. Issue a fresh IDENTIFY DEVICE command and poll for DMA completion
//    WITHOUT clearing PORT_IS ...
// 4. Diff the snapshots to find the newly-pending SPI.
```

The current code filters known SPIs 33, 53, 54, and 55, then records the first unknown pending SPI. Prior Breenix captures found SPI 34, which matches Linux's hwirq 34 from Turn 1.

### ARE_NS sequencing

GIC init happens before device drivers:

```rust
// kernel/src/main_aarch64.rs:516-562
Gicv2::init();
...
let device_count = kernel::drivers::init();
```

The GICv3 distributor init sets `ARE_NS` before AHCI platform probing runs:

```rust
// kernel/src/arch_impl/aarch64/gic.rs:1339-1342
gicd_write(
    GICD_CTLR,
    GICD_CTLR_DS | GICD_CTLR_ARE_NS | GICD_CTLR_ENABLE_GRP0 | GICD_CTLR_ENABLE_GRP1_NS,
);
```

So the earlier `IROUTER` failure was probably not "written before ARE_NS" in the simple boot-order sense. Breenix still differs from Linux by using only `dsb sy; isb` and readback after `GICD_CTLR`; Linux calls `gic_dist_wait_for_rwp()` after distributor control writes.

### IROUTER write path

Breenix has the correct `GICD_IROUTER` base offset today:

```rust
// kernel/src/arch_impl/aarch64/gic.rs:1143-1148
// GICD_IROUTER[n] is at 0x6000 + n*8 per the GICv3 spec.
const GICD_IROUTER: usize = 0x6000;
```

But all current distributor accessors are 32-bit:

```rust
// kernel/src/arch_impl/aarch64/gic.rs:272-289
fn gicd_read(offset: usize) -> u32 { ... read_volatile(addr) }
fn gicd_write(offset: usize, value: u32) { ... write_volatile(addr, value); }
```

`enable_spi()` routes to CPU0 by writing the low half of `IROUTER[n]`, then the high half, then enables the SPI:

```rust
// kernel/src/arch_impl/aarch64/gic.rs:855-883
let affinity = mpidr & 0xFF_00FF_FFFF;
gicd_write(GICD_IROUTER + (irq as usize * 8), affinity as u32);
gicd_write(GICD_IROUTER + (irq as usize * 8) + 4, (affinity >> 32) as u32);
gicd_write(GICD_ISENABLER + (reg_index as usize * 4), 1 << bit);
```

`enable_spi_on_cpu()` does a stronger sequence: disable, clear pending, write low half, write high half, barrier, read low/high back, retry up to three times, then enable:

```rust
// kernel/src/arch_impl/aarch64/gic.rs:937-990
gicd_write(GICD_ICENABLER + (reg_index as usize * 4), bit_mask);
gicd_write(GICD_ICPENDR + (reg_index as usize * 4), bit_mask);
gicd_write(router_offset, affinity as u32);
gicd_write(router_offset + 4, (affinity >> 32) as u32);
let mut readback =
    gicd_read(router_offset) as u64 | ((gicd_read(router_offset + 4) as u64) << 32);
...
gicd_write(enable_offset, bit_mask);
```

Linux's GICv3 path is materially different:

- `drivers/irqchip/irq-gic-v3.c::gic_dist_init()` disables the distributor, waits for RWP, enables `ARE_NS` and Group 1, waits for RWP again, then writes all SPI `IROUTER` registers to the boot CPU.
- `gic_set_affinity()` masks an enabled interrupt before changing `IROUTER`.
- `arch/arm64/include/asm/arch_gicv3.h` defines `gic_write_irouter(v, c)` as `writeq_relaxed(v, c)`, a 64-bit MMIO write.

Breenix's split 32-bit `IROUTER` write may still be architecturally legal on some implementations, but it is not Linux-equivalent. If Parallels' GIC emulation requires or only reliably implements 64-bit accesses for `GICD_IROUTER`, Breenix would see exactly the Turn 10 failure: enable-state writes work, but route writes/readbacks do not retain the intended CPU2 affinity.

### ISENABLER sequencing

For both `enable_spi()` and `enable_spi_on_cpu()`, Breenix writes `GICD_ISENABLER` after the `IROUTER` writes. That part is in the right order. It also happens after boot-time `ARE_NS` setup. The missing Linux parity is not simple order; it is the lack of 64-bit `IROUTER` access and lack of explicit distributor RWP waits around control/mask/route transitions.

### ISR registration and handler behavior

AHCI IRQ dispatch is indirect: `exception.rs` asks the AHCI driver for its registered IRQ and calls `handle_interrupt()` when the IDs match:

```rust
// kernel/src/arch_impl/aarch64/exception.rs:1288-1293
if let Some(ahci_irq) = crate::drivers::ahci::get_irq() {
    if irq_id == ahci_irq {
        crate::drivers::ahci::handle_interrupt();
    }
}
```

The AHCI handler is intended to be lock-free. It reads `HBA_IS`, scans all ports for wired level interrupts, clears `PORT_IS` and `HBA_IS`, then completes slot 0 via a lock-free completion:

```rust
// kernel/src/drivers/ahci/mod.rs:2310-2470
let hba_is = hba_read(abar, HBA_IS);
let check_all = !AHCI_IRQ_EDGE.load(Ordering::Relaxed);
...
ack_port_interrupt(abar, port, sampled_is);
...
AHCI_COMPLETIONS[port][0].complete(pending_cmd_num);
...
if AHCI_IRQ_EDGE.load(Ordering::Relaxed) {
    gic::clear_spi_pending(irq);
}
```

For level-triggered AHCI, Breenix relies on clearing the device line and the outer exception handler's GIC deactivate:

```rust
// kernel/src/arch_impl/aarch64/exception.rs:1302-1305
dispatch_irq_action(irq_id, frame);
gic::deactivate_irq(irq_id);
```

One high-suspicion difference is the IRQ tail, not the AHCI handler itself. Breenix priority-drops the interrupt before dispatch, then reopens a nested IRQ window before the level source is handled and before `DIR`/deactivate:

```rust
// kernel/src/arch_impl/aarch64/exception.rs:1323-1339
gic::priority_drop_irq(irq_id);
...
if reopen_nested_irq_window {
    asm!("msr daifclr, #3", "isb", ...);
}
handle_irq_event(irq_id, frame);
```

That is a plausible place for the earlier CPU0 vtimer/AHCI interaction to live. Linux's AHCI level handler itself is conventional: it reads host IRQ status, processes ports under the ATA host lock, then clears `HOST_IRQ_STAT` after port events (`drivers/ata/libahci.c::ahci_single_level_irq_intr()`). Breenix clears `PORT_IS` before `HBA_IS` in `ack_port_interrupt()`, which matches the comment and is probably not the first bug to chase.

## C. The polling-mode gate

Commit `ede8246a517affbe03380e1f58a717b687ad8068` (`fix(ahci): force polling on aarch64 platform irq`) disables IRQ registration entirely for aarch64 platform AHCI. The diff replaces the actual registration:

```rust
gic::clear_spi_pending(found_spi);
AHCI_IRQ_EDGE.store(false, Ordering::Release);
AHCI_IRQ.store(found_spi, Ordering::Release);
gic::enable_spi_on_cpu(found_spi, 2);
```

with:

```rust
// kernel/src/drivers/ahci/mod.rs:2242-2254
gic::disable_spi(found_spi);
return;
```

The comment says this leaves `AHCI_IRQ` at 0 so the existing timer-tick/polling path is used. That is what the runtime code does. `setup_cmd_slot0()` gates the scheduler sleep path on `AHCI_IRQ != 0`:

```rust
// kernel/src/drivers/ahci/mod.rs:1120-1155
let has_irq = AHCI_IRQ.load(Ordering::Relaxed) != 0 && port < MAX_AHCI_PORTS;
let scheduler_running =
    has_irq && current_thread_id().is_some() && timer_running;
let arm_completion = has_irq && scheduler_running;
```

When `AHCI_IRQ` is 0, `wait_cmd_slot0()` takes the `PORT_CI` polling path and explicitly clears AHCI interrupt status itself. So `ede8246a` does not layer polling on top of a registered but broken IRQ. It silences the discovered SPI and prevents AHCI IRQ registration on aarch64 platform AHCI.

The same commit also removed a Turn 11 diagnostic post-SMP `disable_spi(ahci_irq)` block from `main_aarch64.rs`. The older post-SMP reroute remains in current source, but it is guarded by `ahci_irq != 0`, so it is a no-op under the polling gate.

## D. The IROUTER readback claim re-examined

The prior Turn 10 evidence was real but over-interpreted. The report captured:

```text
last_enable_spi_irq=34
last_enable_spi_cpu=2
last_enable_spi_gic_version=3
last_enable_spi_mapped_affinity=0x0000000000000002
last_enable_spi_affinity_written=0x0000000000000002
last_enable_spi_irouter_readback=0x0000000000000000
last_enable_spi_isenabler_readback=0x00600006
last_enable_spi_retry_count=3
last_enable_spi_outcome=2
ahci_isr_count=89 ahci_isr_last_mpidr_aff0=0
```

and concluded:

> `GICD_IROUTER[34]` behaves as RAZ/WI or otherwise does not retain the write in the current GIC mode.

Turn 11 then noted `GICD_CTLR` read back `0x53`, so `ARE_NS` was apparently already enabled, and concluded that Parallels drops or ignores `IROUTER` writes for this SPI.

Turn 1 overturns the production conclusion, not necessarily the raw readback. Linux uses this exact AHCI platform controller interrupt-driven on hwirq 34, effectively on CPU0, without firing `ahci_exec_polled_cmd*`. Therefore the strongest honest restatement is:

- Turn 10 proved Breenix failed to reroute SPI34 from CPU0 to CPU2 using its current `enable_spi_on_cpu()` implementation.
- It did not prove AHCI interrupts are unusable on Parallels.
- It did not prove CPU0 delivery is inherently fatal, because Linux's effective AHCI affinity is CPU0.
- It did not prove a Parallels GIC quirk independent of Breenix's access sequence, because Breenix uses split 32-bit `IROUTER` accesses and no Linux-style RWP waits, while Linux uses `writeq_relaxed()`.

The prior conclusion was an honest misreading of incomplete evidence. The bad part was the leap from "Breenix cannot make CPU2 routing stick" to "the production-safe configuration is AHCI polling." Linux should have been checked before accepting that leap.

## E. Suspected Breenix bug list

1. **Wrong fix target: Breenix tried to route AHCI away from CPU0 instead of first making CPU0 interrupt-driven AHCI healthy.** Linux proves CPU0 AHCI delivery can work on this Parallels environment. The first new experiment should restore aarch64 platform AHCI interrupts on CPU0 and remove the CPU2 reroute assumption.

2. **Breenix's GICv3 `IROUTER` access is not Linux-equivalent.** `GICD_IROUTER` is a 64-bit register. Breenix writes two 32-bit halves and reads two 32-bit halves. Linux arm64 uses `writeq_relaxed()`. If Turn 3 still needs explicit routing, Breenix should add 64-bit GICD read/write helpers for `IROUTER` and compare again.

3. **Missing GIC distributor RWP waits around control/mask/route transitions.** Breenix uses barriers but not `RWP` polling. Linux waits after distributor disable/enable and after masking when changing affinity. This is lower than the CPU0 experiment because Linux's observed working mode may not require moving the interrupt at all.

4. **IRQ tail ordering/nested-window behavior may be unsafe for level SPIs.** Breenix priority-drops, potentially unmasks nested IRQs, then runs the device handler and deactivates. If AHCI level delivery on CPU0 still kills the vtimer after the polling gate is removed, this is the next most plausible Breenix-specific root cause. This path is timing-sensitive and should be changed only with a focused directive.

5. **AHCI ack ordering could still differ in a harmful way, but it is lower probability.** Breenix clears `PORT_IS` then `HBA_IS` with barriers. Linux processes port events and clears `HOST_IRQ_STAT` after port handling. If restored interrupts storm or lose completions, compare this path under GDB or memory-only counters before changing it.

6. **Device enumeration is a low-probability bug.** Breenix does not ACPI-enumerate `PRL4010`, but the hardcoded platform MMIO fallback reaches the same controller at `0x0214_0000`. The IRQ discovery method is less principled than Linux's `platform_get_irq()`, but previous Breenix and Linux evidence agree on SPI 34.

## F. Proposed Turn 3 scope

Attack the smallest falsifiable bug first: restore interrupt-driven platform AHCI on aarch64 using CPU0 routing, not CPU2 rerouting.

Proposed source scope if Turn 3 authorizes edits:

1. In `kernel/src/drivers/ahci/mod.rs::probe_platform_irq()`, replace the aarch64 polling gate with normal registration:
   - `gic::clear_spi_pending(found_spi);`
   - `AHCI_IRQ_EDGE.store(false, Ordering::Release);`
   - `AHCI_IRQ.store(found_spi, Ordering::Release);`
   - `gic::enable_spi(found_spi);`
2. In `kernel/src/main_aarch64.rs`, remove or guard the post-SMP `enable_spi_on_cpu(ahci_irq, 2)` reroute so it does not move AHCI away from CPU0 during this experiment.
3. Do not edit `exception.rs` or hot interrupt paths in the first experiment.
4. Validate with a clean build, then a single Parallels boot with endpoint GDB state for `ahci_irq`, `ahci_isr_count`, `ahci_isr_last_mpidr_aff0`, CPU0 timer ticks, and AHCI timeout counters. Continue to a 5-boot gate only if the one-boot result is not an immediate regression.

If CPU0 AHCI interrupts still reproduce vtimer death, Turn 4 should compare Breenix's IRQ tail/deactivate/nested-window behavior against Linux before returning to GIC routing or polling. If CPU0 AHCI works, Turn 4 can decide whether CPU2 routing is worth fixing with a Linux-style 64-bit `IROUTER` helper and distributor RWP waits.

Linux source references used for comparison:

- `drivers/ata/ahci_platform.c` from Linux 6.8.y: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/drivers/ata/ahci_platform.c?h=linux-6.8.y
- `drivers/ata/libahci_platform.c` from Linux 6.8.y: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/drivers/ata/libahci_platform.c?h=linux-6.8.y
- `drivers/ata/libahci.c` from Linux 6.8.y: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/drivers/ata/libahci.c?h=linux-6.8.y
- `drivers/irqchip/irq-gic-v3.c` from Linux 6.8.y: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/drivers/irqchip/irq-gic-v3.c?h=linux-6.8.y
- `arch/arm64/include/asm/arch_gicv3.h` from Linux 6.8.y: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/plain/arch/arm64/include/asm/arch_gicv3.h?h=linux-6.8.y
