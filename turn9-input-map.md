# Turn 9 Input Polling Map

## Timer-Side Call Sites

- `kernel/src/arch_impl/aarch64/timer_interrupt.rs`: no live calls to VirtIO input, VirtIO PCI input, or EHCI keyboard polling remain in the timer handler. Existing comments still mention historical keyboard polling in gold-master timer documentation, but there is no input drain call on the timer path.

## VirtIO MMIO Input

- `kernel/src/drivers/virtio/input_mmio.rs`
  - Keyboard and tablet devices already enable their MMIO IRQs during init (`VIRTIO_IRQ_BASE + slot`).
  - `kernel/src/arch_impl/aarch64/exception.rs` already dispatches matching SPIs to `input_mmio::handle_interrupt()` and `input_mmio::handle_tablet_interrupt()`.
  - The attempted Turn 9 patch converted the keyboard used-ring drain from a public polling API into a private IRQ-side drain helper and added counters for IRQs, drained events, and generated keyboard bytes. That source patch was reverted after validation exposed a baseline fresh-deploy CPU0 regression.

## EHCI Keyboard

- `kernel/src/drivers/usb/ehci.rs`
  - Existing code linked a keyboard interrupt qTD into the periodic schedule, then exposed `poll_keyboard()` for timer-side token polling.
  - The attempted Turn 9 patch added PCI MSI/GICv2m setup, enabled EHCI USB transfer interrupts only after the keyboard qTD was linked, and drained completed keyboard qTDs from `handle_interrupt()`.
  - The attempted patch also dispatched the EHCI SPI from `kernel/src/arch_impl/aarch64/exception.rs` to `ehci::handle_interrupt()`.
  - The driver still remains dormant unless EHCI init is invoked by the platform path; current Parallels boot path primarily uses xHCI HID.

## VirtIO PCI Input

- `kernel/src/drivers/virtio/input_pci.rs` is not exported from `kernel/src/drivers/virtio/mod.rs`, and there are no call sites for `input_pci::init()` or `input_pci::poll_events()`.
- The attempted Turn 9 patch removed the orphaned timer-oriented polling implementation block from this dormant file.

## Validation Counters

The attempted Turn 9 patch extended `/proc/xhci/counters` with:

- xHCI MSI/IRQ totals and scheduler stale counters from Turn 8.
- `XHCI_KBD_EVENT_COUNT` for current Parallels USB keyboard evidence.
- `USB_HID_NONZERO_KBD_COUNT` and `USB_HID_LAST_KBD_REPORT_U64` for USB HID report evidence.
- `VIRTIO_INPUT_IRQ_TOTAL`, `VIRTIO_INPUT_EVENT_TOTAL`, and `VIRTIO_INPUT_KEY_BYTES_TOTAL` for VirtIO MMIO input evidence.
- `EHCI_IRQ_TOTAL`, `EHCI_INT_COMPLETIONS`, `EHCI_IRQ_ERROR_TOTAL`, and `EHCI_KBD_BYTES_TOTAL` for EHCI IRQ completion evidence.
