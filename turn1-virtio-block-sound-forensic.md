# Turn 1: Linux virtio-block/sound forensic capture

## Verdict

Status: BLOCKED.

The requested runtime capture cannot be completed on the configured
`linux-probe` VM because the VM does not expose a virtio-block disk or a
virtio-sound device. Linux has the virtio-block driver symbols available, but
all exercised block I/O goes through the Parallels ATA/AHCI path:

- `/dev/sda` is bound to `sd` under `platform/PRL4010:00/ata1`.
- `prlctl list -i linux-probe` reports `hdd0 (+) sata:0`, not virtio.
- `/proc/interrupts` increments `ahci[PRL4010:00]` during direct `dd`.
- `virtblk_done` ftrace capture records no calls.
- bpftrace sees `blk_mq_complete_request` and generic wakeups during `dd`, but
  no `virtblk_done` or `virtio_queue_rq`.
- Sound is Intel HDA (`snd_hda_intel`), not virtio-sound.

This turn therefore did not produce the success criterion "Linux's virtio-blk
IRQ completion path with runtime evidence." It produced negative evidence that
the current probe VM is not the right target for that capture.

## A. Linux virtio-blk IRQ completion path

Source references were fetched from upstream Linux stable v6.8 via
`git.kernel.org` into `turn1-artifacts/linux-source/`.

Relevant source pattern:

- `drivers/virtio/virtio_ring.c::vring_interrupt` is the shared virtqueue IRQ
  entry. It invokes `vq->vq.callback(&vq->vq)` after `more_used(vq)` confirms
  used-ring activity and callbacks are enabled.
- `drivers/block/virtio_blk.c::init_vq` registers `virtblk_done` as the
  virtqueue callback.
- `drivers/block/virtio_blk.c::virtblk_done` disables callbacks, repeatedly
  drains used descriptors with `virtqueue_get_buf`, then calls
  `blk_mq_complete_request(req)` for each completed request.
- `drivers/block/virtio_blk.c::virtio_queue_rq` submits requests with
  `virtblk_prep_rq`, `blk_mq_start_request`, `virtqueue_kick_prepare`, and
  `virtqueue_notify`.

Captured excerpts:

- `turn1-artifacts/linux-source/virtio-blk-ring-excerpts.txt`
- `turn1-artifacts/linux-source/virtio_blk.c`
- `turn1-artifacts/linux-source/virtio_ring.c`

Runtime capture attempt:

- `turn1-artifacts/linux-virtio-blk/ftrace-virtblk-done.txt`

The ftrace setup accepted `virtblk_done` as `set_graph_function`, then a direct
read from `/dev/sda` completed, but the trace contained only the function graph
header and no `virtblk_done` call entries. This matches the device inventory:
the disk is not virtio-blk.

bpftrace count capture:

- `turn1-artifacts/linux-virtio-blk/bpftrace-counts.txt`

During `dd if=/dev/sda of=/dev/null bs=1M count=128 iflag=direct`, bpftrace
reported:

```text
@complete: 149
@wake: 553
```

It did not report `@vd_calls`, so `virtblk_done` was not called. The
`blk_mq_complete_request` events are from the AHCI/SCSI-backed `/dev/sda`
path, not virtio-blk.

## B. Linux virtio-sound IRQ completion path

Source references:

- `turn1-artifacts/linux-source/virtio-sound-excerpts.txt`
- `turn1-artifacts/linux-source/virtio_card.c`
- `turn1-artifacts/linux-source/virtio_ctl_msg.c`
- `turn1-artifacts/linux-source/virtio_pcm_msg.c`
- `turn1-artifacts/linux-source/virtio_pcm_ops.c`

Relevant source pattern:

- `sound/virtio/virtio_card.c` registers virtqueue callbacks:
  `virtsnd_ctl_notify_cb`, `virtsnd_event_notify_cb`,
  `virtsnd_pcm_tx_notify_cb`, and `virtsnd_pcm_rx_notify_cb`.
- `sound/virtio/virtio_ctl_msg.c::virtsnd_ctl_notify_cb` drains completed
  control messages with `virtqueue_get_buf` and completes each message via
  `virtsnd_ctl_msg_complete`, which calls `complete(&msg->notify)`.
- `sound/virtio/virtio_pcm_msg.c::virtsnd_pcm_tx_notify_cb` chains into
  `virtsnd_pcm_notify_cb`, which drains completed PCM messages with
  `virtqueue_get_buf` and calls `virtsnd_pcm_msg_complete`.

Runtime virtio-sound capture was not possible on this VM:

- `turn1-artifacts/linux-virtio-sound/preflight.txt`

The VM exposes Intel HDA:

```text
0 [Intel]: HDA-Intel - HDA Intel
00:01.0 Audio device [0403]: Intel Corporation 82801I (ICH9 Family) HD Audio Controller
```

There are no `virtsnd_*` symbols in `/proc/kallsyms`, no virtio-sound driver
binding under `/sys/bus/virtio/drivers`, and no virtio-sound IRQ line.

## C. Submit-side parking pattern

The source-level virtio-blk submit path is visible in
`virtio_blk.c::virtio_queue_rq`, but runtime submit-side parking could not be
captured because no request reaches `virtio_queue_rq` on this VM.

Capture attempt:

- `turn1-artifacts/linux-virtio-blk/bpftrace-submit-park.txt`

During the same direct `/dev/sda` read, the probe did not report
`@submit` for `virtio_queue_rq` and did not report `@sched` for `io_schedule`.
The single printed `try_to_wake_up` stack came from bpftrace/kprobe setup
itself, not the block I/O workload.

No claim is made here about Linux's runtime virtio-blk parking behavior on
Parallels; that still needs a VM with an actual virtio-blk disk.

## D. Shared virtqueue IRQ entry

`drivers/virtio/virtio_ring.c::vring_interrupt` is the common IRQ entry point
that both virtio-blk and virtio-sound use through registered virtqueue
callbacks. The captured source excerpt shows this chain:

1. IRQ enters `vring_interrupt(int irq, void *_vq)`.
2. The virtqueue is checked for used-ring activity with `more_used(vq)`.
3. The callback is skipped if disabled.
4. `vq->vq.callback(&vq->vq)` is invoked.
5. The specific driver callback drains its used ring.

For Breenix, the analogous implementation target is: IRQ handler drains the
used ring and wakes the waiting thread. The syscall caller must not spin on
`queue.has_used()`.

## E. Parallels-specific notes

Artifacts:

- `turn1-artifacts/linux-virtio-blk/preflight.txt`
- `turn1-artifacts/linux-virtio-blk/sysfs-and-irq-confirmation.txt`
- `turn1-artifacts/linux-virtio-blk/prlctl-linux-probe-info.txt`

Device inventory:

- `hdd0` is `sata:0` in Parallels config.
- Linux sees the disk as `/dev/sda` on `platform-PRL4010:00/ata1`.
- The active block IRQ is `GICv3 34 Level ahci[PRL4010:00]`.
- The direct read incremented AHCI IRQ count from `5615` to `5628`.
- Visible virtio devices are balloon, net, GPU, and vsock.
- virtio net/GPU/vsock use MSI/MSI-X queues; no virtio-blk IRQ exists.
- Sound is Intel HDA on MSI IRQ 32, not virtio-sound.

This means the current `linux-probe` VM cannot prove that Linux virtio-blk
completion works on Parallels. It can only prove that this VM's storage path is
not virtio-blk.

## F. Turn 2 proposal

Do not write Breenix virtio-block or virtio-sound code yet. First make the
forensic target valid:

1. Claude/operator should either provide a Parallels Linux VM that exposes a
   real virtio-blk disk, or explicitly authorize adding a scratch virtio disk
   to `linux-probe`.
2. If Parallels can expose virtio-sound, also authorize configuring the probe VM
   with virtio-sound; otherwise keep sound runtime forensics deferred and say so.
3. Re-run the same ftrace/bpftrace plan against the virtio-blk device and
   capture `/proc/interrupts` before/after I/O to identify MSI-X vs legacy INTx.
4. Only after virtio-blk runtime evidence exists should the next turn map
   Breenix's `block.rs` submit/completion paths and IRQ infrastructure.

## Artifact index

- `turn1-artifacts/linux-virtio-blk/preflight.txt`
- `turn1-artifacts/linux-virtio-blk/sysfs-and-irq-confirmation.txt`
- `turn1-artifacts/linux-virtio-blk/ftrace-virtblk-done.txt`
- `turn1-artifacts/linux-virtio-blk/bpftrace-counts.txt`
- `turn1-artifacts/linux-virtio-blk/bpftrace-submit-park.txt`
- `turn1-artifacts/linux-virtio-blk/prlctl-linux-probe-info.txt`
- `turn1-artifacts/linux-virtio-sound/preflight.txt`
- `turn1-artifacts/linux-source/source-availability.txt`
- `turn1-artifacts/linux-source/source-urls.txt`
- `turn1-artifacts/linux-source/key-symbols-rg.txt`
- `turn1-artifacts/linux-source/virtio-blk-ring-excerpts.txt`
- `turn1-artifacts/linux-source/virtio-sound-excerpts.txt`
- `turn1-artifacts/linux-source/virtio_blk.c`
- `turn1-artifacts/linux-source/virtio_ring.c`
- `turn1-artifacts/linux-source/virtio_card.c`
- `turn1-artifacts/linux-source/virtio_ctl_msg.c`
- `turn1-artifacts/linux-source/virtio_pcm.c`
- `turn1-artifacts/linux-source/virtio_pcm.h`
- `turn1-artifacts/linux-source/virtio_pcm_msg.c`
- `turn1-artifacts/linux-source/virtio_pcm_ops.c`
