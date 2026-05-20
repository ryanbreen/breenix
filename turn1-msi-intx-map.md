# Turn 1 - aarch64 PCI MSI/INTx Routing Source Map

Source-only turn. I did not boot Breenix, start VMs, run GDB, or query the
Linux probe.

## A. MSI Allocation Map

### Shared PCI MSI/MSI-X helpers

- `kernel/src/drivers/pci.rs:60-63` defines PCI capability IDs `0x05` (MSI)
  and `0x11` (MSI-X).
- `kernel/src/drivers/pci.rs:316-337` walks the PCI capability list to find
  plain MSI.
- `kernel/src/drivers/pci.rs:339-432` programs plain MSI:
  - detects 32-bit vs 64-bit message format at `kernel/src/drivers/pci.rs:345-357`;
  - masks vector 0 when mask bits exist at `kernel/src/drivers/pci.rs:376-383`;
  - forces single-vector MSI at `kernel/src/drivers/pci.rs:386-393`;
  - writes message address and data at `kernel/src/drivers/pci.rs:395-411`;
  - disables legacy INTx at `kernel/src/drivers/pci.rs:419`;
  - enables MSI and unmasks at `kernel/src/drivers/pci.rs:421-430`.
- `kernel/src/drivers/pci.rs:434-445` finds MSI-X and reads table size.
- `kernel/src/drivers/pci.rs:458-483` enables/disables MSI-X.
- `kernel/src/drivers/pci.rs:485-517` programs MSI-X table entries with
  address low/high, data, and vector-control unmask.

### GIC MSI allocator

- `kernel/src/platform_config.rs:58-64` stores the GICv2m frame base, first SPI,
  SPI count, and next allocation index.
- `kernel/src/platform_config.rs:196-214` exposes the stored GICv2m base/SPI
  range.
- `kernel/src/platform_config.rs:217-255` probes a GICv2m MSI frame by reading
  `MSI_TYPER` at offset `0x008`, extracting `BASE_SPI` and `NUM_SPI`, and
  storing the result.
- `kernel/src/platform_config.rs:258-280` allocates the next GICv2m SPI with
  `GICV2M_NEXT_SPI.fetch_add(1)`.

The kernel uses GICv2m "SPI as MSI" routing. I found no implemented GIC ITS,
LPI, `GITS_*`, `GICR_PROPBASER`, or `GICR_PENDBASER` code under `kernel/src`.
There is only a planning document for ITS support in
`docs/planning/PCI_MSI_NETWORKING_PLAN.md:121-151`.

One important gap: the Parallels loader sees MADT GIC MSI frame entries but only
logs them. `parallels-loader/src/acpi_discovery.rs:121-131` reads
`MadtEntry::GicMsiFrame`, but `parallels-loader/src/hw_config.rs:45-115` and
`kernel/src/platform_config.rs:500-528` have no field for the MSI frame. Current
drivers therefore probe hardcoded Parallels GICv2m base `0x0225_0000`.

### Device-specific MSI users

#### xHCI

- `kernel/src/drivers/usb/xhci.rs:4384-4392` documents xHCI's PCI MSI setup
  through GICv2m.
- `kernel/src/drivers/usb/xhci.rs:4395-4402` requires a plain MSI capability.
  xHCI does not try MSI-X.
- `kernel/src/drivers/usb/xhci.rs:4404-4424` probes GICv2m, using hardcoded
  `0x0225_0000` if no frame is already stored.
- `kernel/src/drivers/usb/xhci.rs:4431-4437` allocates one SPI and treats the
  returned SPI as the GIC INTID.
- `kernel/src/drivers/usb/xhci.rs:4439-4443` writes MSI address
  `base + 0x40` and data `spi`.
- `kernel/src/drivers/usb/xhci.rs:4445-4452` configures the SPI edge-triggered
  but explicitly does not enable it.
- `kernel/src/drivers/usb/xhci.rs:5013-5018` calls `setup_xhci_msi()` before
  enumeration and stores the result as `early_irq`.
- `kernel/src/drivers/usb/xhci.rs:5024-5034` places `early_irq` in the xHCI
  state.
- `kernel/src/drivers/usb/xhci.rs:5210-5213` stores `early_irq` in `XHCI_IRQ`.
- `kernel/src/drivers/usb/xhci.rs:5227-5231` prints the visible line
  `[xhci] Initialized: ... MSI irq={early_irq}`.

The `MSI irq=56` value is not hardcoded. It is derived from the GICv2m base SPI
plus allocation order. On the normal Parallels PCI driver path,
`kernel/src/drivers/mod.rs:176-190` initializes GPU first, `kernel/src/drivers/mod.rs:198-209`
initializes virtio-net next, and `kernel/src/drivers/mod.rs:229-247` initializes
xHCI after that. GPU allocates two SPIs at
`kernel/src/drivers/virtio/gpu_pci.rs:1726-1728`, virtio-net allocates one at
`kernel/src/drivers/virtio/net_pci.rs:368`, and xHCI then allocates the next one
at `kernel/src/drivers/usb/xhci.rs:4431-4433`. If the probed GICv2m base SPI is
53, this source ordering yields GPU 53/54, net 55, xHCI 56.

#### VirtIO GPU PCI

- `kernel/src/drivers/virtio/gpu_pci.rs:1700-1708` requires the Linux-shaped
  two-vector MSI-X layout.
- `kernel/src/drivers/virtio/gpu_pci.rs:1714-1724` probes GICv2m at the same
  hardcoded Parallels base.
- `kernel/src/drivers/virtio/gpu_pci.rs:1726-1734` allocates config and queue
  SPIs.
- `kernel/src/drivers/virtio/gpu_pci.rs:1736-1757` programs MSI-X vector 0 and
  vector 1.
- `kernel/src/drivers/virtio/gpu_pci.rs:1759-1761` enables MSI-X and disables
  INTx.
- `kernel/src/drivers/virtio/gpu_pci.rs:2075-2083` stores both IRQs, clears
  pending state, and enables both SPIs.

#### VirtIO network PCI

- `kernel/src/drivers/virtio/net_pci.rs:298-310` resolves the GICv2m doorbell
  as `base + 0x40`.
- `kernel/src/drivers/virtio/net_pci.rs:312-340` tries plain MSI first,
  allocating one SPI and enabling it immediately if successful.
- `kernel/src/drivers/virtio/net_pci.rs:342-389` then tries MSI-X, programs all
  MSI-X entries to one SPI, configures it edge-triggered, stores it, enables
  MSI-X, and disables INTx.
- `kernel/src/drivers/virtio/net_pci.rs:423-431` disables MSI-X and re-enables
  INTx if the device rejects the RX vector. There is no aarch64 INTx routing
  path to make that fallback interrupt-driven.
- `kernel/src/net/mod.rs:421-430` calls `net_pci::enable_msi_spi()` after the
  synchronous network init polling is complete.
- `kernel/src/drivers/virtio/net_pci.rs:959-984` clears device ISR state and
  enables the stored SPI.

#### AHCI

- `kernel/src/drivers/ahci/mod.rs:1990-2003` sets up PCI AHCI MSI-X or MSI
  through GICv2m.
- `kernel/src/drivers/ahci/mod.rs:2009-2029` probes GICv2m and allocates one
  SPI.
- `kernel/src/drivers/ahci/mod.rs:2031-2061` tries MSI-X, programs every vector
  to the same SPI, disables INTx, clears pending state, enables the SPI, and
  stores `AHCI_IRQ`.
- `kernel/src/drivers/ahci/mod.rs:2064-2073` falls back to plain MSI with the
  same immediate GIC enable.
- `kernel/src/drivers/ahci/mod.rs:2076-2077` falls back to polling if neither
  MSI-X nor MSI exists.

## B. INTx Legacy Routing Map

There is no generic aarch64 PCI INTx routing implementation in the current
source.

- `kernel/src/drivers/pci.rs:1019-1023` reads PCI config offset `0x3c` into
  `interrupt_line` and `interrupt_pin`.
- `kernel/src/drivers/pci.rs:194-198` stores those values in `Device`.
- `kernel/src/drivers/pci.rs:272-294` can set or clear PCI Command bit 10
  (`INTx Disable`).
- `kernel/src/drivers/pci.rs:1088-1122` logs discovered PCI devices including
  `interrupt_line`, but does not route that line.
- The aarch64 loader parses MADT, MCFG, and SPCR at
  `parallels-loader/src/acpi_discovery.rs:53-60`, but does not parse DSDT `_PRT`
  or device-tree `interrupt-map`/`msi-map`.
- `docs/planning/PCI_MSI_NETWORKING_PLAN.md:154-168` describes ACPI `_PRT` INTx
  routing as a future approach, not current code.

For xHCI specifically, MSI unavailable means `setup_xhci_msi()` returns 0
(`kernel/src/drivers/usb/xhci.rs:4395-4424`), `XHCI_IRQ` is stored as 0
(`kernel/src/drivers/usb/xhci.rs:5210-5213`), and `get_irq()` returns no IRQ
(`kernel/src/drivers/usb/xhci.rs:6273-6284`). xHCI init itself only enables
memory space and bus mastering at `kernel/src/drivers/usb/xhci.rs:4512-4538`;
it does not call `enable_intx()` or install any legacy INTx route. So the
effective fallback is timer polling, not INTx.

The closest current non-MSI wired interrupt path is not PCI INTx: platform AHCI
probes GIC pending bits for a level-triggered platform SPI at
`kernel/src/drivers/ahci/mod.rs:2080-2277`. That path snapshots
`GICD_ISPENDR`, issues an IDENTIFY, diffs pending SPIs, stores `AHCI_IRQ`, and
enables the SPI. It is for the Parallels ACPI/MMIO AHCI controller, not a PCI
INTA/B/C/D route. Its `known_spis` list at `kernel/src/drivers/ahci/mod.rs:2226-2228`
is stale for the prior xHCI `irq=56` observation because it excludes only
`[33, 53, 54, 55]`.

## C. GIC Dispatch Chain

### IRQ entry and dispatch

- `kernel/src/arch_impl/aarch64/exception.rs:1318-1324` is the external IRQ
  entry point. It calls `gic::acknowledge_irq()`.
- `kernel/src/arch_impl/aarch64/gic.rs:632-686` acknowledges IRQs. On normal
  GICv3 (QEMU/Parallels) it reads `ICC_IAR1_EL1` and returns IDs `<= 1019`.
- `kernel/src/arch_impl/aarch64/exception.rs:1328-1345` priority-drops the IRQ,
  optionally reopens nested interrupts for non-level-triggered IRQs, then calls
  `handle_irq_event()`.
- `kernel/src/arch_impl/aarch64/exception.rs:1307-1310` calls
  `dispatch_irq_action()` and then `gic::deactivate_irq()`.
- `kernel/src/arch_impl/aarch64/gic.rs:710-722` deactivates the IRQ with
  `ICC_DIR_EL1` on GICv3 or `GICC_EOIR` on GICv2.

### Explicitly handled IRQ IDs/classes

`dispatch_irq_action()` is at `kernel/src/arch_impl/aarch64/exception.rs:1195-1305`:

- timer IRQ: `timer_interrupt::timer_irq()` at `kernel/src/arch_impl/aarch64/exception.rs:1198-1207`;
- UART0 IRQ 33 at `kernel/src/arch_impl/aarch64/exception.rs:1209-1212`;
- SGIs 0-15, including reschedule and timer rearm, at `kernel/src/arch_impl/aarch64/exception.rs:1214-1227`;
- PPIs 16-31, with virtual/physical timer special cases, at `kernel/src/arch_impl/aarch64/exception.rs:1229-1247`;
- SPIs 32-1019 at `kernel/src/arch_impl/aarch64/exception.rs:1249-1300`.

The SPI dispatch is explicit only:

- VirtIO input MMIO: `kernel/src/arch_impl/aarch64/exception.rs:1252-1257`;
- VirtIO tablet MMIO: `kernel/src/arch_impl/aarch64/exception.rs:1258-1263`;
- VirtIO net MMIO: `kernel/src/arch_impl/aarch64/exception.rs:1264-1269`;
- xHCI USB: `kernel/src/arch_impl/aarch64/exception.rs:1270-1275`;
- VirtIO GPU PCI config and queue MSI-X: `kernel/src/arch_impl/aarch64/exception.rs:1276-1285`;
- VirtIO net PCI GICv2m MSI/MSI-X: `kernel/src/arch_impl/aarch64/exception.rs:1287-1291`;
- AHCI MSI/MSI-X/wired: `kernel/src/arch_impl/aarch64/exception.rs:1293-1299`.

There is no generic "unknown SPI" handler. If an SPI reaches the `32..=1019`
arm and no registered driver IRQ equals it, the code falls out of the arm and
the IRQ is deactivated silently.

### xHCI MSI delivery chain

If xHCI MSI is armed:

1. `setup_xhci_msi()` programs PCI MSI address `GICv2m_base + 0x40` and data
   `spi` at `kernel/src/drivers/usb/xhci.rs:4439-4443`.
2. `gic::configure_spi_edge_triggered()` marks the SPI edge-triggered at
   `kernel/src/drivers/usb/xhci.rs:4445-4450`.
3. `poll_hid_events()` performs the first enable at poll >= 50:
   `clear_spi_pending()` and `enable_spi()` at
   `kernel/src/drivers/usb/xhci.rs:6097-6107`.
4. `gic::enable_spi()` routes the SPI to CPU0 via GICD_IROUTER on GICv3 and
   writes `GICD_ISENABLER` at `kernel/src/arch_impl/aarch64/gic.rs:856-907`.
5. A device MSI write to the GICv2m doorbell makes the GIC deliver that SPI.
6. `handle_irq()` acknowledges the IRQ, `dispatch_irq_action()` compares it
   with `xhci::get_irq()`, and calls `xhci::handle_interrupt()` at
   `kernel/src/arch_impl/aarch64/exception.rs:1270-1275`.
7. `xhci::handle_interrupt()` disables and clears the SPI, acknowledges IMAN
   and USBSTS, drains the event ring, increments `MSI_EVENT_COUNT`, and re-enables
   the SPI at `kernel/src/drivers/usb/xhci.rs:5240-5496`.

## D. CPU0 Timer Regression Interaction

The CPU0 regression alarm is in a frozen gold-master region at
`kernel/src/arch_impl/aarch64/timer_interrupt.rs:531-607`. It fires when any
non-CPU0 reaches tick 30000 and CPU0 has less than 10% of the max peer tick
count:

- trigger documentation: `kernel/src/arch_impl/aarch64/timer_interrupt.rs:531-548`;
- condition: `cpu_id >= 1`, `this_cpu_ticks == 30000`, and
  `cpu0.saturating_mul(10) < max_peer` at
  `kernel/src/arch_impl/aarch64/timer_interrupt.rs:549-562`;
- panic: `kernel/src/arch_impl/aarch64/timer_interrupt.rs:598-603`.

CPU0 has special work in the timer handler:

- global wall-clock tick update: `kernel/src/arch_impl/aarch64/timer_interrupt.rs:734-737`;
- keyboard/EHCI/xHCI polling: `kernel/src/arch_impl/aarch64/timer_interrupt.rs:750-755`;
- network softirq kick for PCI net/e1000: `kernel/src/arch_impl/aarch64/timer_interrupt.rs:755-760`;
- CPU0 soft-lockup detector and heartbeat trace markers:
  `kernel/src/arch_impl/aarch64/timer_interrupt.rs:763-778`.

For xHCI, CPU0 death directly blocks the first GIC SPI enable because
`poll_hid_events()` is the only path that performs the first xHCI
`gic::enable_spi(state.irq)`. The gate is at
`kernel/src/drivers/usb/xhci.rs:6097-6107`. This means `MSI_EVENT_COUNT=0` is
not evidence that xHCI MSI delivery is broken when `SPI_ACTIVATED=0`; the GIC
SPI was never armed.

CPU0 death does not block every PCI MSI path by source structure:

- GPU enables both MSI-X SPIs during GPU init at
  `kernel/src/drivers/virtio/gpu_pci.rs:2075-2083`.
- AHCI PCI MSI/MSI-X enables immediately during setup at
  `kernel/src/drivers/ahci/mod.rs:2049-2054` and
  `kernel/src/drivers/ahci/mod.rs:2064-2073`.
- VirtIO-net plain MSI enables immediately at
  `kernel/src/drivers/virtio/net_pci.rs:324-336`; MSI-X enables later through
  `kernel/src/net/mod.rs:421-430` and `kernel/src/drivers/virtio/net_pci.rs:959-984`.

So the source-level conclusion is narrower than "CPU0 death blocks all PCI
interrupt delivery." It blocks xHCI's deferred first SPI enable and CPU0-only
polling/recovery paths. It may also delay net runtime switching if the network
init path depends on CPU0-timer-driven RX polling, but not all PCI MSI enabling
is timer-gated.

## E. Diagnostic Infrastructure Inventory

### CPU0 autopsy and branch history

`docs/planning/cpu0-user-guard-autopsy/README.md` says the older CPU0 user-guard
bug was closed by PR #334 / commit `9da897f4`, and that the root cause was a
self-referential CPU0 dispatch guard, not HVF/Parallels vtimer behavior. It also
lists rejected hypotheses: missing ISB before ERET, DAIF mask differences,
idle-loop DAIF state, missing return-to-userspace ISB, idle-loop rearm removal,
PCI MSI ordering, xHCI `state.irq = 0`, HVF vtimer death on IMASK transition,
SGI admission, idle-gate spin, per-CPU `need_resched`, and ret-based idle
dispatch.

`git log --all --grep cpu0-timer` finds merge `ee3518e0` for
`feat/cpu0-timer-death-gdb`. Without switching branches, the branch history shows
diagnostic work including:

- GDB/chat infrastructure: `9f05b3a7 feat: add ARM64 GDB debugging infrastructure`;
- timer hardware state in AHCI timeout diagnostics: `c7721491`;
- GICR PPI 27/30 enable/pending checks: `a19a8cc9`, `0d0bfa85`;
- per-CPU DAIF and hardware MPIDR tracking: `5431bf32`, `b96ca452`;
- CPU0 CVAL/CNTVCT snapshots: `eb8342f3`;
- CPU0 idle-loop GIC/timer register snapshots: `8814a6ac`;
- fork/schedule breadcrumbs: `1b6b303d`, `8aa3e9e0`;
- ISB/ERET and IMASK experiments: `4b81ac20`, `ddcb47c9`, `f5a1d663`,
  `23a1f47a`, `aeb3e989`, `aade0871`.

`breenix-gdb-chat/WORKING_DEMO.md` on that branch documents a JSON GDB wrapper
that can start QEMU, set breakpoints, continue with auto-interrupt, inspect
registers, and terminate cleanly.

### Current counters and traces

- Timer CPU0 diagnostics:
  - `TIMER_TICK_COUNT`, `IDLE_LOOP_COUNT`, hardware CPU/tick counters, CNTV_CTL
    snapshots, CPU0 CVAL/CNTVCT, last timer ELR, and CPU0 breadcrumbs are defined
    at `kernel/src/arch_impl/aarch64/timer_interrupt.rs:65-166`.
  - CPU0 CVAL/CNTVCT, hardware MPIDR, hardware tick count, and last ELR are
    updated at `kernel/src/arch_impl/aarch64/timer_interrupt.rs:700-731`.
  - CPU1 can send `SGI_TIMER_REARM` to CPU0 if CPU0 ticks stay <= 10 while CPU1
    advances beyond 100 ticks at `kernel/src/arch_impl/aarch64/timer_interrupt.rs:780-794`.
  - `dispatch_irq_action()` handles `SGI_TIMER_REARM` by calling `rearm_timer()`
    at `kernel/src/arch_impl/aarch64/exception.rs:1219-1225`.
- GIC diagnostics:
  - `LAST_ENABLE_SPI_*` fields are defined at `kernel/src/arch_impl/aarch64/gic.rs:262-270`.
  - `enable_spi_on_cpu()` records affinity, IROUTER readback, retry count, and
    outcome at `kernel/src/arch_impl/aarch64/gic.rs:909-1008`.
  - `snapshot_pending_spis()` reads `GICD_ISPENDR` for SPIs 32-127 at
    `kernel/src/arch_impl/aarch64/gic.rs:767-778`.
- xHCI diagnostics:
  - poll, event, PSC, MSI event, and SPI enable counters are defined at
    `kernel/src/drivers/usb/xhci.rs:409-469`.
  - `/proc/xhci/trace` is generated through `kernel/src/fs/procfs/xhci.rs:1-10`.
  - `format_trace_buffer()` appends xHCI diagnostic counters at
    `kernel/src/drivers/usb/xhci.rs:1003-1050`.
- AHCI diagnostics:
  - AHCI IRQ count, last MPIDR, PMR, ELR, SPSR, and per-port hit/complete
    counters are defined at `kernel/src/drivers/ahci/mod.rs:221-266`.
  - The AHCI ISR updates those fields at `kernel/src/drivers/ahci/mod.rs:2319-2366`.
  - AHCI timeout diagnostics print CPU tick counts and CPU0 breadcrumb/timer
    state around `kernel/src/drivers/ahci/mod.rs:1457-1600`.
- `/proc/stat` exposes aggregate IRQ/timer counters and `net_msi_irqs` at
  `kernel/src/fs/procfs/mod.rs:780-820`.

## F. Hypothesis Ranking

1. **C-like for xHCI measurement only: CPU0 timer death explains the zero xHCI
   MSI evidence so far.** The source proves xHCI's first GIC SPI enable happens
   only in CPU0 timer-driven `poll_hid_events()` at
   `kernel/src/drivers/usb/xhci.rs:6097-6107`. If CPU0 only reached five polls,
   then `SPI_ACTIVATED=0` and `MSI_EVENT_COUNT=0` are expected even with perfect
   MSI delivery.

2. **A remains plausible for actual MSI routing: the MSI infrastructure is real
   but incomplete/hardcoded.** Breenix has GICv2m MSI and MSI-X programming, but
   the loader does not pass the MADT GIC MSI frame to the kernel, there is no
   ITS/LPI implementation, and unknown SPIs are silently dropped. If Linux shows
   xHCI MSI works on the same Parallels VM and Breenix still fails after the SPI
   is armed, this should be treated as a Breenix MSI routing bug.

3. **B is lowest until Linux-probe says otherwise.** The source contains no
   evidence that Parallels cannot deliver MSI. It contains Breenix gaps:
   hardcoded GICv2m discovery, no ACPI-derived PCI INTx route, no ITS, and a
   timer-gated xHCI SPI enable. Per the contract, an environmental limitation
   cannot be claimed without Linux probe evidence.

Important honesty note: the xHCI `MSI irq=56` line is not just an INTx label. If
nonzero, it came from `setup_xhci_msi()` programming plain PCI MSI through
GICv2m. But it only means the PCI MSI message was programmed and an SPI number
was stored. It does not mean the GIC SPI was enabled or that delivery occurred.

## G. Turn 2 Proposal

Collect Linux probe evidence first, without changing Breenix:

```bash
ssh linux-probe 'uname -a'
ssh linux-probe 'lspci -nnvv'
ssh linux-probe 'grep -E "xhci|xhci_hcd|virtio|ahci|GIC|ITS|MSI" /proc/interrupts; cat /proc/interrupts'
ssh linux-probe 'for d in /sys/bus/pci/devices/*; do echo "== $d =="; cat "$d/vendor" "$d/device" "$d/class"; find "$d/msi_irqs" -maxdepth 1 -type f -printf "%f\n" 2>/dev/null; done'
ssh linux-probe 'dmesg | grep -Ei "gic|its|v2m|msi|msi-x|xhci|virtio|ahci|irq"'
```

If `bpftrace` is available on the probe, add a short interrupt attribution run:

```bash
ssh linux-probe 'sudo bpftrace -e '\''tracepoint:irq:irq_handler_entry { @entry[args->irq, str(args->name)] = count(); } tracepoint:irq:irq_handler_exit { @exit[args->irq] = count(); } interval:s:10 { print(@entry); print(@exit); clear(@entry); clear(@exit); }'\'''
```

The first runtime question should be: does Linux route the Parallels xHCI
controller through MSI/MSI-X, and do `/proc/interrupts` counts advance under
USB HID activity? If yes, Breenix must route the same hardware path. If Linux
uses INTx or ACPI platform routing instead, Turn 3 should map the exact Linux
route and implement the equivalent Breenix fallback.
