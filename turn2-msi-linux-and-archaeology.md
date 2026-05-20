# Turn 2: Linux Probe MSI Evidence and xHCI SPI-Enable Archaeology

## A. Linux Probe Evidence Summary

Current linux-probe state:

- Host: `wrb@10.211.55.3`
- Kernel: `Linux probe 6.8.0-111-generic #111-Ubuntu SMP PREEMPT_DYNAMIC Sat Apr 11 22:59:23 UTC 2026 aarch64`
- Captures: `turn2-artifacts/linux-probe/`

Linux binds the Parallels NEC xHCI controller at PCI `0000:00:03.0` to
`xhci_hcd` with plain PCI MSI enabled:

```text
00:03.0 USB controller [0c03]: NEC Corporation uPD720200 USB 3.0 Host Controller [1033:0194]
Control: ... BusMaster+ ... DisINTx+
Interrupt: pin A routed to IRQ 26
Capabilities: [e0] MSI: Enable+ Count=1/1 Maskable+ 64bit-
        Address: 02250040  Data: 0038
        Masking: 00000000  Pending: 00000000
Kernel driver in use: xhci_hcd
```

`/proc/interrupts` shows xHCI on Linux logical IRQ 26 through MSI hwirq
`49152`, edge-triggered:

```text
26:        137          0          0          0       MSI 49152 Edge      xhci_hcd
```

The idle and after-30s snapshots did not show xHCI advancing (`137 -> 137`).
That is expected for this SSH-only capture because no HID activity was
exercisable during the window. The important point is that the count is already
nonzero and Linux's dmesg shows several USB devices enumerated through
`xhci_hcd`, which requires working xHCI event delivery on this hypervisor:

```text
usb 3-1: new SuperSpeed USB device number 2 using xhci_hcd
usb 3-2: new SuperSpeed USB device number 3 using xhci_hcd
usb 3-4: new SuperSpeed USB device number 4 using xhci_hcd
```

HID-specific interrupt advancement remains unexercised in this turn's capture,
but xHCI MSI delivery on Parallels aarch64 is confirmed.

## B. Per-Device MSI IRQ Map

From `turn2-artifacts/linux-probe/msi-irqs-per-device.txt`:

| PCI device | Linux IRQ(s) | Notes |
| --- | ---: | --- |
| `0000:00:01.0` HDA | 32 | Plain MSI enabled, data `0x003d` |
| `0000:00:03.0` xHCI | 26 | Plain MSI enabled, data `0x0038` |
| `0000:00:05.0` virtio-net | 20, 21, 22 | MSI-X enabled, 3 vectors |
| `0000:00:0a.0` virtio-gpu | 27, 28 | MSI-X enabled, 2 vectors |
| `0000:00:0e.0` virtio-socket | 29, 30 | MSI-X enabled, 2 vectors |

The xHCI MSI message data is `0x0038`, decimal 56. That matches the Breenix
`MSI irq=56` observation from Turn 1: Linux logical IRQ 26 is not the GIC SPI;
the MSI data is.

AHCI and EHCI in this Linux VM use wired GIC interrupts, not MSI:

```text
15:      19421          0          0          0     GICv3  34 Level     ahci[PRL4010:00]
23:          0          0          0          0     GICv3  35 Level     ehci_hcd:usb1
```

## C. GICv2m Bringup on Linux

Linux discovers the same GICv2m frame Breenix probes:

```text
GICv3: 988 SPIs implemented
GICv2m: range[mem 0x02250000-0x02250fff], SPI[53:116]
ACPI: Using GIC for interrupt routing
```

The xHCI MSI address is the GICv2m doorbell at `0x02250000 + 0x40 =
0x02250040`; the xHCI MSI data is absolute SPI 56. This matches the existing
Breenix `setup_xhci_msi()` model in `kernel/src/drivers/usb/xhci.rs:4384`:
probe GICv2m, allocate an SPI, write doorbell `base + 0x40`, write MSI data as
the allocated SPI, and configure the GIC SPI edge-triggered.

## D. Hypervisor Verdict

Verdict: Parallels' aarch64 GICv2m delivers xHCI MSI.

The current probe gives three independent confirmations:

- Linux has xHCI MSI enabled in PCI config (`MSI: Enable+`, `DisINTx+`).
- `/proc/interrupts` has a nonzero `xhci_hcd` MSI count.
- Linux enumerated SuperSpeed USB devices via `xhci_hcd`.

The 30-second idle window did not add xHCI interrupts, so this turn does not
prove HID-event advancement under live pointer/keyboard input. It does prove
that the hypervisor can deliver xHCI MSI and that Breenix's dependency on the
CPU0 timer for first SPI enable is a kernel design issue, not a GICv2m absence.

## E. Git Archaeology

Artifacts:

- `turn2-artifacts/git-archaeology/commits-with-poll-50.txt`
- `turn2-artifacts/git-archaeology/commits-with-spi-activated.txt`
- `turn2-artifacts/git-archaeology/commit-list.txt`
- `turn2-artifacts/git-archaeology/commit-messages.txt`
- `turn2-artifacts/git-archaeology/pr-333.json`
- `turn2-artifacts/git-archaeology/pr-333-body.md`

The relevant history is:

1. `0f641606 feat: interrupt-driven xHCI keyboard - eliminate 20ms poll latency`

   This commit introduced the current `SPI_ACTIVATED` one-shot and changed the
   first activation delay from an older poll-based delay to `poll >= 50`.
   Its commit message says the previous handler disabled the GIC SPI after each
   interrupt and relied on a 50 Hz timer poll to re-enable it, adding up to
   20 ms latency. It made `handle_interrupt()` re-enable the SPI after draining
   the event ring, while keeping a one-shot timer-poll activation for the first
   enable.

2. `8e066410 fix(usb): F32t Phase 4 enable xHCI MSI delivery`

   This commit changed `XhciState.irq` from `0` to `early_irq`, with the
   rationale that Linux-order PCI MSI programming should remove the historical
   MSI-storm condition. The first GIC SPI enable still remained inside
   `poll_hid_events()`.

3. `703db7de fix(usb): F32t Phase 5a move xHCI SPI enable to end of init`

   This was the simple fix candidate: enable the SPI at the end of `xhci::init()`
   after MSI programming, `state.irq`, IMAN.IE, USBCMD.RS, enumeration, and HID
   TRB queueing. It removed the poll-counter path as the only first activation.

4. `c077c6ba Revert "fix(usb): F32t Phase 5a move xHCI SPI enable to end of init"`

   The revert commit itself only says it reverts `703db7de`, but PR #333's body
   explains why: "First attempt at Phase 5 (enabling SPI inline at xhci::init()
   completion) was reverted - it fires too early, before AHCI/FS/scheduler finish
   initialization, causing disk reads to stall. Needs a deferred trigger tied to
   system-ready, not a poll counter."

That PR context is important. The deferred SPI enable is not just accidental
technical debt: it avoided enabling xHCI interrupts before the rest of boot was
ready. The bug is that the deferral trigger is CPU0 timer polling, so if CPU0
timer progress dies before `poll >= 50`, xHCI MSI can never start.

There were no matching planning-doc references under `docs/planning/` from the
directive grep. The PR body is the clearest rationale found.

## F. GPU Comparison

VirtIO GPU programs MSI-X entries, configures SPIs, enables MSI-X/DisINTx, then
enables the GIC SPIs during GPU init after state is stored and completion state
is reset.

Relevant code:

```rust
// kernel/src/drivers/virtio/gpu_pci.rs:1753
pci_dev.configure_msix_entry(msix_cap, GPU_MSIX_CONFIG_VECTOR, msi_address, config_spi);
pci_dev.configure_msix_entry(msix_cap, GPU_MSIX_QUEUE_VECTOR, msi_address, queue_spi);

gic::configure_spi_edge_triggered(config_spi);
gic::configure_spi_edge_triggered(queue_spi);

pci_dev.enable_msix(msix_cap);
pci_dev.disable_intx();
```

```rust
// kernel/src/drivers/virtio/gpu_pci.rs:2075
GPU_CONFIG_IRQ.store(msi_config.config_spi, Ordering::Release);
GPU_IRQ.store(msi_config.queue_spi, Ordering::Release);
GPU_COMPLETED_USED_IDX.store(0, Ordering::Release);
GPU_COMPLETION.reset();

gic::clear_spi_pending(msi_config.config_spi);
gic::clear_spi_pending(msi_config.queue_spi);
gic::enable_spi(msi_config.config_spi);
gic::enable_spi(msi_config.queue_spi);
```

AHCI also enables its SPI immediately in `setup_ahci_msi()` because its commands
are serialized by the AHCI controller mutex:

```rust
// kernel/src/drivers/ahci/mod.rs:2049
gic::clear_spi_pending(spi);
gic::enable_spi(spi);
```

xHCI is different because `drivers::init()` initializes xHCI before AHCI:

```text
kernel/src/drivers/mod.rs:229-247  xHCI init
kernel/src/drivers/mod.rs:261-283  AHCI init
```

Then `main_aarch64.rs` mounts ext2 and preloads `/sbin/init` after
`drivers::init()`:

```text
kernel/src/main_aarch64.rs:561-578  drivers::init(), then ext2 root mount
kernel/src/main_aarch64.rs:836-865  preload init before timer init
```

That ordering explains the reverted init-time xHCI SPI enable: it can fire while
storage/filesystem boot reads are still in progress.

## G. Hypothesis Verdict

Recommended fix shape:

Keep first xHCI SPI activation deferred, but move the trigger to a
non-CPU0-timer-dependent system-ready point.

Do not simply enable the SPI at the end of `xhci::init()`. That exact patch was
already attempted in `703db7de` and reverted in `c077c6ba`; PR #333 says it
fires too early and stalls disk reads.

Do not keep the only first-enable trigger in `poll_hid_events()`. Current code
still gates first enable on `poll >= 50`:

```rust
// kernel/src/drivers/usb/xhci.rs:6102
if state.irq != 0 && poll >= 50 && !SPI_ACTIVATED.load(Ordering::Relaxed) {
    SPI_ACTIVATED.store(true, Ordering::Release);
    crate::arch_impl::aarch64::gic::clear_spi_pending(state.irq);
    crate::arch_impl::aarch64::gic::enable_spi(state.irq);
    DIAG_SPI_ENABLE_COUNT.fetch_add(1, Ordering::Relaxed);
}
```

The right next patch is an explicit activation helper, callable after storage,
filesystem, scheduler, and boot-critical reads are ready, while leaving timer
polling as housekeeping/rescue rather than the only first-enable path.

## H. Turn 3 Proposal

Concrete patch plan:

1. Add an idempotent xHCI helper, likely `activate_msi_if_ready()`:
   - require `XHCI_INITIALIZED == true`
   - read `XHCI_STATE`
   - if `state.irq != 0` and `SPI_ACTIVATED.compare_exchange(false, true, ...)`
     succeeds, clear pending and enable the SPI
   - increment `DIAG_SPI_ENABLE_COUNT`
   - avoid hot-path logging

2. Call that helper from a system-ready boot point, not from `xhci::init()`:
   - after `drivers::init()` has completed
   - after ext2 root/home mount and `/sbin/init` preload are complete
   - before enabling interrupts for userspace/test dispatch

   Candidate call sites need a small Turn 3 source pass, but
   `kernel/src/main_aarch64.rs` after init preload and before test/userspace
   dispatch is the likely location.

3. Keep the `poll_hid_events()` `poll >= 50` activation for one commit as a
   fallback during validation, but make it call the same helper. Once boot-stage
   evidence shows system-ready activation runs first, remove or downgrade the
   timer path so CPU0 timer death cannot block first xHCI MSI enable.

4. Verify with normal boot stages first. If it fails, use GDB rather than adding
   hot-path logging to syscall/interrupt code.

Status: COMPLETE. Linux xHCI MSI delivery is confirmed; git archaeology explains
why the simple init-time enable was reverted; Turn 3 should implement a
system-ready, non-CPU0-timer-dependent first activation.
