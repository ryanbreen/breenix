# Turn 1 Driver Comment Honesty Audit

## A. Pattern Grep Results

Raw grep outputs are stored in `turn1-artifacts/`.

| Pattern | Artifact | Matches |
|---|---:|---:|
| fallback / no interrupt claims | `turn1-artifacts/pattern1-fallback.txt` | 12 |
| TODO / not used / disabled / unused | `turn1-artifacts/pattern2-disabled.txt` | 17 |
| future-tense driver descriptions | `turn1-artifacts/pattern3-future-tense.txt` | 2 |
| MSI / INTx / legacy interrupt claims | `turn1-artifacts/pattern4-msi-intx.txt` | 242 |
| function references in comments | `turn1-artifacts/pattern5-fn-refs-raw.txt` | 17 |
| race / atomic / lock-free claims | `turn1-artifacts/pattern6-races.txt` | 311 |
| Linux-reference comments | `turn1-artifacts/pattern7-linux-refs.txt` | 6 |
| should-never / impossible claims | `turn1-artifacts/pattern8-cant-happen.txt` | 0 |

The high-volume MSI and race hits are mostly identifiers, diagnostic fields, or accurate comments. The suspect findings below are the ones where the surrounding code contradicts the comment.

## B. Genuinely Suspect Findings

| File:Line | Original Comment (truncated) | Code Path That Contradicts | Severity |
|---|---|---|---|
| `kernel/src/drivers/usb/xhci.rs:411` | `start_hid_polling` is "deferred from init to after MSI active" | `DEFERRED_TRB_POLL` is `0` (`xhci.rs:93`), so `init()` calls `start_hid_polling()` during init (`xhci.rs:5204-5206`). The SPI is not activated until the timer path reaches `poll >= 50` (`xhci.rs:6097-6105`). | HIGH |
| `kernel/src/drivers/usb/xhci.rs:567-576` | Keyboard/HID TRBs are deferred until after SPI enable so the full MSI path is active | The same active path queues TRBs before SPI activation: `start_hid_polling()` sets `KBD_TRB_FIRST_QUEUED` and `HID_TRBS_QUEUED` (`xhci.rs:3915-3916`) from `init()` (`xhci.rs:5204-5206`), while `SPI_ACTIVATED` is set later in `poll_hid_events()` (`xhci.rs:6102-6105`). | HIGH |
| `kernel/src/drivers/usb/xhci.rs:4384-4391` | `setup_xhci_msi()` configures the GIC "and enables the interrupt"; falls back to polling if unavailable | The function only calls `gic::configure_spi_edge_triggered()` and returns the INTID (`xhci.rs:4445-4452`). The actual GIC SPI enable happens later in `poll_hid_events()` (`xhci.rs:6097-6105`). | HIGH |
| `kernel/src/drivers/usb/xhci.rs:4447-4449` | `init()` enables the SPI after disabling `IMAN.IE`; with `IMAN.IE=0` no MSI doorbells fire | `init()` enables `IMAN.IE` (`xhci.rs:4963-4967`) and does not enable the GIC SPI. The deferred SPI enable is in `poll_hid_events()` at `poll >= 50` (`xhci.rs:6097-6105`). | HIGH |
| `kernel/src/drivers/usb/xhci.rs:4957-4961` | "MSI is NOT configured here... MSI will be configured after start_hid_polling()" | `init()` configures MSI before enumeration via `setup_xhci_msi(pci_dev)` (`xhci.rs:5013-5018`), before `scan_ports()` (`xhci.rs:5122-5134`) and before `start_hid_polling()` (`xhci.rs:5204-5206`). | HIGH |
| `kernel/src/drivers/usb/xhci.rs:6026-6027` | "MSI requeue fallback" handles cases where the MSI handler failed to requeue, e.g. lock contention | The only stores to `MSI_*_NEEDS_REQUEUE` are in the command-wait path when a CC=12 Transfer Event is consumed while waiting (`xhci.rs:3720-3740`). The handler does not set these flags; the poll path swaps them at `xhci.rs:6028-6047`. | HIGH |
| `kernel/src/drivers/ahci/mod.rs:2279` | "AHCI MSI interrupt handler" | The handler also services platform wired, level-triggered AHCI IRQs. `probe_platform_irq()` sets `AHCI_IRQ_EDGE` false for wired IRQs (`ahci/mod.rs:2269-2271`); `handle_interrupt()` switches behavior with `check_all = !AHCI_IRQ_EDGE` (`ahci/mod.rs:2371-2374`) and only clears the SPI pending bit for edge-triggered MSI (`ahci/mod.rs:2473-2478`). | HIGH |
| `kernel/src/drivers/virtio/gpu_pci.rs:1700` | "Set up PCI MSI-X or MSI for the VirtIO GPU" | The function configures only MSI-X. If MSI-X is unavailable, plain MSI is only logged as unusable (`gpu_pci.rs:1776-1781`) and the function returns `GpuMsiConfig::NONE` (`gpu_pci.rs:1783-1786`); init then errors if MSI-X is not enabled (`gpu_pci.rs:1912-1914`). | MEDIUM |
| `kernel/src/drivers/virtio/gpu_pci.rs:1714` | GICv2m is "needed for both MSI-X and MSI" | No plain-MSI setup exists in this function. GICv2m is used for the two MSI-X vectors allocated at `gpu_pci.rs:1726-1757`; the plain MSI branch does not program a vector (`gpu_pci.rs:1776-1781`). | MEDIUM |
| `kernel/src/drivers/usb/xhci.rs:2930-2931` | `configure_hid()` parses descriptors, configures endpoints, "and start[s] polling for HID reports" | `configure_hid()` returns after HID class setup (`xhci.rs:3412-3413`). HID polling/TRB queueing is done by `start_hid_polling()` (`xhci.rs:3886-3896`) from `init()` after enumeration (`xhci.rs:5204-5206`) or by the deferred timer path (`xhci.rs:5721-5726`). | MEDIUM |
| `kernel/src/drivers/usb/xhci.rs:6155-6156` | "TRBs are now queued inline during enumeration (Phase 3)" | Phase 3 comments explicitly defer queueing to `start_hid_polling()` (`xhci.rs:3318-3320`, `3348-3350`, `3386-3388`), and the actual call is post-enumeration in `init()` (`xhci.rs:5204-5206`). | MEDIUM |

## C. Already-Addressed Comments Excluded

The following known PR territory was not counted as new work:

- PR #346 / Ralph 2:
  - `kernel/src/drivers/usb/xhci.rs:433` (`MSI_*_NEEDS_REQUEUE` ownership comment)
  - `kernel/src/drivers/usb/xhci.rs:5499` ("Polling Mode fallback" section label)
  - `kernel/src/drivers/usb/xhci.rs:5685-5686` (`poll_hid_events()` safety-net wording)
- PR #347 / Ralph 3:
  - `activate_msi_if_ready()` docstring territory. This branch does not expose that function in `kernel/src/drivers/usb/xhci.rs`, so there was no additional edit candidate here.

## D. Gold-Master Mentions

The requested greps were scoped to `kernel/src/drivers/`, so no gold-master files were matched:

- `kernel/src/arch_impl/aarch64/context_switch.rs`
- `kernel/src/arch_impl/aarch64/gic.rs::init_gicv3_redistributor`
- `kernel/src/arch_impl/aarch64/timer_interrupt.rs`

No gold-master comments were inspected or modified.

## E. Turn 2 Proposal

There are 11 HIGH/MEDIUM findings, concentrated in xHCI plus one AHCI handler label and one VirtIO GPU MSI setup docstring pair. Turn 2 should make a single comment-only relabel commit for all 11 findings, because the blast radius is small and all contradictions are verified against local code paths. No behavior changes, no logging, and no prohibited hot-path files are needed.

Suggested grouping for Turn 2:

1. xHCI MSI/TRB timing comments: update the stale "after MSI active", "MSI configured after polling", and deferred SPI-enable descriptions to match the current order: MSI programmed before enumeration, HID TRBs queued during init when `DEFERRED_TRB_POLL == 0`, and GIC SPI enabled later from `poll_hid_events()`.
2. xHCI `MSI_*_NEEDS_REQUEUE`: say these flags are set by command-wait event handling and drained by the timer path, not by the MSI handler.
3. AHCI handler label: rename the comment to AHCI interrupt handler and mention MSI edge plus wired level-triggered behavior.
4. VirtIO GPU MSI doc: state that the current implementation requires MSI-X and deliberately does not use plain MSI.
