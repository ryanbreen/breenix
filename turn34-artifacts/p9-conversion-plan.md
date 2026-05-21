# Turn 34: P9 conversion plan

## Target shape

Mirror Linux's division of labor, adapted to Breenix's simpler MMIO GPU driver:

```text
send_command()
  -> write descriptors into CTRL_QUEUE
  -> prepare expected completion token
  -> notify queue 0
  -> sleep on Completion::wait_timeout()

GIC SPI dispatch
  -> gpu_mmio::handle_interrupt()
  -> ack VirtIO MMIO interrupt status
  -> read CTRL_QUEUE.used.idx
  -> complete expected token if used.idx advanced
```

Linux schedules a workqueue to drain vbuffers because it has dynamic vbuffer
objects, fences, callback lists, and multiple outstanding commands. Breenix MMIO
GPU currently serializes commands under `GPU_LOCK` and uses one static command
buffer plus one static response buffer. For P9, the smallest correct shape is
therefore closer to Breenix PCI GPU: the hard IRQ publishes `used.idx` and wakes
the sleeping submitter; the submitter remains responsible for reading the static
response buffer.

## Existing infrastructure to reuse

- `kernel/src/task/completion.rs`: `Completion::new()`, `reset()`,
  `complete(token)`, and `wait_timeout(expected_token, timeout_ns)`.
- `kernel/src/drivers/virtio/gpu_pci.rs`: direct reference for
  `GPU_COMPLETED_USED_IDX`, `GPU_COMPLETION`, `prepare_ctrlq_completion_wait()`,
  `wait_for_ctrlq_completion()`, and IRQ tokening by used idx.
- `kernel/src/drivers/virtio/mmio.rs`: `notify_queue()`,
  `read_interrupt_status()`, and `ack_interrupt()`.
- `kernel/src/drivers/virtio/net_mmio.rs`: MMIO slot tracking and
  `VIRTIO_IRQ_BASE + slot` identity pattern.
- `kernel/src/drivers/virtio/block_mmio.rs` and `sound_mmio.rs`: MMIO IRQ
  completion examples where a hard IRQ acknowledges the device and wakes a
  `Completion`.
- `kernel/src/arch_impl/aarch64/exception.rs`: central SPI dispatch for MMIO
  VirtIO device handlers.

## Infrastructure to add

Expected source changes for the implementation turns:

- `kernel/src/drivers/virtio/gpu_mmio.rs` (~80-130 lines total across turns)
  - Store MMIO slot and HHDM-mapped base at probe time.
  - Add `GPU_MMIO_COMPLETED_USED_IDX: AtomicU32`.
  - Add `GPU_MMIO_COMPLETION: Completion`.
  - Add `get_irq() -> Option<u32>`.
  - Add a hard IRQ `handle_interrupt()`:
    - read `InterruptStatus` from `base + 0x60`
    - if nonzero, write it to `base + 0x64`
    - read `CTRL_QUEUE.used.idx`
    - if it differs from the last published value, store it and complete the
      token for that used idx
  - Add prepare/wait helpers analogous to PCI GPU but scoped to the static
    MMIO control queue.
  - Convert `send_command()` from spin to completion wait after validation.
- `kernel/src/arch_impl/aarch64/exception.rs` (~5-10 lines)
  - Add an MMIO GPU dispatch branch beside input/net/block/sound MMIO handlers.
  - This file is high scrutiny; the change should be limited to dispatch only.

No new softirq type is required for the first P9 implementation. Linux's
deferred drain maps to Breenix's sleeping submitter because Breenix has only one
in-flight MMIO GPU control command under `GPU_LOCK`.

## Per-turn breakdown

### T35: Additive MMIO GPU IRQ completion instrumentation

Single hypothesis: the MMIO GPU control queue produces a GIC SPI interrupt that
can be acknowledged and observed without relying on the polling loop.

Changes:

- Add slot/base tracking, `get_irq()`, and `handle_interrupt()` to
  `gpu_mmio.rs`.
- Wire `gpu_mmio::handle_interrupt()` into the aarch64 SPI dispatch.
- Add atomic counters or non-hot-path trace markers only if needed; do not log in
  the IRQ path.
- Keep the existing `send_command()` spin unchanged.

Pass criteria:

- Clean release build with `testing,external_test_bins`.
- Aarch64 boot reaches the same GPU stage as before.
- Evidence shows the MMIO GPU IRQ handler ran and saw `CTRL_QUEUE.used.idx`
  advance while the old spin path still succeeds. Because the hard IRQ path
  cannot log, evidence should come from non-hot-path counters read after boot or
  a GDB breakpoint/watchpoint.

### T36: Convert MMIO GPU command wait to Completion with spin fallback

Single hypothesis: the IRQ-published used idx can wake the command submitter.

Changes:

- Add `prepare_ctrlq_completion_wait(previous_used_idx)`.
- Add `wait_for_ctrlq_completion(previous_used_idx)`.
- In `send_command()`, prepare the expected token before notify, notify queue 0,
  then call `Completion::wait_timeout()`.
- Preserve the old bounded spin as a temporary fallback only when the completion
  wait times out. The fallback must record explicit evidence that it was used.

Pass criteria:

- Clean release build.
- Aarch64 boot and GPU initialization succeed.
- Evidence shows completion wait success and zero fallback hits on at least one
  boot.

### T37: Remove the MMIO GPU spin fallback

Single hypothesis: the completion path is sufficient and the busy wait is no
longer load-bearing.

Changes:

- Delete the `loop { read CTRL_QUEUE.used.idx; spin_loop(); }` path from
  `gpu_mmio.rs`.
- On timeout or interrupt, return a real GPU command error rather than polling.
- Keep response validation unchanged.

Pass criteria:

- Clean release build.
- Aarch64 boot reaches GPU init completion.
- No `GPU command timeout`.
- Source grep confirms no `spin_loop()` or bounded `used.idx` polling remains in
  `gpu_mmio.rs` for control queue completion.

### T38: Hardening and regression coverage

Single hypothesis: the P9 conversion remains deterministic across repeated boots
and does not regress existing PCI GPU behavior.

Changes:

- Add or update boot-stage markers if needed for MMIO GPU completion evidence.
- Run repeated aarch64 boots and the standard boot-stage health test where
  applicable.
- Compare PCI GPU path untouched unless a shared helper was introduced.

Pass criteria:

- Multiple consecutive aarch64 boots complete the GPU stages.
- No compile warnings.
- No source changes in prohibited hot paths beyond the already-reviewed dispatch
  branch.

## Proposed T35 first step

T35 should be additive only: add MMIO GPU IRQ identity and a hard IRQ handler that
acknowledges the VirtIO MMIO interrupt and publishes `CTRL_QUEUE.used.idx`, then
wire it into the aarch64 SPI dispatcher. Do not alter `send_command()` yet. This
isolates the first question: "Does the MMIO GPU produce usable queue interrupts
under Breenix?"

The T35 implementation should deliberately leave the existing spin in place so
GPU boot behavior remains unchanged while the new IRQ path is measured.

