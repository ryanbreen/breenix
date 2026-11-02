# Catalogue warm-up trace (issue 2645)

The retained failure has sync 1500, context 40, SUBMIT_3D, publication 1572,
descriptor head 59, and used/taken 1571/1571 after the 2000 ms watchdog.
That capture does not locate the loss. No root cause or repair is claimed here.

`gpu_queue_trace` implies the kernel's `testing` feature. Its runtime code is
also gated to aarch64. Production builds and the native GPU backend carry no
trace recording calls. The watchdog, suite and scoring remain unchanged.

## Guest record

The bounded atomic store retains 128 events. Stage numbers are publication (1),
kick entry (2), kick return (3), used-ring observation (4), returned request (5),
IRQ signal (6), watchdog observation (7) and queue enable/reset (8).
Request events retain the queue epoch, lifetime publication ordinal, descriptor
head, command, context, core serial and native wire fence. Kick events carry
the publication watermark the kick covers and its notification address.
Ring observations carry the sampled used/taken indices and the available index
in `detail`; repeated identical observations are omitted. This adds no DMA read
to ordinary polling or publication. A returned request's detail is its used
length. Reset's detail is the descriptor table's physical address.

An unexpected watchdog observation of a request still held by the device
freezes the store before quarantine or reset. An intentionally withheld core
fence already returned by the device does not freeze it; WITHHOLD_ALL also
does not freeze it. These distinctions affect diagnostics alone. `kgpu` posts
the frozen snapshot after releasing its core lock, through fixed deferred
notice nodes. The GPU recording path performs no console output or allocation.
Each `GQ` ticket has five lines. The header reports total written and writer
collisions; ticket gaps and the overwritten prefix remain visible. Missing
records are missing evidence, not successful steps. The store is not thawed
by a device reset.

`tests/gpu_queue_trace_test.py` compiles the same store with warnings rejected
and exercises identity round trips, descriptor reuse across epochs, wide
counters, explicit bounded retention and freezing before a simulated reset.

## Host capture

Use the installed GL bundle, without rebuilding or patching QEMU or its
renderer. `scripts/gpu-queue-trace-qemu.py` selects existing trace events and
refuses a binary missing a requested event. It records its binary/version and
event names in `qemu-gpu-trace.json`, and raw log-backend output in
`qemu-gpu-trace.log`, beside the run's serial. The existing boot launcher still
owns the Mac boot slot, window, Vigil registration and suite verdict.

```sh
export CARGO_BUILD_JOBS=4
export BREENIX_KERNEL_FEATURES=gpu_queue_trace
export BREENIX_QEMU_GL_BINARY="$PWD/scripts/gpu-queue-trace-qemu.py"
scripts/boot-interactive.sh --mode suite --suite gpu --gpu-gl --display \
  --serial-log /tmp/warmup-trace/run-01/serial.txt
```

The installed bundle exposes these relevant events:

| Event | Observation and correlation |
|---|---|
| `virtio_pci_notify_write` | PCI notification address/value/width |
| `virtio_queue_notify` | Device and queue pointers and queue number |
| `virtqueue_pop` | Dequeued element pointer; match its later fill by that pointer |
| `virtio_gpu_cmd_ctx_submit` | Renderer dispatch, context and command-buffer bytes |
| `virtio_gpu_fence_ctrl` | Native fence and command, after renderer dispatch returns |
| `virtio_gpu_fence_resp` | Native fence being completed from the renderer callback |
| `virtqueue_fill`, `virtqueue_flush` | Element fill and the point before the used-index write |
| `virtio_notify` | Queue interrupt notification after the response's used-index write |
| `virtio_set_status` | Device status transitions, including reset |

The trace schema does **not** put a descriptor head or native fence on the
dequeue event, nor a native fence on renderer-submit entry. Those joins use
the split queue's publication/dequeue order within the identified queue and
epoch, and the element pointer through used fill. Treat that join as an
inference, not an extra identity field emitted by QEMU. A `fence_ctrl` has an
explicit native fence; `fence_resp` names the request completed by the callback,
not necessarily the callback's high-water fence. Fill and flush precede
used-index publication; the subsequent `virtio_notify` on the response path
follows it. A suppressed interrupt may omit notify, so flush alone leaves the
exact publication point uncertain.
The raw traces remain intact for analysis; this tool produces no suite verdict.
QEMU's PCI notify address is relative to its notification region; the guest's
kick detail is its mapped virtual address. They are not comparable as address
strings: identify the device/queue, then the publication interval the kick covers.

These meanings were checked against the installed bundle's matching QEMU
10.0.12-utm source: `hw/display/trace-events`, `hw/virtio/trace-events`,
`virtio_gpu_virgl_process_cmd`, `virgl_write_fence` and
`virtio_gpu_ctrl_response`. QEMU's
[tracing documentation](https://www.qemu.org/docs/master/devel/tracing.html)
describes the existing log backend and event selection.

## Validation record

The check-first host run failed with the correlation module absent. The
implementation run passed its three Rust checks. The existing native-fence
ordering host check passed its two Rust checks. Builds and boot results are
recorded in the unit ledger; no clean-run rate is asserted before the boots.
