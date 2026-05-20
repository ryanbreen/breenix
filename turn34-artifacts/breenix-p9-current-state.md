# Turn 34: Breenix P9 current state

## Scope

P9 covers the VirtIO GPU MMIO control queue. This turn is documentation-only.
No Breenix boot was run.

## Current MMIO GPU control queue

The MMIO GPU driver is `kernel/src/drivers/virtio/gpu_mmio.rs`.

The control queue is configured in `init_device()`:

- `gpu_mmio.rs:338-371`: selects queue 0, caps the queue at 16 entries, initializes
  `CTRL_QUEUE.avail` and `CTRL_QUEUE.used`, and writes the queue addresses.
- `gpu_mmio.rs:358-361`: both `avail.flags` and `used.flags` are initialized to 0,
  so the driver is not deliberately suppressing used-ring notifications.
- `gpu_mmio.rs:374-375`: sets `DRIVER_OK`.

The first command that hits the spin is display discovery:

- `gpu_mmio.rs:431-440`: writes a `VIRTIO_GPU_CMD_GET_DISPLAY_INFO` header into
  `CMD_BUF`.
- `gpu_mmio.rs:443-451`: calls `send_command()` with `CMD_BUF` and `RESP_BUF`.

The shared submission helper is:

- `gpu_mmio.rs:475-511`: writes descriptor 0 as the command buffer, descriptor 1
  as the writable response buffer, advances `avail.idx`, then calls
  `device.notify_queue(0)`.
- `gpu_mmio.rs:513-534`: busy-waits for `CTRL_QUEUE.used.idx` to differ from
  `state.last_used_idx`, with a 10,000,000-iteration timeout and
  `core::hint::spin_loop()`.
- `gpu_mmio.rs:562-580`: `send_command_expect_ok()` wraps the same `send_command()`
  helper for most later control commands, then checks `RESP_OK_NODATA`.

The exact spin site is `kernel/src/drivers/virtio/gpu_mmio.rs:519-534`:

```rust
    let mut timeout = 10_000_000u32;
    loop {
        fence(Ordering::SeqCst);
        let used_idx = unsafe {
            let ptr = &raw const CTRL_QUEUE;
            read_volatile(&(*ptr).used.idx)
        };
        if used_idx != state.last_used_idx {
            state.last_used_idx = used_idx;
            break;
        }
        timeout -= 1;
        if timeout == 0 {
            return Err("GPU command timeout");
        }
        core::hint::spin_loop();
    }
```

## What is not wired for MMIO GPU

`gpu_mmio.rs` has no `get_irq()` or `handle_interrupt()` entry point. The aarch64
SPI dispatcher covers MMIO input, net, block, sound, USB, PCI GPU, PCI net, and
AHCI in `kernel/src/arch_impl/aarch64/exception.rs:1251-1305`, but there is no
MMIO GPU dispatch branch there today.

The shared VirtIO MMIO transport already exposes the device interrupt registers:

- `kernel/src/drivers/virtio/mmio.rs:298-300`: `notify_queue(queue)`.
- `kernel/src/drivers/virtio/mmio.rs:303-310`: `read_interrupt_status()` and
  `ack_interrupt(flags)`.

Existing MMIO drivers show the local IRQ pattern:

- `net_mmio.rs:235-246`: stores the MMIO slot and uses `VIRTIO_IRQ_BASE + slot`.
- `net_mmio.rs:276-291`: records the slot and virtual MMIO base during probe.
- `net_mmio.rs:731-745`: enables the GIC IRQ for the MMIO slot.
- `net_mmio.rs:747-785`: publishes `get_irq()` and handles interrupts by reading
  `InterruptStatus`, writing `InterruptACK`, and deferring work to softirq.
- `block_mmio.rs:607-613`: enables the MMIO device GIC IRQ.
- `block_mmio.rs:925-950`: hard IRQ path reads `InterruptStatus`, writes
  `InterruptACK`, then completes a waiting request.

## PCI GPU reference design

The PCI GPU path is already event-driven and is the closest Breenix reference.

- `kernel/src/drivers/virtio/gpu_pci.rs:1317-1322`: defines
  `GPU_COMPLETION_TIMEOUT_NS`, `GPU_COMPLETED_USED_IDX`, and `GPU_COMPLETION`.
- `gpu_pci.rs:1542-1558`: `handle_interrupt()` reads the latest used idx and
  completes `GPU_COMPLETION` with a token derived from that used idx.
- `gpu_pci.rs:2155-2162`: `prepare_ctrlq_completion_wait()` verifies an IRQ is
  configured, resets the completion, and seeds `GPU_COMPLETED_USED_IDX` with the
  previous used idx.
- `gpu_pci.rs:2183-2228`: `wait_for_ctrlq_completion()` blocks with
  `GPU_COMPLETION.wait_timeout(expected_token, GPU_COMPLETION_TIMEOUT_NS)`, then
  verifies `used.idx` advanced and updates `state.last_used_idx`.
- `gpu_pci.rs:2248-2317` and `gpu_pci.rs:2336-2418`: both two-descriptor and
  three-descriptor submit paths drain stale completions, prepare the wait,
  write descriptors, enable queue interrupts, notify the device, and wait for
  the IRQ-driven completion.

## Delta: PCI completion vs. MMIO spin

| Concern | PCI GPU | MMIO GPU |
| --- | --- | --- |
| IRQ identity | MSI-X queue SPI in `GPU_IRQ` | No stored MMIO slot IRQ |
| Interrupt entry | `gpu_pci::handle_interrupt()` wired in `exception.rs` | No `gpu_mmio::handle_interrupt()` |
| Completion state | `GPU_COMPLETED_USED_IDX` plus `GPU_COMPLETION` | Only `GpuDeviceState.last_used_idx` |
| Submit wait | `Completion::wait_timeout()` | Busy loop on `CTRL_QUEUE.used.idx` |
| Notification suppression | `enable_ctrlq_interrupts()` before notify | `avail.flags = 0`, no explicit re-enable/drain race handling |
| Used-ring owner | IRQ path publishes used idx | Submitter polls used idx directly |

## Required infrastructure delta

The Linux-shaped MMIO conversion should be additive first:

1. Store the MMIO GPU slot and HHDM-mapped base in `gpu_mmio.rs`, like
   `net_mmio.rs`, so the IRQ handler can read `InterruptStatus` and write
   `InterruptACK` without probing or locking.
2. Add `gpu_mmio::get_irq()` and `gpu_mmio::handle_interrupt()`.
3. In the handler, keep the hard IRQ path bounded: acknowledge the MMIO interrupt,
   read `CTRL_QUEUE.used.idx`, publish the new used idx in an atomic, and complete
   a `Completion` token. No logging, allocation, locks, or ring draining in IRQ.
4. Wire the handler in `arch_impl/aarch64/exception.rs` alongside the other MMIO
   VirtIO devices. This file is high scrutiny, so the implementation turn should
   justify the small dispatch addition explicitly.
5. Convert `send_command()` to prepare an expected used idx, notify queue 0, then
   use `Completion::wait_timeout()`. Keep the existing spin as a redundant fallback
   for the first validation turn only, then remove it after the IRQ completion path
   is proven.

