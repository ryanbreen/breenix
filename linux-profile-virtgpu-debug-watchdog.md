# Linux Profile: VirtGPU Debug Watchdog

Status: SOURCE PROFILE ONLY

Turn 16 could not collect a live runtime trace from `linux-probe` because the same noninteractive SSH failures from Turn 15 remain: the `linux-probe` alias fails host-key verification, and direct SSH to `wrb@10.211.55.3` or `parallels@10.211.55.3` rejects available public keys. This profile therefore uses source citations from the saved Linux source snapshots.

Raw citation notes are in `linux-profile-artifacts/virtgpu-watchdog-source-refs.txt`.

## Linux VirtGPU Source Shape

Source snapshots:

- `turn1-artifacts/linux-source/virtgpu_vq.c`
- `turn1-artifacts/linux-probe/linux-v6.8-virtgpu_kms.c`
- `turn1-artifacts/linux-probe/linux-v6.8-virtgpu_drv.h`

Linux virtgpu completion handling is event-driven:

- `virtio_gpu_ctrl_ack()` is the control virtqueue callback and schedules `ctrlq.dequeue_work`.
- `virtio_gpu_cursor_ack()` does the same for the cursor queue.
- `virtio_gpu_init_vq()` initializes a queue lock, wait queue, and dequeue work item.
- `virtio_gpu_init()` registers the control and cursor callbacks and initializes both dequeue work items.
- `virtio_gpu_dequeue_ctrl_func()` disables callbacks, drains used buffers with `virtqueue_get_buf()`, re-enables callbacks, then processes responses outside the queue lock.
- When descriptors are unavailable, `virtio_gpu_queue_ctrl_sgs()` notifies the device and blocks on `wait_event()` for queue space instead of polling.
- Initialization waits such as display info use `wait_event_timeout()` rather than a periodic diagnostic thread.
- Deinit flushes dequeue work before deleting virtqueues.

Important citations:

- `virtgpu_vq.c:57-70`: virtqueue callbacks schedule dequeue work.
- `virtgpu_vq.c:217-241`: control dequeue work drains used buffers with callback disable/re-enable.
- `virtgpu_vq.c:372-399`: control queue submission uses `wait_event()` when virtqueue descriptors are unavailable.
- `linux-v6.8-virtgpu_kms.c:60-64`: queue state initializes dequeue work.
- `linux-v6.8-virtgpu_kms.c:120-148`: virtgpu initialization registers callbacks and work items.
- `linux-v6.8-virtgpu_kms.c:257-288`: init/deinit wait and flush work paths.
- `linux-v6.8-virtgpu_drv.h:201`, `398-401`, `472-473`: queue work structure, dequeue function prototypes, and debugfs hook.

## Linux State Machine

1. Driver submits control/cursor queue work and notifies the device.
2. Device completes one or more virtqueue buffers and raises the virtqueue interrupt.
3. Virtqueue callback schedules dequeue work.
4. Dequeue work disables callbacks, drains used buffers, and re-enables callbacks in a race-aware loop.
5. Response handling wakes waiters, completes fences, updates queue space waiters, and returns buffers to the driver.
6. Debugging/inspection is available through DRM/debug infrastructure, not through a driver kthread that periodically samples unrelated scheduler and process-manager locks.

## Breenix Mapping

Breenix GPU PCI command completion is already interrupt-driven:

- `kernel/src/drivers/virtio/gpu_pci.rs` stores `GPU_COMPLETED_USED_IDX` and signals `GPU_COMPLETION`.
- `kernel/src/drivers/virtio/gpu_pci.rs` installs MSI-X queue/config vectors during PCI setup.
- `kernel/src/drivers/virtio/gpu_pci.rs` `handle_interrupt()` reads the used index and completes the wait token when progress is observed.

The P10 polling behavior is not the command completion path. It is the diagnostic freeze watchdog:

- `kernel/src/drivers/virtio/gpu_pci.rs` has `FREEZE_WATCH_STARTED`, `start_freeze_watchdog()`, and `freeze_watchdog_thread()`.
- The watchdog sleeps for 500 ms during early runtime, then 5 seconds later, and wakes forever.
- Each wake samples GPU completion counters, FPS, current CPU, scheduler context-switch and run-queue state, per-CPU timer ticks, process-manager lock status, and GPU PCI lock status.
- It emits `[freeze-watch]` serial lines periodically and emits GPU lock attribution plus scheduler wake attribution about every 30 seconds.
- `kernel/src/main_aarch64.rs` starts the watchdog after GPU initialization.

Because GPU PCI already has IRQ-driven completion, P10 can be handled by deleting this periodic diagnostic thread or moving the useful counters to an on-demand proc/debug endpoint. A Turn 17 implementation should avoid touching the GPU interrupt completion path except as needed to keep counters available.

## Turn 17 Recommendation

Remove or relocate the periodic freeze watchdog:

1. Delete `start_freeze_watchdog()`, `freeze_watchdog_thread()`, and helper code that exists only for periodic sampling.
2. Remove the call site in `main_aarch64.rs` only if that file is approved for the next turn, or gate the function to a no-op if the orchestrator keeps the same hard constraint.
3. Preserve useful observability as counters updated by existing GPU paths, preferably readable on demand through proc/debug output.
4. Validate with one fresh Parallels boot: heartbeat through about 60 seconds, no CPU0 timer regression, no panic.

This is the highest-confidence next target because it removes periodic polling without adding new IRQ infrastructure.
