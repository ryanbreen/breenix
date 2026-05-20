# Turn 2 Relabel PR Close-Out

## A. Edits Applied

All source edits are comments only.

| File:Line | Before | After |
|---|---|---|
| `kernel/src/drivers/usb/xhci.rs:411` | `start_hid_polling` was "deferred from init to after MSI active" | Documents that `DEFERRED_TRB_POLL=0` makes `init()` call `start_hid_polling()` before the GIC SPI is enabled, and that current SPI activation is later in `poll_hid_events()` at `poll >= 50`. |
| `kernel/src/drivers/usb/xhci.rs:571` | Keyboard TRBs were described as deferred to `poll=300` after SPI enable at `poll=200` | Documents current init-time TRB queueing before GIC SPI enable, with later activation in `poll_hid_events()`. |
| `kernel/src/drivers/usb/xhci.rs:577` | Initial HID TRBs were described as deferred until after `XHCI_INITIALIZED` and SPI enable | Documents that the branch sets `XHCI_INITIALIZED` before queueing, but does not wait for GIC SPI enable before queuing HID TRBs. |
| `kernel/src/drivers/usb/xhci.rs:2933` | `configure_hid()` said it starts polling for HID reports | Limits `configure_hid()` to descriptor parsing and endpoint configuration; names `start_hid_polling()` as the later TRB queueing path. |
| `kernel/src/drivers/usb/xhci.rs:4388` | `setup_xhci_msi()` said it configures the GIC and enables the interrupt, and falls back to polling | Says the function programs PCI MSI and configures the GIC trigger mode; SPI enable is later in `poll_hid_events()`. |
| `kernel/src/drivers/usb/xhci.rs:4452` | Step 5 said `init()` enables the SPI after disabling `IMAN.IE` | Says `setup_xhci_msi()` only programs PCI MSI and trigger mode; `poll_hid_events()` enables the SPI after XHCI state is ready. |
| `kernel/src/drivers/usb/xhci.rs:4962` | Init comment said MSI was not configured there and would be configured after `start_hid_polling()` | Documents actual ordering: MSI is programmed before enumeration and before `start_hid_polling()`, while GIC SPI enable remains later. |
| `kernel/src/drivers/usb/xhci.rs:6029` | Requeue path was labeled an MSI-handler fallback | Documents command-wait recovery ownership: `wait_for_event_inner()` sets the flags, timer-side recovery drains them. |
| `kernel/src/drivers/usb/xhci.rs:6160` | Deferred reconfigure block said TRBs are queued inline during enumeration Phase 3 | Documents that `start_hid_polling()` queues TRBs after enumeration when `DEFERRED_TRB_POLL=0`, and the `poll=600` reconfigure path is no longer needed. |
| `kernel/src/drivers/ahci/mod.rs:2279` | Handler doc said "AHCI MSI interrupt handler" | Documents that the handler covers both PCI MSI/MSI-X edge-triggered delivery and platform wired level-triggered IRQs. |
| `kernel/src/drivers/virtio/gpu_pci.rs:1700` and `:1715` | GPU setup doc said "MSI-X or MSI" and GICv2m was needed for both | Documents the current MSI-X requirement; plain MSI is logged as unusable and returns `GpuMsiConfig::NONE`. |

Commit: `bb06768a docs(drivers): honesty pass - relabel 11 misleading driver comments`

## B. Build Verification

- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
  - Result: pass
  - Warning/error grep: no output
  - Log: `turn2-artifacts/build-aarch64.log`
- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
  - Result: pass
  - Warning/error grep: no output
  - Log: `turn2-artifacts/build-x86.log`

## C. Diff Non-Comment Check

Command:

```bash
git diff kernel/src/drivers/ | grep -E "^[+-]" | grep -vE "^\+\+\+|^---|^[+-]\s*//|^[+-]\s*///|^[+-]\s*\*|^[+-]\s*$" | head -n 20
```

Result: no output. The driver source diff is comment-only.

## D. PR URL

https://github.com/ryanbreen/breenix/pull/348

## E. Status

COMPLETE. The 11 verified misleading comments were relabeled, both builds were clean with zero warning/error grep output, the diff was confirmed comment-only, the branch was pushed, and PR #348 was opened.
