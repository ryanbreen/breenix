# Turn 1 Linux Probe: AHCI completion mode on Parallels

## A. Linux on Parallels: interrupt-driven or polled?

Verdict: Linux on the Parallels `linux-probe` VM uses interrupt-driven AHCI completion, not polling.

Evidence:

- `/proc/interrupts` shows AHCI bound to Linux IRQ 15, backed by `GICv3` hwirq 34:
  `15: ... GICv3 34 Level ahci[PRL4010:00]`
- During a 512 MiB direct read from `/dev/sda`, the AHCI IRQ count increased from `126677` to `127236`, a delta of `559`.
  See `turn1-artifacts/linux-probe/proc-interrupts-ahci-before.txt`,
  `turn1-artifacts/linux-probe/proc-interrupts-ahci-after.txt`, and
  `turn1-artifacts/linux-probe/proc-interrupts-ahci-diff.txt`.
- bpftrace under a 4 GiB direct read captured hard AHCI IRQ handling and command completion:
  - `ahci_single_level_irq_intr`: `4633`
  - `ahci_handle_port_interrupt`: `4593`
  - `ahci_qc_complete`: `4559`
  See `turn1-artifacts/linux-probe/bpftrace-ahci.txt`.
- A second bpftrace run checked the polling helper directly under another 4 GiB direct read:
  - `ahci_single_level_irq_intr`: `4770`
  - `ahci_handle_port_interrupt`: `4770`
  - `ahci_exec_polled_cmd*`: `0`
  See `turn1-artifacts/linux-probe/bpftrace-ahci-polled-check.txt`.
- `dmesg` identifies the controller as AHCI platform mode and gives the IRQ:
  `ahci PRL4010:00: AHCI 0001.0100 32 slots 6 ports 3 Gbps 0x3f impl platform mode`
  and `ata1..ata6 ... irq 15`.
  See `turn1-artifacts/linux-probe/dmesg-ahci.txt`.

The prior conclusion that Parallels requires AHCI polling is not supported by Linux behavior on this VM.

## B. AHCI IRQ wiring on Linux

This VM does not expose the AHCI controller as a PCI `:2922` device. The AHCI controller is an ACPI platform device:

- ACPI HID: `PRL4010`
- ACPI path: `\_SB_.AHC0`
- modalias: `acpi:PRL4010:010601:`
- platform driver: `/sys/bus/platform/drivers/ahci`
- kernel module path: `ahci_platform`

Raw captures:

- `turn1-artifacts/linux-probe/platform-ahci-details.txt`
- `turn1-artifacts/linux-probe/sys-bus-platform-drivers-ahci.txt`
- `turn1-artifacts/linux-probe/lspci-v.txt`
- `turn1-artifacts/linux-probe/lspci-v-ahci-2922.txt` (empty: no PCI AHCI `:2922` device)

IRQ routing details:

- Linux IRQ: `15`
- IRQ chip: `GICv3`
- GIC hwirq: `34`
- IRQ type: `level`
- action: `ahci[PRL4010:00]`
- `smp_affinity`: `f` / `0-3`
- `effective_affinity`: `1` / CPU0

Raw captures:

- `turn1-artifacts/linux-probe/sys-kernel-irq15.txt`
- `turn1-artifacts/linux-probe/irq-routing-details.txt`
- `turn1-artifacts/linux-probe/dmesg-gic.txt`

Linux booted with ACPI GIC routing:

- `GICv3: 988 SPIs implemented`
- `Root IRQ handler: gic_handle_irq`
- `ACPI: Using GIC for interrupt routing`

The AHCI IRQ is currently delivered on CPU0 in Linux, which is important: Linux handles AHCI interrupts on CPU0 without killing the CPU0 vtimer.

## C. Comparison to the prior Breenix IROUTER claim

Linux successfully uses interrupt-driven AHCI completion on Parallels. That means the polling workaround in Breenix is masking a Breenix-side bug, not a Parallels requirement.

This probe does not prove whether Linux writes `GICD_IROUTER[34]` or simply accepts the default CPU0 routing. The visible Linux state is that the AHCI platform IRQ is `GICv3` hwirq 34 and effectively targets CPU0. That is enough to reject the production conclusion that AHCI must be polled on Parallels: thousands of Linux AHCI IRQ handler and completion-path invocations fire under disk load, and the polled command helper does not.

If Breenix sees `GICD_IROUTER` writes/readbacks fail, the next investigation should treat that as a Breenix GIC setup, register indexing, ordering, affinity encoding, or diagnostic-method bug until proven otherwise. Linux demonstrates that Parallels can deliver AHCI interrupts for this controller.

## D. Implications for Turn 2

Turn 2 should inspect Breenix's AHCI platform IRQ registration and GICv3 setup against the Linux-observed wiring:

- AHCI should be a platform/ACPI-style controller at MMIO `0x02140000-0x02141fff`, with GIC hwirq 34 / Linux IRQ 15 behavior as the reference.
- Do not assume moving AHCI off CPU0 is required for correctness; Linux's effective affinity is CPU0 and the vtimer remains healthy.
- Focus on why Breenix's AHCI ISR path or GIC setup caused the earlier "CPU0 vtimer dies after AHCI ISR fires" symptom:
  - GICD/GICR enable, priority, group, and ARE ordering
  - IROUTER index/affinity encoding and readback method
  - AHCI interrupt ack/clear sequence
  - ISR lock/scheduler interaction on CPU0

Raw artifacts are under `turn1-artifacts/linux-probe/`.
