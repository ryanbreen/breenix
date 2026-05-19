# Turn 1: Linux virtio-gpu IRQ behavior on Parallels

## A. Linux virtio-gpu IRQ wiring on Parallels

Probe target:

- VM: `linux-probe`
- Kernel: `Linux probe 6.8.0-107-generic #107-Ubuntu SMP PREEMPT_DYNAMIC Fri Mar 13 19:42:33 UTC 2026 aarch64`
- DRM nodes: `/dev/dri/card1`, `/dev/dri/renderD128`
- Raw artifacts: `turn1-artifacts/linux-probe/`

PCI device:

- `00:0a.0 VGA compatible controller [0300]: Red Hat, Inc. Virtio 1.0 GPU [1af4:1050] (rev 01)`
- `uevent`: `DRIVER=virtio-pci`, `PCI_ID=1AF4:1050`, `PCI_SLOT_NAME=0000:00:0a.0`
- `lspci -vvv`: legacy pin A routes to IRQ 18, but `INTx-` and `MSI-X: Enable+ Count=2 Masked-`. Runtime completion uses MSI-X, not legacy INTx.

Linux IRQ routing:

```text
27:          2          0          0          0       MSI 163840 Edge      virtio2-config
28:      77789          0          0          0       MSI 163841 Edge      virtio2-virtqueues
```

Sysfs confirms GPU `0000:00:0a.0` owns two MSI-X IRQs:

```text
=== /sys/bus/pci/drivers/virtio-pci/0000:00:0a.0/ ===
irq=18
msi_irqs:
  27: msix
  28: msix
```

Architecture observed on Linux 6.8:

- The GPU exposes only two MSI-X vectors. Linux first tries per-vq vectors, but virtio-gpu needs three vectors for config + controlq + cursorq, so it falls back to one config vector plus one shared virtqueue vector.
- Config vector: MSI-X table vector 0 -> Linux IRQ 27, name `virtio2-config`.
- Shared virtqueue vector: MSI-X table vector 1 -> Linux IRQ 28, name `virtio2-virtqueues`.
- Top-half for shared virtqueue MSI-X fallback: `vp_vring_interrupt` in `drivers/virtio/virtio_pci_common.c`.
- `vp_vring_interrupt` walks all virtqueues and calls `vring_interrupt`.
- `vring_interrupt` in `drivers/virtio/virtio_ring.c` checks for used buffers and calls the queue callback.
- virtio-gpu queue callbacks are `virtio_gpu_ctrl_ack` and `virtio_gpu_cursor_ack`.
- Those callbacks schedule work; the bottom halves are workqueue functions `virtio_gpu_dequeue_ctrl_func` and `virtio_gpu_dequeue_cursor_func`, not IRQ-thread functions on this kernel.
- `virtio_gpu_dequeue_ctrl_func` drains used buffers with `virtqueue_get_buf`, processes responses/fences/callbacks, then wakes `ctrlq.ack_queue`.

## B. Runtime evidence

The first non-root `~/virgl_raw_test` run did not generate GPU command traffic:

```text
No DRM device found
=== VirGL Raw Test - byte-for-byte reference ===
```

It moved only network virtio IRQs. That output is preserved in:

- `turn1-artifacts/linux-probe/virgl-irq-delta-run.txt`
- `turn1-artifacts/linux-probe/gpu-irqs.diff`
- `turn1-artifacts/linux-probe/virgl_raw_test.out`

Running the same test via `sudo` opened `/dev/dri/card1`, created a VirGL resource, submitted two command batches, and set the KMS CRTC:

```text
DRM: /dev/dri/card1 - connected 1024x768@60
Batch 1 submitted OK
Batch 2 submitted OK
SetCrtc: OK - display should show CORNFLOWER BLUE
Done.
```

During that successful run, Linux's GPU virtqueue IRQ count advanced:

```diff
- 28:      77789          0          0          0       MSI 163841 Edge      virtio2-virtqueues
+ 28:      77794          0          0          0       MSI 163841 Edge      virtio2-virtqueues
```

So the GPU's shared virtqueue MSI-X vector fired 5 times during the VirGL command load. Raw files:

- `turn1-artifacts/linux-probe/virgl-sudo-irq-delta-run.txt`
- `turn1-artifacts/linux-probe/gpu-irqs-sudo.before`
- `turn1-artifacts/linux-probe/gpu-irqs-sudo.after`
- `turn1-artifacts/linux-probe/gpu-irqs-sudo.diff`
- `turn1-artifacts/linux-probe/virgl_raw_test_sudo_irq.out`

`bpftrace` confirms the Linux interrupt and dequeue path fired while the same successful VirGL load ran:

```text
Attaching 5 probes...
@vp: 22
@vring: 45
@ctrl: 21
@cursor: 0
```

Observed probes:

- `vp_vring_interrupt`: 22
- `vring_interrupt`: 45
- `virtio_gpu_dequeue_ctrl_func`: 21
- `virtio_gpu_dequeue_cursor_func`: 0

Raw file:

- `turn1-artifacts/linux-probe/bpftrace-gpu-run.txt`

Interpretation: Linux is not relying on command-completion polling here. The virtqueue completion path is interrupt-driven through MSI-X IRQ 28, then the virtio ring callback path schedules the virtio-gpu control dequeue worker.

## C. Linux source pointers

Primary source snapshots are saved in `turn1-artifacts/linux-probe/` from upstream Linux `v6.8`.

Key files and functions:

- `linux-v6.8-virtgpu_kms.c`
  - `virtio_gpu_init_vq`: initializes per-queue spinlock, `ack_queue`, and `dequeue_work`.
  - `virtio_gpu_init`: defines callbacks `{ virtio_gpu_ctrl_ack, virtio_gpu_cursor_ack }`, names `{ "control", "cursor" }`, calls `virtio_find_vqs(vgdev->vdev, 2, vqs, callbacks, names, NULL)`, then stores `ctrlq.vq = vqs[0]`, `cursorq.vq = vqs[1]`.
- `linux-v6.8-virtio_pci_common.c`
  - `vp_find_vqs`: tries MSI-X per queue first, then MSI-X shared queue vector, then INTx.
  - `vp_request_msix_vectors`: requests config IRQ and, in shared-vector mode, requests `vp_vring_interrupt` for the shared virtqueue vector.
  - `vp_find_vqs_msix`: in shared-vector mode assigns all queue callbacks to `VP_MSIX_VQ_VECTOR`.
  - `vp_vring_interrupt`: walks all virtqueues and calls `vring_interrupt` for each.
- `linux-v6.8-virtio_ring.c`
  - `vring_interrupt`: checks for used buffers and invokes the queue callback.
- `linux-v6.8-virtgpu_vq.c`
  - `virtio_gpu_ctrl_ack`: schedules `ctrlq.dequeue_work`.
  - `virtio_gpu_cursor_ack`: schedules `cursorq.dequeue_work`.
  - `virtio_gpu_dequeue_ctrl_func`: disables callbacks, reclaims used buffers with `virtqueue_get_buf`, re-enables callbacks, processes responses/fences/callbacks, and wakes `ctrlq.ack_queue`.
  - `virtio_gpu_queue_ctrl_sgs`: if the queue lacks free descriptors, it calls `wait_event(vgdev->ctrlq.ack_queue, vq->num_free >= elemcnt)` instead of polling.
  - `virtio_gpu_notify`: calls `virtqueue_kick_prepare` and `virtqueue_notify`.

## D. Breenix codepath map

Current Breenix already has partial MSI-X plumbing, but it does not yet match the Linux topology or eliminate polling.

Initialization and IRQ routing:

- `kernel/src/drivers/virtio/gpu_pci.rs:1577` `setup_gpu_msi()` probes GICv2m, allocates one MSI SPI, programs every MSI-X table entry to the same SPI, enables MSI-X, and disables INTx.
- `kernel/src/drivers/virtio/gpu_pci.rs:1765` computes `queue_vector = 0` and `config_vector = 0` when MSI-X exists.
- `kernel/src/drivers/virtio/gpu_pci.rs:1769` writes `config_msix_vector`.
- `kernel/src/drivers/virtio/gpu_pci.rs:1818` writes queue 0 `queue_msix_vector`.
- `kernel/src/drivers/virtio/gpu_pci.rs:1857` writes queue 1 `queue_msix_vector`.
- `kernel/src/drivers/virtio/gpu_pci.rs:1241` `enable_gpu_yield()` later disables config MSI-X (`0xFFFF`), rewrites controlq vector 0, and enables the single SPI.
- `kernel/src/arch_impl/aarch64/exception.rs:1276` dispatches the GPU SPI to `gpu_pci::handle_interrupt()`.
- `kernel/src/drivers/virtio/gpu_pci.rs:1653` `handle_interrupt()` disables the SPI, reads the VirtIO ISR status, sets `GPU_CMD_COMPLETE`, wakes `GPU_WAITING_THREAD` via `sched.unblock`, and re-enables the SPI.
- `kernel/src/drivers/virtio/pci_transport.rs:535` and `:552` expose `set_config_msix_vector()` and `set_queue_msix_vector()`.
- `kernel/src/drivers/virtio/pci_transport.rs:571` reads the ISR status register, which clears it.
- `kernel/src/arch_impl/aarch64/gic.rs:827` configures MSI SPIs edge-triggered.
- `kernel/src/arch_impl/aarch64/gic.rs:847` enables and routes an SPI.

Important mismatch with Linux:

- Linux on this Parallels VM uses two MSI-X vectors: vector 0 for config, vector 1 shared by both virtqueues.
- Breenix currently programs all MSI-X entries to the same SPI and assigns queue vector 0. That does not reproduce Linux's `virtio2-config` plus `virtio2-virtqueues` layout.

Polling locations:

- `kernel/src/drivers/virtio/gpu_pci.rs:2206` documents `send_command()` as "spin-wait for completion".
- `kernel/src/drivers/virtio/gpu_pci.rs:2282` explicitly suppresses interrupts for 2-desc commands with `VRING_AVAIL_F_NO_INTERRUPT`.
- `kernel/src/drivers/virtio/gpu_pci.rs:2299` begins the tight used-ring polling loop for 2-desc commands.
- `kernel/src/drivers/virtio/gpu_pci.rs:2308-2348` invalidates the used ring cache line, reads `used.idx`, and `spin_loop()`s until completion or timeout.
- `kernel/src/drivers/virtio/gpu_pci.rs:2455-2605` has a partial MSI-X path for 3-desc commands, but it still repeatedly checks `used.idx`, uses 10 ms timer fallback attempts, then suppresses interrupts after waking.
- `kernel/src/drivers/virtio/gpu_pci.rs:2606-2674` retains the 3-desc polling fallback and timer-assisted polling loop.

Existing scheduler primitives relevant to Turn 2:

- `kernel/src/task/scheduler.rs:1837` `block_current_for_compositor()` can block the current thread with a timeout.
- `kernel/src/task/scheduler.rs:2692` `isr_unblock_for_io()` is the lock-free ISR wakeup path; using it would avoid taking the scheduler lock directly in the GPU ISR.

## E. Proposed Turn 2 design

Implement the Linux-shaped interrupt path first: allocate MSI-X vectors like Linux does on Parallels, with vector 0 for config and vector 1 as the shared virtqueue vector for both controlq and cursorq; program vector 0 to a config SPI and vector 1 to a dedicated GPU virtqueue SPI, then dispatch the virtqueue SPI to a minimal completion ISR. Add a small control-queue completion wait state (single waiter is enough while the GPU PCI state lock serializes command submission) that records the expected `used.idx`, enables device notifications before notify, blocks the current thread on a queue wait condition, and wakes through `isr_unblock_for_io()` when the ISR observes a used-ring advance. Then replace both `send_command()` and `send_command_3desc()` completion waits with the same interrupt-driven helper, removing the tight polling loops and leaving only a one-shot timeout/error path rather than repeated timer polling. The first correctness patch should also stop writing `VRING_AVAIL_F_NO_INTERRUPT` on normal command submission and should remove the "all MSI-X entries use one SPI / queue vector 0" shortcut so Breenix matches Linux's shared `virtio2-virtqueues` mechanism.
