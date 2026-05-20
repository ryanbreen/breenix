# Turn 34 validation

Status: COMPLETE

## What was profiled

Breenix P9 spin site:

- `kernel/src/drivers/virtio/gpu_mmio.rs:338-371`: queue 0 setup.
- `kernel/src/drivers/virtio/gpu_mmio.rs:443-451`: initial
  `GET_DISPLAY_INFO` command enters `send_command()`.
- `kernel/src/drivers/virtio/gpu_mmio.rs:475-511`: descriptor submission and
  `device.notify_queue(0)`.
- `kernel/src/drivers/virtio/gpu_mmio.rs:519-534`: bounded busy wait on
  `CTRL_QUEUE.used.idx`.
- `kernel/src/drivers/virtio/gpu_mmio.rs:562-580`: later command helper reuses
  the same spin path.

Breenix reference event-driven GPU path:

- `kernel/src/drivers/virtio/gpu_pci.rs:1317-1322`: completion globals.
- `kernel/src/drivers/virtio/gpu_pci.rs:1542-1558`: IRQ handler publishes used
  idx and completes a token.
- `kernel/src/drivers/virtio/gpu_pci.rs:2155-2228`: prepare/wait helpers.
- `kernel/src/drivers/virtio/gpu_pci.rs:2248-2317` and `2336-2418`: submit,
  notify, and wait for completion.

Linux profile:

- Probe kernel: `6.8.0-111-generic` on aarch64.
- Driver module: `virtio_gpu`.
- Source citations use captured upstream stable v6.8 excerpts from
  `drivers/gpu/drm/virtio/` because the probe headers did not include that
  driver source directory.
- `virtgpu_vq.c:56-62`: `virtio_gpu_ctrl_ack()` schedules
  `ctrlq.dequeue_work`.
- `virtgpu_vq.c:196-245`: `virtio_gpu_dequeue_ctrl_func()` disables callbacks,
  drains used-ring entries with `virtqueue_get_buf()`, processes responses and
  fences, wakes `ctrlq.ack_queue`, and frees vbuffers.
- `virtgpu_vq.c:314-369`: `virtio_gpu_queue_ctrl_sgs()` adds scatter-gather
  entries and increments `pending_commands`.
- `virtgpu_vq.c:425-445`: `virtio_gpu_notify()` does
  `virtqueue_kick_prepare()` and `virtqueue_notify()`, and
  `virtio_gpu_queue_ctrl_buffer()` wraps the fenced enqueue path.
- `virtgpu_ioctl.c:341-365`: userspace wait surface blocks on reservation
  objects instead of spinning.

## Ftrace evidence

Raw trace is saved at `turn34-artifacts/linux-virtgpu-ftrace.txt`.

Key lines from the live probe:

```text
83409.147470: virtio_gpu_notify <-virtio_gpu_primary_plane_update
83409.147470: virtqueue_kick_prepare <-virtio_gpu_notify
83409.147470: virtqueue_notify <-virtio_gpu_notify
83409.148066: irq_handler_entry: irq=28 name=virtio2-virtqueues
83409.148067: vring_interrupt <-vp_vring_interrupt
83409.148068: virtio_gpu_ctrl_ack <-vring_interrupt
83409.148079: virtio_gpu_dequeue_ctrl_func <-process_one_work
83409.148080: virtqueue_get_buf <-reclaim_vbufs
```

Observed chain: submit/kick -> virtqueue IRQ -> `virtio_gpu_ctrl_ack()` ->
scheduled work -> `virtio_gpu_dequeue_ctrl_func()` -> `virtqueue_get_buf()`.

## Plan summary

P9 should mirror Linux's interrupt-driven completion chain but use Breenix's
existing PCI GPU `Completion` shape rather than a full Linux workqueue drain.
The MMIO GPU has only one in-flight static command under `GPU_LOCK`, so the hard
IRQ can acknowledge the MMIO interrupt, publish the advanced `used.idx`, and
complete the expected token. The sleeping submitter then validates the response
buffer. The implementation should be split into additive IRQ observation first,
then completion wait with a temporary fallback, then fallback deletion.

## Proposed T35 first step

Add MMIO GPU IRQ identity and a hard IRQ handler, wire it into aarch64 SPI
dispatch, and leave the existing `send_command()` spin unchanged. T35 passes if
the old boot path still works and independent evidence shows the new handler saw
control-queue `used.idx` advancement.
