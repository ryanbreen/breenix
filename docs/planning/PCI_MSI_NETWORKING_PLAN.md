# PCI MSI Interrupt-Driven Networking

## Problem

ARM64 network drivers (VirtIO net PCI on Parallels, e1000 on VMware) rely on
timer-based polling at 100Hz (every 10ms). This adds 5-10ms latency per
network round-trip, which compounds across DNS, TCP handshake, and HTTP
response phases. On x86, the e1000 has a proper IRQ 11 handler that processes
packets immediately via softirq.

## Goal

Replace timer-based polling with interrupt-driven packet processing on ARM64,
achieving sub-millisecond packet delivery latency.

---

## Phase 1: VirtIO Net PCI MSI on Parallels (Priority: Immediate)

### Why This Is Easy

All infrastructure already exists and is proven working:
- **GIC driver** (`gic.rs`): `enable_spi()`, `disable_spi()`,
  `configure_spi_edge_triggered()`, `clear_spi_pending()` — all present
- **PCI driver** (`pci.rs`): `find_msi_capability()`, `configure_msi()`,
  `disable_intx()` — all present
- **GICv2m MSI** (`platform_config.rs`): `probe_gicv2m()`,
  `allocate_msi_spi()` — already used by xHCI and GPU PCI drivers on Parallels
- **net_pci.rs** already has `handle_interrupt()` (line 552) that reads ISR
  and raises NetRx softirq — it's just never called from the interrupt path

### Files to Modify

#### 1. `kernel/src/drivers/virtio/net_pci.rs`

Add MSI setup following the exact pattern from `xhci.rs:setup_xhci_msi()`:

```rust
static NET_PCI_IRQ: AtomicU32 = AtomicU32::new(0);

pub fn get_irq() -> Option<u32> {
    let irq = NET_PCI_IRQ.load(Ordering::Relaxed);
    if irq != 0 { Some(irq) } else { None }
}

fn setup_net_pci_msi(pci_dev: &pci::Device) -> Option<u32> {
    // 1. Find MSI capability (cap ID 0x05)
    let cap_offset = pci_dev.find_msi_capability()?;
    // 2. Probe GICv2m (already probed by xHCI, returns cached value)
    let gicv2m_base = platform_config::gicv2m_base_phys()?;
    // 3. Allocate SPI from GICv2m pool
    let spi = platform_config::allocate_msi_spi()?;
    // 4. Program MSI: address = GICv2m doorbell, data = SPI number
    pci_dev.configure_msi(cap_offset, gicv2m_base + 0x40, spi);
    // 5. Disable INTx (MSI replaces it)
    pci_dev.disable_intx();
    // 6. Configure GIC: edge-triggered, enable SPI
    gic::configure_spi_edge_triggered(spi);
    gic::enable_spi(spi);
    Some(spi)
}
```

In `init()`, after device setup: call `setup_net_pci_msi()`, store result in
`NET_PCI_IRQ`.

Update `handle_interrupt()` with disable/clear/ack/enable SPI pattern (matching
the xHCI and GPU handlers):

```rust
pub fn handle_interrupt() {
    let irq = NET_PCI_IRQ.load(Ordering::Relaxed);
    if irq != 0 {
        gic::disable_spi(irq);
        gic::clear_spi_pending(irq);
    }
    // Read ISR status register (existing code — auto-acks on read for legacy VirtIO)
    // Raise NetRx softirq (existing code)
    if irq != 0 {
        gic::enable_spi(irq);
    }
}
```

#### 2. `kernel/src/arch_impl/aarch64/exception.rs`

Add dispatch entry in the SPI match arm (32..=1019), alongside existing GPU
PCI handler:

```rust
if let Some(net_pci_irq) = crate::drivers::virtio::net_pci::get_irq() {
    if irq_id == net_pci_irq {
        crate::drivers::virtio::net_pci::handle_interrupt();
    }
}
```

#### 3. `kernel/src/arch_impl/aarch64/timer_interrupt.rs`

Conditionalize polling — only poll when no MSI IRQ is configured:

```rust
if !crate::drivers::virtio::net_pci::get_irq().is_some()
    && (net_pci::is_initialized() || e1000::is_initialized())
    && _count % 10 == 0
{
    raise_softirq(SoftirqType::NetRx);
}
```

### Verification

- DNS resolution should complete in <200ms (was 4-5 seconds)
- HTTP fetch should complete in <2 seconds (was 10 seconds)
- `cat /proc/interrupts` or trace counters should show NIC interrupts firing

---

## Phase 2: E1000 MSI on VMware (Priority: Next)

VMware Fusion uses GICv3 with ITS (Interrupt Translation Service), not GICv2m.
This is a different MSI delivery mechanism.

### Approach A: GICv3 ITS (Correct, Complex)

The ITS provides MSI translation for GICv3 systems:

1. **Discover ITS**: Parse ACPI MADT for ITS entry, or scan GIC redistributor
   space. ITS is typically at a well-known address (e.g., 0x0801_0000 on
   VMware virt).

2. **Initialize ITS**:
   - Allocate command queue (4KB aligned, mapped uncacheable)
   - Allocate device table and collection table
   - Enable ITS via GITS_CTLR

3. **Per-device setup**:
   - `MAPD` command: map device ID to interrupt table
   - `MAPTI` command: map event ID to LPI (physical interrupt)
   - `MAPI` command: map interrupt to collection (target CPU)
   - `INV` command: invalidate cached translation

4. **MSI configuration**:
   - MSI address = `GITS_TRANSLATER` physical address
   - MSI data = device-specific event ID
   - Program via `pci_dev.configure_msi(cap, its_translater, event_id)`

5. **IRQ handling**: LPIs are delivered via GICv3 ICC_IAR1_EL1, same as SPIs.
   Dispatch by LPI number in exception.rs.

**Estimated effort**: 200-400 lines of new code for ITS initialization + per-device
setup. Most complex part is the command queue protocol.

### Approach B: INTx via ACPI _PRT (Simpler, Limited)

Parse the ACPI DSDT for PCI interrupt routing:

1. **Parse ACPI _PRT**: The PCI Routing Table maps (slot, pin) -> GIC SPI.
   Breenix already has basic ACPI parsing for MADT/SPCR. Extend to parse
   DSDT for _PRT entries.

2. **Configure SPI**: Once the SPI number is known from _PRT, configure it as
   level-triggered (INTx is level, not edge), enable in GIC.

3. **Shared interrupt handling**: INTx lines may be shared between devices.
   Handler must check each device's ISR before claiming the interrupt.

**Estimated effort**: 100-200 lines for _PRT parsing + level-triggered handler.

### Approach C: VMware-Specific Probe (Pragmatic)

If VMware always maps e1000 INTx to a known SPI (discoverable from the device
tree or hardcoded for the vmware-aarch64 machine model), we could:

1. Read `interrupt_line` from PCI config space (currently 0xFF on ARM64)
2. Use VMware's DT to find the actual SPI mapping
3. Hardcode the mapping as a platform quirk if it's stable

**Estimated effort**: 20-50 lines, but fragile.

### Recommendation

Start with Approach B (_PRT parsing) since ACPI infrastructure partially exists.
Defer ITS to Phase 3 when multiple PCI devices need independent MSI vectors.

---

## Phase 3: Generic PCI Interrupt Framework (Priority: Future)

### Dynamic IRQ Dispatch Table

Replace the chain of `if let Some(irq)` in exception.rs with a registration-
based dispatch:

```rust
static PCI_IRQ_HANDLERS: Mutex<[(u32, fn()); 16]>;

pub fn register_pci_irq(spi: u32, handler: fn()) { ... }
```

This allows any PCI driver to register its own handler without modifying
exception.rs.

### Full ITS Support

For GICv3 platforms (VMware, newer QEMU configs, real hardware):
- ITS command queue management
- LPI configuration tables (PROPBASER, PENDBASER)
- Per-device interrupt translation
- Multi-CPU interrupt routing via collections

### QEMU Virt INTx Mapping

QEMU virt machine maps PCI INTx to fixed SPIs:
- INTA -> SPI 3 (GIC INTID 35)
- INTB -> SPI 4 (GIC INTID 36)
- INTC -> SPI 5 (GIC INTID 37)
- INTD -> SPI 6 (GIC INTID 38)

With swizzling: `actual_pin = (slot + pin - 1) % 4`

These are level-triggered and shared, requiring ISR checks per device.

---

## Architecture Reference

### Current Packet Receive Path (Polling)

```
Timer interrupt (1000Hz)
  -> every 10th tick: raise_softirq(NetRx)
    -> net_rx_softirq_handler()
      -> process_rx()
        -> net_pci::receive() / e1000::receive()
          -> process_packet()
            -> udp::enqueue_packet() / tcp::handle_segment()
              -> wake blocked thread
```

Latency: 0-10ms (mean 5ms) per packet.

### Target Packet Receive Path (MSI)

```
NIC MSI interrupt -> GIC SPI
  -> exception.rs handle_irq()
    -> net_pci::handle_interrupt()
      -> read ISR (auto-ack)
      -> raise_softirq(NetRx)
        -> net_rx_softirq_handler()
          -> process_rx()
            -> ... (same as above)
```

Latency: <100us per packet (GIC + softirq overhead).

### MSI Delivery on Parallels (GICv2m)

```
Device writes MSI data to GICv2m doorbell address:
  addr = GICV2M_BASE + 0x40 (MSI_SETSPI_NS)
  data = allocated SPI number

GICv2m translates write to GIC SPI assertion.
GIC delivers SPI to target CPU via ICC_IAR1_EL1.
```
