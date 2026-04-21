# F32t Linux xHCI MSI Ground Truth

Captured on 2026-04-20 from `wrb@10.211.55.3` (`Linux probe 6.8.0-107-generic #107-Ubuntu ... aarch64`) and audited against `/tmp/linux-v6.8`.

## Live Linux Device State

Linux binds the Parallels NEC xHCI controller at `0000:00:03.0` to `xhci_hcd` with MSI enabled:

```text
00:03.0 USB controller: NEC Corporation uPD720200 USB 3.0 Host Controller (rev 04)
Control: I/O- Mem+ BusMaster+ ... DisINTx+
Capabilities: [e0] MSI: Enable+ Count=1/1 Maskable+ 64bit-
        Address: 02250040  Data: 0038
        Masking: 00000000  Pending: 00000000
Kernel driver in use: xhci_hcd
```

`/proc/interrupts` shows Linux delivering xHCI through MSI:

```text
26:        206          0          0          0       MSI 49152 Edge      xhci_hcd
```

GICv2m discovery from dmesg:

```text
GICv2m: range[mem 0x02250000-0x02250fff], SPI[53:116]
```

Linux final PCI config bytes for the MSI capability at `0xe0`:

```text
e0: 05 00 01 01 40 00 25 02 38 00 00 00 00 00 00 00
```

Decoded:
- `0xe0`: capability ID `0x05`, next `0x00`
- `0xe2`: MSI flags `0x0101` (`Enable=1`, `Maskable=1`, `64bit=0`, `Count=1`)
- `0xe4`: message address `0x02250040`
- `0xe8`: message data `0x0038` (`56`, a GIC SPI in the `[53:116]` frame)
- `0xec`: mask bits `0x00000000`
- `0xf0`: pending bits `0x00000000`

## Observed Linux PCI Config Write Order

I attached kprobes to `pci_bus_write_config_word`, `pci_bus_write_config_dword`, and `__pci_write_msi_msg`, then unbound and rebound `0000:00:03.0` from `xhci_hcd`. Device function `0x18` is bus 0, device 3, function 0.

Relevant bind sequence:

```text
pciw_word  devfn=0x18 pos=0x4  val=0x16        # INTx enabled during early bind setup
pciw_word  devfn=0x18 pos=0xe2 val=0x100       # clear MSI Enable
pcid_dword devfn=0x18 pos=0xec val=0x1         # mask vector 0
msi_msg    __pci_write_msi_msg
pciw_word  devfn=0x18 pos=0xe2 val=0x100       # write flags/QSIZE with enable still clear
pcid_dword devfn=0x18 pos=0xe4 val=0x2250040   # write Message Address
pciw_word  devfn=0x18 pos=0xe8 val=0x38        # write Message Data
pciw_word  devfn=0x18 pos=0x4  val=0x416       # disable INTx
pciw_word  devfn=0x18 pos=0xe2 val=0x101       # set MSI Enable
pcid_dword devfn=0x18 pos=0xec val=0x0         # unmask vector 0
```

This proves the Linux order on the same Parallels VM:

1. Clear MSI Enable first.
2. Mask the supported vector.
3. Write flags/QSIZE with enable still clear.
4. Write Message Address and Message Data.
5. Flush posted config writes by reading MSI flags.
6. Disable INTx.
7. Set MSI Enable.
8. Unmask the vector after MSI is enabled.

## Linux Source Cites

PCI MSI order:
- `/tmp/linux-v6.8/drivers/pci/msi/msi.c:271-280` implements `pci_msi_set_enable()` by reading flags, clearing `PCI_MSI_FLAGS_ENABLE`, conditionally setting it, then writing flags.
- `/tmp/linux-v6.8/drivers/pci/msi/msi.c:359-376` disables MSI during setup and masks all MSIs before configuring the message.
- `/tmp/linux-v6.8/drivers/pci/msi/msi.c:184-204` writes flags/QSIZE, Message Address, Message Data, then reads back `PCI_MSI_FLAGS` to ensure visibility.
- `/tmp/linux-v6.8/drivers/pci/msi/msi.c:387-389` disables INTx and then enables MSI.
- `/tmp/linux-v6.8/drivers/pci/msi/msi.c:111-124` updates the per-vector mask dword when masking or unmasking.

GICv2m MSI composition and SPI mapping:
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:100-105` uses the MSI frame base plus `V2M_MSI_SETSPI_NS` (`0x40`) as the standard MSI address.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:108-123` composes MSI data from `data->hwirq`, except for documented quirks.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:135-165` allocates the parent GIC interrupt as `IRQ_TYPE_EDGE_RISING`.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:198-211` sets `hwirq = spi_start + offset` and stores that same hwirq in the IRQ domain.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:341-369` reads `MSI_TYPER` to derive `spi_start`/`nr_spis` and documents that standard GICv2m data is the absolute SPI value.
- `/tmp/linux-v6.8/drivers/irqchip/irq-gic-v2m.c:517-522` accepts ACPI override `spi_base`/`spi_count`; on this VM Linux reports `SPI[53:116]`.

Input wake path:
- `/tmp/linux-v6.8/drivers/input/evdev.c:253-285` pushes input values into client buffers and wakes the client waitqueue on `SYN_REPORT`.
- `/tmp/linux-v6.8/drivers/input/evdev.c:558-603` implements blocking `evdev_read()` with `wait_event_interruptible()` until packet data exists.

## Breenix Implications

- Breenix must write MSI data as the absolute GIC SPI for standard GICv2m, matching Linux's `msg->data = data->hwirq`.
- The SPI enabled in GIC must be the same integer as the MSI data written to the xHCI device.
- MSI setup should mask before writing the message and should unmask only after the device has INTx disabled and MSI enabled.
- Input consumption should follow Linux's wake-on-event model: HID report handling updates state and wakes BWM, while BWM blocks on one readiness syscall.
