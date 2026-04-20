# F32p Linux xHCI MSI Delivery Ground Truth

Date: 2026-04-20

Probe: `linux-probe` at `10.211.55.3`, accessed as `wrb` with sudo. The prompt's `root@10.211.55.3` shorthand did not authenticate in this session; the repo's existing probe convention is `wrb` / password `root`.

Kernel:

```text
Linux probe 6.8.0-107-generic #107-Ubuntu SMP PREEMPT_DYNAMIC Fri Mar 13 19:42:33 UTC 2026 aarch64 aarch64 aarch64 GNU/Linux
```

## Summary

Linux on the same Parallels ARM64 hypervisor uses the xHCI MSI path successfully. The xHCI controller is the Parallels/NEC device `0000:00:03.0` (`1033:0194`). Linux allocates one MSI vector, programs the device's PCI MSI capability, unmasks the parent GIC SPI through the GICv2m MSI domain, and routes active xHCI completion traffic through:

```text
xhci_msi_irq -> xhci_irq -> xhci_handle_event
```

No timer-driven xHCI input fallback is involved in the Linux path.

## Probe Evidence

### GIC MSI Frame

Linux discovers a GICv2m MSI frame and advertises its SPI allocation range:

```text
GICv3: 988 SPIs implemented
Root IRQ handler: gic_handle_irq
GICv2m: range[mem 0x02250000-0x02250fff], SPI[53:116]
```

This means PCI MSI writes target the GICv2m frame at `0x02250000`. Linux uses offset `0x40` (`V2M_MSI_SETSPI_NS`) for the non-secure SetSPI register, so the MSI message address for Parallels xHCI is `0x02250040`.

Local Linux source:

- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:40` defines `V2M_MSI_SETSPI_NS` as `0x040`.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:100-105` composes the MSI address as `v2m->res.start + V2M_MSI_SETSPI_NS`.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:108-123` composes the MSI data; without an erratum flag, `msg->data = data->hwirq`.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:135-165` allocates the parent GIC interrupt and configures it as `IRQ_TYPE_EDGE_RISING`.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:317-397` initializes the MSI frame, records `spi_start` / `nr_spis`, and logs the SPI range.

### xHCI PCI MSI Capability

`sudo lspci -vvv -s 00:03.0`:

```text
00:03.0 USB controller: NEC Corporation uPD720200 USB 3.0 Host Controller (rev 04) (prog-if 30 [XHCI])
        Subsystem: Parallels, Inc. uPD720200 USB 3.0 Host Controller
        Control: I/O- Mem+ BusMaster+ ... DisINTx+
        Interrupt: pin A routed to IRQ 26
        Region 0: Memory at 1400c000 (32-bit, non-prefetchable) [size=4K]
        Capabilities: [e0] MSI: Enable+ Count=1/1 Maskable+ 64bit-
                Address: 02250040  Data: 0038
                Masking: 00000000  Pending: 00000000
        Kernel driver in use: xhci_hcd
```

Important configuration values to match:

| Field | Linux value |
| --- | --- |
| PCI device | `0000:00:03.0`, vendor/device `1033:0194` |
| BAR0 | `0x1400c000`, size 4 KiB |
| MSI capability offset | `0xe0` |
| MSI mode | enabled, one 32-bit message, maskable |
| MSI message address | `0x02250040` |
| MSI message data | `0x0038` |
| Actual GIC SPI selected by Linux | SPI 56 (`0x38`) |
| Linux IRQ number | IRQ 26 |

`/proc/interrupts` confirms Linux exposes the handler as an MSI-backed edge interrupt:

```text
26:        328          0          0          0       MSI 49152 Edge      xhci_hcd
```

The left number (`26`) is Linux's virtual IRQ. The MSI message data from the PCI capability (`0x0038`) is the GICv2m hwirq/SPI value for the actual interrupt line in this boot.

### xHCI Enumeration

`dmesg` confirms Linux enumerates Parallels virtual HID devices behind xHCI:

```text
xhci_hcd 0000:00:03.0: hcc params 0x01c15001 hci version 0x110 quirks 0x0000000000000014
usb 3-1: Product: Virtual Mouse
usb 3-1: Manufacturer: Parallels
usb 3-2: Product: Virtual Keyboard
usb 3-2: Manufacturer: Parallels
hid-generic ... Mouse ... on usb-0000:00:03.0-1/input0
hid-generic ... Keyboard ... on usb-0000:00:03.0-2/input0
```

## Tracefs Confirmation

I enabled ftrace function tracing for:

```text
xhci_irq
xhci_handle_event
xhci_urb_enqueue
xhci_ring_cmd_db
input_event
evdev_pass_values
evdev_read
```

Then I forced active USB traffic with `lsusb -v -d 203a:fffc` and `lsusb -v -d 203a:fffb` against the Parallels mouse and keyboard. The xHCI interrupt count rose from 328 to 352:

```text
xhci_irq_delta=24
```

The trace shows the Linux MSI IRQ path draining the xHCI event ring:

```text
lsusb-10020 [000] ..... xhci_urb_enqueue <-usb_hcd_submit_urb
<idle>-0    [000] d.h1. xhci_irq <-xhci_msi_irq
<idle>-0    [000] d.h2. xhci_handle_event <-xhci_irq
<idle>-0    [000] d.h2. xhci_handle_event <-xhci_irq
```

This proves Parallels delivers xHCI MSI interrupts on this hypervisor. Breenix must match this path rather than assuming a Parallels quirk.

## Linux Source Path

### xHCI MSI Enable

Linux enables MSI before running the controller:

- `/tmp/linux-v6.8/drivers/usb/host/xhci-pci.c:191-201` calls `xhci_try_enable_msi(hcd)` in `xhci_pci_run()` before `xhci_run(hcd)`.
- `/tmp/linux-v6.8/drivers/usb/host/xhci-pci.c:120-164` unregisters legacy IRQ, calls `pci_alloc_irq_vectors(..., PCI_IRQ_MSIX | PCI_IRQ_MSI)`, requests `xhci_msi_irq`, and marks MSI enabled.

### PCI MSI Programming Order

Linux's MSI framework sequence is:

1. Disable MSI while programming.
2. Mask all MSI vectors during setup.
3. Program message address and data.
4. Read back MSI flags to ensure writes are visible.
5. Disable INTx and set MSI Enable.
6. Unmask the IRQ through the MSI/GIC domain.

Source citations:

- `/tmp/linux-v6.8/drivers/pci/msi/api.c:207-240` documents `pci_alloc_irq_vectors()`.
- `/tmp/linux-v6.8/drivers/pci/msi/api.c:269-280` tries MSI-X first, then MSI.
- `/tmp/linux-v6.8/drivers/pci/msi/msi.c:348-390` disables MSI during setup, masks entries, sets up MSI IRQs, disables INTx, and sets MSI Enable.
- `/tmp/linux-v6.8/drivers/pci/msi/msi.c:190-204` writes MSI flags, address, data, and reads flags back as a posted-write flush.

### GIC SPI Configuration

Linux's GIC layer accepts only level-high or edge-rising for SPIs and programs the selected type:

- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:658-685` validates and configures interrupt type.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1491-1507` wires `irq_unmask`, `irq_eoi`, and `irq_set_type` into the GICv3 irqchip.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1570-1637` translates firmware interrupt specs to GIC hwirq/type.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v3.c:1643-1661` maps allocated hwirqs into the GIC IRQ domain.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:135-165` specifically asks the parent GIC domain for an edge-rising SPI and then calls `irq_set_type(..., IRQ_TYPE_EDGE_RISING)`.

### xHCI IRQ Drain

Linux's MSI wrapper is direct:

- `/tmp/linux-v6.8/drivers/usb/host/xhci-ring.c:3168-3171` implements `xhci_msi_irq()` as `return xhci_irq(hcd);`.

The IRQ handler checks `STS_EINT`, clears it, drains events, and updates the event ring dequeue pointer:

- `/tmp/linux-v6.8/drivers/usb/host/xhci-ring.c:3077-3117` checks the xHCI status register and clears `STS_EINT`.
- `/tmp/linux-v6.8/drivers/usb/host/xhci-ring.c:3142-3160` loops through `xhci_handle_event()` and updates the dequeue pointer.

### Input Wake Path

Linux evdev is wake-based:

- `/tmp/linux-v6.8/drivers/input/evdev.c:244-286` passes events to each client buffer and calls `wake_up_interruptible_poll()` after `SYN_REPORT`.
- `/tmp/linux-v6.8/drivers/input/evdev.c:558-607` implements blocking `evdev_read()` with `wait_event_interruptible()` until the packet head differs from the tail.

For Breenix, the relevant parity point is not the exact evdev ABI; it is that input consumers block on a waitqueue and are woken by input event production, rather than polling global input state in a render loop.

## Breenix Requirements Derived From Linux

Breenix must:

1. Program xHCI's MSI capability at offset `0xe0` with Enable set only after message address/data are installed.
2. Use GICv2m address `0x02250040` for this Parallels environment.
3. Use the allocated GIC SPI as MSI message data. Linux selected SPI 56 on this boot (`Data: 0038`). Breenix may allocate a different free SPI, but the SPI enabled in GIC must exactly match the MSI data.
4. Configure that SPI as edge-triggered, non-secure/group-1, priority-enabled, and unmasked before relying on device interrupts.
5. Route the interrupt to `handle_interrupt()` and drain the xHCI event ring there.
6. Wake input consumers from the event path; do not use a timer-driven HID fallback.
