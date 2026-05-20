# Turn 36: revised P9 plan

## Target correction

P9 is not dead code. `gpu_mmio.rs` is live for ARM64 QEMU `virt` targets that
attach VirtIO devices with `-device virtio-gpu-device`. Evidence:

- `run.sh` uses `virtio-gpu-device` on ARM64 and launches `qemu-system-aarch64`
  with `-M virt` (`run.sh:925-934`, `run.sh:986-1001`).
- `docker/qemu/run-aarch64-test.sh` uses `-M virt`, `-kernel`, and
  `-device virtio-gpu-device` (`docker/qemu/run-aarch64-test.sh:34-56`).
- `docker/qemu/run-aarch64-userspace.sh` documents QEMU VirtIO MMIO slots and
  uses `virtio-gpu-device` (`docker/qemu/run-aarch64-userspace.sh:67-91`).
- `scripts/run-arm64-graphics.sh` explicitly selects MMIO devices instead of
  PCI devices (`scripts/run-arm64-graphics.sh:82-100`).

The T35 test environment was wrong for this substep. Parallels uses the PCI GPU
path (`kernel/src/drivers/mod.rs:156-213`) and T35's serial log confirms
`virtio-gpu-pci` with queue SPI 54
(`turn35-artifacts/boot-1-gpu-path-lines.txt:5-20`). A Parallels boot cannot
prove a `gpu_mmio.rs` IRQ observer or polling-removal change unless the change
also touches Parallels-visible dispatch code.

## Guard rails for T37+

1. Do not use Parallels as the primary pass/fail target for MMIO GPU P9 work.
   Use ARM64 QEMU `virt` with `virtio-gpu-device` first.
2. Do not reintroduce an unconditional Parallels-visible SPI dispatch branch in
   `exception.rs` as part of a multi-change test. T35 indicates that even a
   cheap `get_irq()` probe can perturb the Parallels CPU0 guard.
3. If an interrupt dispatch branch is required, gate it to the MMIO/QEMU path
   and test it separately from `gpu_mmio.rs` queue/observer changes.
4. Do not call `Gicv2::enable_irq()` for an MMIO GPU unless
   `gpu_mmio::init_device()` has found a real MMIO GPU and recorded its slot.
5. Check for SPI collisions explicitly before enabling MMIO GPU IRQs. T35's PCI
   GPU queue used SPI 54 (`turn35-artifacts/boot-1-gpu-path-lines.txt:5-13`),
   while the T35 MMIO formula was `48 + slot`
   (`turn35-artifacts/source-diff.txt:37-60`).
6. Keep IRQ handlers hard-path safe: no logging, no allocation, no locks, and
   bounded acknowledgement only. Logging counters should be sampled from
   non-IRQ context.
7. If a Parallels regression appears during a QEMU-only MMIO step, split the
   source change immediately: first prove whether the Parallels-visible
   dispatch branch alone is the trigger, then restore the QEMU-only MMIO work.

## Proposed T37 first step

T37 should be a baseline ARM64 QEMU MMIO reachability turn with no P9 source
changes:

1. Build or use the existing ARM64 QEMU kernel expected by the QEMU runner.
2. Run an ARM64 QEMU `virt` boot with `virtio-gpu-device`.
3. Capture serial evidence that `init_virtio_mmio()` runs and that
   `[drivers] VirtIO GPU driver initialized` or the existing
   `[virtio-gpu] Found GPU device` marker appears.
4. Record whether the current MMIO GPU command path spins during initialization
   or runtime flush, and identify the exact polling loop before changing it.

Only after that baseline should T38 reapply a minimal MMIO-GPU observer or
polling-removal change. That implementation turn should avoid `exception.rs`
unless the QEMU baseline proves an IRQ path is needed, and it should keep any
dispatch change separately bisectable.

## If QEMU cannot exercise it

If the ARM64 QEMU target no longer boots or no longer initializes
`gpu_mmio.rs`, P9 should pause and be reclassified explicitly. The choices would
be:

- repair the ARM64 QEMU MMIO target, then continue P9 as live polling-removal
  work; or
- document `gpu_mmio.rs` as unsupported/dead and delete it in a separate
  dead-code-removal change.

Current source and launcher evidence points to the first path: MMIO GPU is a
live QEMU target, not dead code.
