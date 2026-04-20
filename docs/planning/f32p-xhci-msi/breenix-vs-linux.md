# F32p Breenix vs Linux xHCI MSI Audit

Date: 2026-04-20

This audit compares Breenix `main`/`f32p-xhci-msi-interrupt` against Linux v6.8 and the live `linux-probe` evidence in `linux-ground-truth.md`.

## Executive Finding

Breenix already finds the xHCI MSI capability, discovers GICv2m, allocates an SPI, writes MSI address/data, and records the allocated IRQ in `XHCI_IRQ`.

The blocker is the split-brain IRQ state:

- `setup_xhci_msi()` returns `early_irq`.
- `XHCI_IRQ.store(early_irq)` records it for IRQ dispatch.
- But `XhciState.irq` is initialized to `0`.
- The first and later SPI enable paths are gated on `state.irq != 0`, so the xHCI SPI is never enabled.
- `handle_interrupt()` would be called if an IRQ arrived, but no xHCI IRQ can arrive while the SPI remains disabled.

This was not accidental. Git history shows commit `488d2fc2` changed `irq` to `0` after MSI storms and documented the controller as polling-only. Linux on the same hypervisor proves the MSI/SPI path works; Breenix must stop treating the disabled SPI as a fallback mode.

## Side-by-Side Configuration Table

| Step | Linux v6.8 | Breenix Current | Parity Status |
| --- | --- | --- | --- |
| Discover xHCI PCI function | Probe sees `0000:00:03.0`, vendor/device `1033:0194`, driver `xhci_hcd`. | xHCI init receives a PCI `Device` and uses BAR/runtime/doorbell registers in `kernel/src/drivers/usb/xhci.rs`. | OK. |
| Discover GIC MSI frame | Linux logs `GICv2m: range[mem 0x02250000-0x02250fff], SPI[53:116]`. Source: `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:317-397`. | Breenix probes hardcoded Parallels base `0x0225_0000` and reads `MSI_TYPER` into `GICV2M_SPI_BASE` / `GICV2M_SPI_COUNT` (`kernel/src/platform_config.rs:217-255`). | OK for Parallels. Breenix does not yet parse MADT generically here, but the live address matches Linux. |
| Allocate MSI SPI | Linux GICv2m allocates a free hwirq from the frame bitmap (`irq-gic-v2m.c:177-218`). Probe selected SPI 56 (`MSI Data: 0038`). | Breenix uses `allocate_msi_spi()` atomic fetch-add from the discovered base/count (`kernel/src/platform_config.rs:258-280`). | Mostly OK. Linux selected SPI 56 because prior MSI devices consumed earlier vectors; Breenix may select a different free SPI. The invariant is MSI data must match the enabled SPI. |
| MSI address composition | Linux uses `v2m->res.start + V2M_MSI_SETSPI_NS`; `V2M_MSI_SETSPI_NS = 0x040` (`irq-gic-v2m.c:40`, `:100-105`). Probe address: `0x02250040`. | Breenix uses `base + 0x40` (`kernel/src/drivers/usb/xhci.rs:4439-4442`). | OK. |
| MSI data composition | Linux standard GICv2m uses `msg->data = data->hwirq` (`irq-gic-v2m.c:108-123`). Probe data: `0x0038` = SPI 56. | Breenix writes `msi_data = spi as u16` (`kernel/src/drivers/usb/xhci.rs:4439-4443`). | OK if and only if the same `spi` is enabled in GIC. Currently not true because `state.irq` is zero. |
| PCI MSI programming order | Linux disables MSI during setup, masks vectors, programs message, flushes by reading flags, disables INTx, then sets MSI Enable (`/tmp/linux-v6.8/drivers/pci/msi/msi.c:348-390`, `:190-204`). | Breenix writes address/data/mask then sets MSI Enable in `configure_msi()` (`kernel/src/drivers/pci.rs:325-375`), then disables INTx (`kernel/src/drivers/usb/xhci.rs:4443-4444`). | Partial. Breenix does not explicitly clear MSI Enable before programming and does not read back flags after message writes. It likely starts disabled after reset, but Linux parity calls for explicit disable and posted-write flush. |
| MSI-X vs MSI selection | Linux tries MSI-X first, then MSI via `pci_alloc_irq_vectors(... PCI_IRQ_MSIX | PCI_IRQ_MSI)` (`/tmp/linux-v6.8/drivers/usb/host/xhci-pci.c:120-164`, `/tmp/linux-v6.8/drivers/pci/msi/api.c:269-280`). Probe xHCI exposes plain MSI only. | Breenix only uses plain MSI for xHCI. | OK for current Parallels xHCI because `lspci` shows only MSI, not MSI-X. |
| GIC SPI trigger type | Linux GICv2m allocates parent GIC interrupt as `IRQ_TYPE_EDGE_RISING` and calls parent `irq_set_type` (`irq-gic-v2m.c:135-165`). GICv3 accepts edge-rising for SPIs (`irq-gic-v3.c:658-685`). | Breenix calls `configure_spi_edge_triggered(intid)` (`kernel/src/drivers/usb/xhci.rs:4446-4451`; implementation `kernel/src/arch_impl/aarch64/gic.rs:813-827`). | OK. |
| GIC group and priority | Linux initializes GIC and maps MSI parent interrupts through the irq domain. Probe shows GICv3 root handler and edge MSI. | Breenix GICv3 init sets all SPIs Group 1 Non-Secure and default priority (`kernel/src/arch_impl/aarch64/gic.rs:1192-1203`), and enables Group 1 (`:1205-1210`). | OK for group/priority. |
| SPI routing/unmask | Linux MSI unmask calls parent unmask (`irq-gic-v2m.c:75-84`) and `/proc/interrupts` shows active edge MSI. | Breenix has `enable_spi()` with IROUTER/ITARGETSR routing and GICD_ISENABLER write (`kernel/src/arch_impl/aarch64/gic.rs:829-880`). | Implementation exists but xHCI does not call it because `state.irq == 0`. |
| xHCI MSI enable timing | Linux calls `xhci_try_enable_msi()` before `xhci_run()` (`/tmp/linux-v6.8/drivers/usb/host/xhci-pci.c:191-201`). | Breenix calls `setup_xhci_msi()` before enumeration after controller run/startup has already reached the current point (`kernel/src/drivers/usb/xhci.rs:5014-5018`). | Probably acceptable for current Breenix flow, but not exact Linux order. The live bug is not this timing; it is SPI never being enabled. |
| IRQ dispatch registration | Linux `request_irq(pci_irq_vector(...), xhci_msi_irq, ...)` (`xhci-pci.c:157-158`) and `xhci_msi_irq()` directly calls `xhci_irq()` (`xhci-ring.c:3168-3171`). | Breenix `get_irq()` returns `XHCI_IRQ` even before initialized (`kernel/src/drivers/usb/xhci.rs:6270-6276`), and aarch64 IRQ dispatch calls `xhci::handle_interrupt()` when `irq_id == xhci_irq` (`kernel/src/arch_impl/aarch64/exception.rs:1299-1303`). | Dispatch path is present. |
| IRQ event drain | Linux checks `STS_EINT`, clears status, drains `xhci_handle_event()`, updates ERDP (`xhci-ring.c:3077-3160`). | Breenix `handle_interrupt()` acks IMAN/USBSTS, processes event ring, handles HID reports, updates ERDP (`kernel/src/drivers/usb/xhci.rs:5243-5493`). | Present. |
| Hot-path fallback | Linux does not use timer polling to drain xHCI input. Probe trace shows active USB traffic triggers `xhci_irq <- xhci_msi_irq` and `xhci_handle_event <- xhci_irq`. | Breenix timer calls `xhci::poll_hid_events()` on CPU 0 every timer tick (`kernel/src/arch_impl/aarch64/timer_interrupt.rs:645-649`). `poll_hid_events()` drains xHCI events and also performs deferred SPI activation (`kernel/src/drivers/usb/xhci.rs:5684-6125`). | Not OK. This is the fallback the factory must delete after MSI is fixed. |
| Input consumer wake | Linux `evdev_pass_values()` wakes clients and `evdev_read()` blocks on a waitqueue (`/tmp/linux-v6.8/drivers/input/evdev.c:244-286`, `:558-607`). | Breenix already has input waitqueues for window input and compositor wait, but BWM still calls `mouse_pos()` and `poll_modifier_state()` in the main loop (`userspace/programs/src/bwm.rs:1604`, `:1691`). | Partial. Phase 5 must migrate BWM to wake-based input state. |

## Root Cause: `early_irq` vs `state.irq`

Current code:

```rust
let early_irq = setup_xhci_msi(pci_dev);

let mut xhci_state = XhciState {
    ...
    irq: 0,
    ...
};
...
XHCI_IRQ.store(early_irq, Ordering::Release);
```

Source: `kernel/src/drivers/usb/xhci.rs:5014-5030` and `:5207-5209`.

`XHCI_IRQ` is enough for IRQ dispatch lookup:

```rust
pub fn get_irq() -> Option<u32> {
    let irq = XHCI_IRQ.load(Ordering::Relaxed);
    if irq != 0 {
        return Some(irq);
    }
    ...
}
```

Source: `kernel/src/drivers/usb/xhci.rs:6270-6276`.

But every xHCI-specific SPI arm/re-arm uses `state.irq`:

- Initial deferred activation in `poll_hid_events()`: `kernel/src/drivers/usb/xhci.rs:6094-6104`.
- IRQ handler disable/clear at entry: `kernel/src/drivers/usb/xhci.rs:5263-5274`.
- IRQ handler clear/re-enable at exit: `kernel/src/drivers/usb/xhci.rs:5483-5492`.

Since `state.irq` is zero, the initial activation does not call `enable_spi()`, so no MSI reaches `handle_irq()` and no xHCI interrupt can be delivered.

## Why Was `state.irq` Set To Zero?

`git blame` points `irq: 0` to commit `488d2fc2`:

```text
488d2fc2 feat: xHCI CC=12 investigation — UEFI DisconnectController + pre-EBS workarounds
```

That commit changed `irq` from the allocated value to `0` and left the polling path in place. The same commit diff says:

```text
The Parallels virtual XHCI generates back-to-back MSIs that cause
interrupt storms, freezing the system. Instead, rely on timer-driven
polling via poll_hid_events() at ~200Hz. The MSI is configured at
the PCI level ... but the GIC SPI is kept disabled to prevent storms.
```

So the current code is an intentional suppression from a CC=12/MSI-storm investigation, not a Linux-parity interrupt design. Phase 1 Linux evidence disproves treating Parallels xHCI MSI delivery as inherently broken.

## Audit Notes

### PCI MSI Capability Writes

Breenix writes the correct register fields for the live Parallels xHCI layout:

- MSI cap offset is found by traversing the PCI capability list (`kernel/src/drivers/pci.rs:302-323`).
- Address goes to `cap + 4`, data goes to `cap + 8` for 32-bit MSI (`kernel/src/drivers/pci.rs:336-354`).
- Mask bits are cleared if the capability is maskable (`kernel/src/drivers/pci.rs:356-364`).
- MSI Enable is set in Message Control (`kernel/src/drivers/pci.rs:366-374`).

Linux differs in two robustness details:

- It explicitly clears MSI Enable before programming (`/tmp/linux-v6.8/drivers/pci/msi/msi.c:359-364`).
- It reads MSI flags back after writing message address/data as a posted-write flush (`/tmp/linux-v6.8/drivers/pci/msi/msi.c:190-204`).

These should be brought into parity during the fix, but they do not explain the complete lack of interrupts as directly as `state.irq = 0`.

### GIC SPI Configuration

Breenix already configures SPI group/priority globally and xHCI trigger type specifically:

- GICv3 sets all SPIs Group 1 Non-Secure and default priority (`kernel/src/arch_impl/aarch64/gic.rs:1192-1203`).
- GICv3 enables Group 1 in `GICD_CTLR` (`kernel/src/arch_impl/aarch64/gic.rs:1205-1210`).
- xHCI calls `configure_spi_edge_triggered()` on the allocated SPI (`kernel/src/drivers/usb/xhci.rs:4446-4451`).
- `enable_spi()` routes and unmasks an SPI with barriers (`kernel/src/arch_impl/aarch64/gic.rs:829-880`).

The missing operation is not GIC configuration; it is that xHCI never invokes `enable_spi()` while `state.irq` is zero.

### Timer Polling Path

The timer path is currently doing real input work:

- CPU 0 timer calls `poll_keyboard_to_stdin()`, EHCI polling, and xHCI `poll_hid_events()` (`kernel/src/arch_impl/aarch64/timer_interrupt.rs:645-649`).
- `poll_hid_events()` increments `POLL_COUNT`, drains the event ring, requeues HID TRBs, performs endpoint reset recovery, and conditionally enables the SPI (`kernel/src/drivers/usb/xhci.rs:5684-6125`).

This violates the F32p hard constraint. It should be removed only after the MSI path is proven to deliver HID reports.

### Input Wake Path

Linux evdev wake parity maps to Breenix's existing waitqueue style:

- Linux wakes evdev readers after `SYN_REPORT` (`/tmp/linux-v6.8/drivers/input/evdev.c:244-286`).
- Linux blocking reads wait until packet head differs from tail (`/tmp/linux-v6.8/drivers/input/evdev.c:558-607`).

Breenix has `INPUT_EVENT_WQ` and `compositor_wait` in `kernel/src/syscall/graphics.rs`, but BWM still polls global state:

- Initial mouse position read: `userspace/programs/src/bwm.rs:1604`.
- Main-loop modifier poll: `userspace/programs/src/bwm.rs:1691`.

Phase 5 should extend the existing compositor/input wait path instead of adding another polling syscall.

## Required Fix Shape

Evidence-backed minimal fix sequence:

1. Make `XhciState.irq = early_irq` so the SPI enabled by Breenix matches the MSI message data, as Linux's GICv2m path requires.
2. Bring `configure_msi()` closer to Linux by clearing MSI Enable before writing message address/data and by reading back Message Control after writes.
3. Enable the xHCI SPI through the interrupt path without timer-poll dependence. The current deferred activation is in `poll_hid_events()`, which is exactly the path F32p must delete.
4. Validate that HID reports are processed from `handle_interrupt()` and that `MSI_EVENT_COUNT` rises on input.
5. Delete `poll_hid_events()` and the timer call only after step 4 is observed.

No Parallels workaround is justified by the Linux evidence.
