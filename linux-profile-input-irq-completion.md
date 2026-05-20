# Linux Profile: Input IRQ Completion

Status: PARTIAL / BLOCKED

Turn 15 required a Linux profile from the `linux-probe` VM before any P5 source change. The source-side profile is available from prior probe snapshots and local Linux source copies, but the required runtime trace could not be collected because the probe is not currently reachable with the available SSH credentials.

## Probe Access

Attempts made:

- `ssh linux-probe ...` failed host-key verification for the alias.
- `ssh wrb@10.211.55.3 ...` reached SSH but rejected available public keys.
- `ssh parallels@10.211.55.3 ...` reached SSH but rejected available public keys.

Because the operator rule requires a runtime profile from the actual probe VM under matching conditions, Turn 15 does not make a P5 source change.

Raw notes are in `linux-profile-artifacts/input-profile-source-refs.txt`.

## Linux VirtIO Input Source Profile

Source snapshot: `turn1-artifacts/linux-source/virtio_input.c`.

Linux virtio-input is callback-driven:

- `virtinput_recv_events()` is the event virtqueue callback.
- It drains the event virtqueue with `virtqueue_get_buf()`.
- It reports each input event via `input_event()`.
- It requeues the event buffer and kicks the virtqueue.
- `virtinput_init_vqs()` wires the event queue callback through `virtio_find_vqs()`.
- `input_register_device()` publishes the input device to the Linux input layer.

Important citations:

- `virtio_input.c:36-56`: callback drains used buffers, calls `input_event()`, requeues buffers, kicks the queue.
- `virtio_input.c:186-199`: event/status virtqueues are registered with callbacks.
- `virtio_input.c:323`: device registration with the input subsystem.
- `virtio_pci_common.c:82-92`, from the prior source snapshot, shows PCI virtqueue IRQ dispatch through `vring_interrupt()`.

State machine:

1. Device writes used ring entry for a completed input event.
2. Device interrupt reaches the virtio transport.
3. Virtqueue IRQ dispatch invokes the event virtqueue callback.
4. Callback drains completed buffers.
5. Callback reports events to the input core.
6. Callback reposts buffers and notifies the device.

## Linux EHCI Source Profile

Source snapshot: `turn1-artifacts/linux-source/ehci-hcd.c`.

Linux EHCI is host-controller IRQ driven:

- `ehci_irq()` is the host controller interrupt entry.
- It reads and masks `USBSTS`.
- It returns `IRQ_NONE` for shared IRQs with no active status.
- It clears interrupt status bits by writing back the masked status.
- It handles normal/error completion status and schedules bottom-half work.
- The generic `hc_driver` installs `.irq = ehci_irq`.

Important citations:

- `ehci-hcd.c:712-760`: interrupt handler reads status, filters shared IRQs, clears bits, and handles normal/error completion.
- `ehci-hcd.c:1247`: EHCI host-controller driver callback table sets `.irq = ehci_irq`.

State machine:

1. EHCI hardware completes a transfer descriptor and raises controller status.
2. Host IRQ enters `ehci_irq()`.
3. Handler acknowledges controller status.
4. Handler accounts normal/error completions and schedules follow-up work.
5. USB core completion paths deliver URB data upward to HID/input consumers.

## Linux Input / TTY Delivery

Source references:

- `/Users/wrb/fun/code/backups/transcode/home/wrb/code/linux/drivers/input/input.c`
- `/Users/wrb/fun/code/backups/transcode/home/wrb/code/linux/drivers/tty/tty_buffer.c`
- `/Users/wrb/fun/code/backups/transcode/home/wrb/code/linux/drivers/tty/n_tty.c`

Relevant flow:

- `input_event()` feeds `input_handle_event()`.
- `input_pass_values()` distributes values to registered input handlers.
- Serial-style TTY drivers use flip buffers: `tty_insert_flip_string*()` followed by `tty_flip_buffer_push()`.
- `tty_ldisc_receive_buf()` forwards bytes into the active line discipline.
- `n_tty_receive_buf_common()` processes canonical/raw line discipline input.

Breenix does not currently mirror this whole Linux input subsystem. Breenix's consumers are direct:

- VirtIO MMIO keyboard events route to `tty::push_char_nonblock()` with fallback to `ipc::stdin::push_byte_from_irq()`.
- USB HID keyboard reports route through `drivers::usb::hid::process_keyboard_report()`, which then uses the same VirtIO keycode-to-character helpers and TTY/stdin path.

## Breenix Mapping

### VirtIO MMIO Input

Current Breenix code is already IRQ-driven for live VirtIO MMIO input:

- `kernel/src/drivers/virtio/input_mmio.rs` initializes the keyboard queue and enables the GIC IRQ for the keyboard slot.
- `get_irq()` exposes the keyboard SPI.
- `kernel/src/arch_impl/aarch64/exception.rs` dispatches that SPI to `input_mmio::handle_interrupt()`.
- `handle_interrupt()` acknowledges the MMIO interrupt status, drains the used ring through `poll_events()`, and routes key bytes to TTY/stdin.

No live timer call to `input_mmio::poll_events()` was found in `kernel/src`.

### EHCI Keyboard

Current Breenix EHCI is not a clean drop-in conversion:

- `kernel/src/drivers/usb/ehci.rs` explicitly disables EHCI interrupts with `USBINTR = 0`.
- `poll_keyboard()` exists and checks the interrupt qTD token, but no live caller was found in `kernel/src`.
- There is no `ehci::get_irq()` and no `ehci::handle_interrupt()` dispatch from `exception.rs`.
- The EHCI code would need PCI IRQ/MSI or INTx routing, controller interrupt enablement, a status-acknowledging handler, and reuse/refactoring of the qTD completion/resubmit body.

That is a real infrastructure task, not just deletion of two timer calls.

### Dormant VirtIO PCI Input

`kernel/src/drivers/virtio/input_pci.rs:373-429` still contains a polling block that is documented as timer-called, but no live caller was found. This can be deleted as dormant cleanup once the turn is allowed to make source changes; it does not satisfy the Linux runtime-profile requirement by itself.

## Turn 15 Decision

Turn 15 is BLOCKED for the requested implementation because the mandatory runtime profile from `linux-probe` could not be collected.

Even ignoring probe access, the Breenix mapping shows P5 is not a single straightforward source change:

- VirtIO MMIO input already has IRQ completion wiring.
- The named timer polling sites are stale in the current source.
- EHCI needs first-class IRQ infrastructure before it can be converted cleanly.
- The only obvious low-risk code removal is dormant VirtIO PCI input polling, but deleting dormant code alone would not complete P5.

Recommended Turn 16:

1. Restore noninteractive access to `linux-probe`.
2. Collect runtime traces for virtio-input and EHCI/USB IRQ handling, or document that the probe lacks the devices.
3. Split P5 into:
   - P5a: delete dormant VirtIO PCI polling and update stale comments/inventory.
   - P5b: implement EHCI IRQ infrastructure if EHCI is still a supported live path.
   - P5c: add a targeted boot/input validation once the real live path is identified.
