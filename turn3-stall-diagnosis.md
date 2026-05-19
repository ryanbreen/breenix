# Turn 3 Virtio-GPU Stall Diagnosis

## A. tid=13 identity

`tid=13` is the BWM userspace thread in PID 3.

Evidence:

- `turn2-artifacts/reproduce-run1/run.out:892-903` shows `/bin/bwm` spawning as PID 3.
- `turn2-artifacts/reproduce-run1/gdb_softlock_state.out:75` locates `tid=13` as not present on ready queues during the captured stall.
- Freeze-watch samples repeatedly show `cur_cpu*=13` while `tid=3` is the reporting BWM process, matching the BWM render loop rather than an init, heartbeat, workqueue, or client thread.

## B. Hypothesis evidence

Turn 3 found two related problems.

### H3: a Ready thread could become unreachable from scheduler queues

Turn 2's GDB snapshot showed `tid=13` Ready but not current, not queued, and not deferred:

- `turn2-artifacts/reproduce-run1/gdb_softlock_state.out:75`: `tid_location tid=13 current_cpus=[1] previous_cpus=[] ready_queues=[]`
- `turn2-artifacts/reproduce-run1/gdb_softlock_state.out:93`: `tid_deferred_membership tid=13 deferred_cpus=[]`

The existing timer safety net eventually recovered that state, but only after visible stalls. The fix now rescues that same predicate immediately in the scheduler's queue-empty selection path instead of selecting idle and waiting for the later safety net.

### H1: an MSI-X queue edge could be lost

After the scheduler rescue was added, a timeout-disabled run still wedged:

- `turn3-artifacts/stall-repro/parallels_70s_timeout_disabled_relabeled.serial.log:66904`
- Sample: `uptime_ms=40513 submits=6157 completes=6159 fails=0 last_completion_ms=21781 fps_last_5s=0 ... gpu_pci_lock=busy`

Because the timeout was effectively disabled, this did not escape through the one-shot timeout error path. The GPU command lock stayed busy because the BWM thread was still waiting for a completion that had already stopped advancing. That points to a lost completion wake/interrupt edge, not a scheduler-only stall.

Breenix's Turn 2 IRQ handler disabled and cleared the SPI at entry and exit. For MSI-X, that is the wrong shape: a queue interrupt that arrives while the handler is masked can be cleared before the normal IRQ acknowledge/EOI path observes it.

### H2: GPU lock ordering was not the primary cause

The `gpu_pci_lock=busy` samples are real, but they are a consequence of BWM waiting for command completion while holding the GPU command lock. The ISR completion path does not take that lock. After preserving MSI-X edges, `gpu_pci_lock=busy` still appears intermittently during in-flight commands, but FPS and `last_completion_ms` continue to advance; it no longer marks a persistent wedge.

## C. Linux bottom-half comparison

Turn 1 captured Linux 6.8 source under `turn1-artifacts/linux-probe/linux-v6.8-virtgpu_vq.c`.

Function-level sequence:

- `virtio_gpu_ctrl_ack()` at lines 56-62 schedules `vgdev->ctrlq.dequeue_work`.
- `virtio_gpu_dequeue_ctrl_func()` at lines 196-237 disables virtqueue callbacks, drains used buffers with `reclaim_vbufs()`, repeats until `virtqueue_enable_cb()` reports no missed callbacks, then wakes `ctrlq.ack_queue`.
- Linux's virtqueue callback does not mask or clear the interrupt-controller line in the virtio-gpu top half.

Breenix still completes inline in the IRQ handler, but the important Turn 3 mismatch was the extra interrupt-controller masking/clearing around that handler. The fix removes that masking/clearing so a new MSI-X queue edge remains pending for normal IRQ acknowledge/EOI handling.

## D. The fix

Source commit:

- `kernel/src/task/scheduler.rs:1068-1136`
  - When all run queues are empty, immediately dispatch a Ready thread that is not current, not idle, and not in any deferred-requeue slot.
  - The diagnostic label changed from `stuck_tid` to `rescue_tid` because the scheduler now fixes the condition in the same selection pass.

- `kernel/src/drivers/virtio/gpu_pci.rs:1716-1731`
  - Removed `gic::disable_spi()`, `gic::clear_spi_pending()`, and `gic::enable_spi()` from the virtio-gpu queue MSI-X handler.
  - The handler now reads the used index and completes the wait token without masking or clearing the SPI.

- `kernel/src/drivers/virtio/gpu_pci.rs:1737-1750`
  - Removed the same SPI masking/clearing from the config interrupt handler.

No polling fallback was added. The production command wait timeout remains a one-shot error path, not a retry loop.

## E. Post-fix Parallels boot evidence

Production-timeout Parallels run:

- Artifact: `turn3-artifacts/stall-repro/parallels_70s_normal_no_mask.serial.log`
- Fatal-marker count: 0 for `PANIC`, `CPU0 REGRESSION`, `stuck_tid=13`, `SCHED_RESCUE`, and `GPU PCI command completion timeout`.
- `turn3-artifacts/stall-repro/parallels_70s_normal_no_mask.serial.log:617`:
  - `uptime_ms=70493 submits=28433 completes=28436 fails=0 last_completion_ms=70492 fps_last_5s=124 ... gpu_pci_lock=ok`
- `turn3-artifacts/stall-repro/parallels_70s_normal_no_mask.serial.log:631`:
  - `uptime_ms=75496 submits=30539 completes=30542 fails=0 last_completion_ms=75495 fps_last_5s=140 ... gpu_pci_lock=ok`
- `turn3-artifacts/stall-repro/parallels_70s_normal_no_mask.serial.log:644`:
  - `uptime_ms=80500 submits=32432 completes=32434 fails=0 last_completion_ms=80482 fps_last_5s=125 ... gpu_pci_lock=busy`
- Lines 645-654 continue heartbeat and BWM FPS through `uptime_ms=85471`, showing the final `busy` sample was an in-flight command, not a persistent lock wedge.

## F. Honesty test result

Timeout-disabled Parallels run with the no-mask fix:

- Artifact: `turn3-artifacts/stall-repro/parallels_70s_timeout_disabled_no_mask.serial.log`
- Fatal-marker count: 0 for `PANIC`, `CPU0 REGRESSION`, `stuck_tid=13`, `SCHED_RESCUE`, and `GPU PCI command completion timeout`.
- The VM ran to `uptime_ms=193525` with timeout disabled.
- `turn3-artifacts/stall-repro/parallels_70s_timeout_disabled_no_mask.serial.log:944`:
  - `uptime_ms=190577 submits=93287 completes=93289 fails=0 last_completion_ms=190576 fps_last_5s=199 ... gpu_pci_lock=ok`

This run contained five `rescue_tid=13` markers. The important distinction is that the scheduler handled them immediately: the line at `:826` is followed by `fps_last_5s=214` and `gpu_pci_lock=ok` at `:833`, with rendering continuing through the end of the capture. There was no long `stuck_tid=13` series, no timer safety-net `SCHED_RESCUE`, and no completion timeout escape.

## G. Status

COMPLETE for Turn 3.

The stall evidence points to H3 plus H1: scheduler orphaned-ready recovery was too late, and GPU MSI-X queue interrupts could lose an edge because Breenix masked/cleared the SPI inside the handler. The narrow fixes remove the long stall without reintroducing polling, and both production-timeout and timeout-disabled Parallels runs meet the Turn 3 gate.

Recommended Turn 4 scope: run the goal-contract's full 5-boot Parallels stress window for at least 220s active rendering per boot, using the no-mask fix and preserving the no-polling invariant.
