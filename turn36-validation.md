# Turn 36 validation: COMPLETE

## Status

COMPLETE. This was documentation-only. No source files were changed, and no
build or boot was run.

## 36A answer

Parallels does not reach `gpu_mmio.rs` in the normal aarch64 boot. It has PCI
ECAM, takes the PCI driver branch, and initializes `virtio-gpu-pci`
(`kernel/src/drivers/mod.rs:156-213`). T35 confirms that with
`[virtio-gpu-pci] MSI-X enabled: config_spi=53 queue_spi=54` and
`[drivers] VirtIO GPU (PCI) initialized`
(`turn35-artifacts/boot-1-gpu-path-lines.txt:5-20`).

The target that does exercise `gpu_mmio.rs` is ARM64 QEMU `virt` using
`virtio-gpu-device`. The launchers configure that path in `run.sh:925-934` and
`run.sh:986-1001`, plus the Docker/graphics QEMU scripts cited in
`turn36-artifacts/gpu-mmio-reachability.md`.

## 36B most likely CPU0 root cause

The most likely T35-specific trigger is the new Parallels-visible SPI dispatch
probe in `exception.rs`, not a live MMIO GPU interrupt. T35's MMIO init,
`enable_mmio_irq()`, handler counters, and SPI overlap scenarios are inconsistent
with the PCI-only Parallels evidence. The dispatch probe is the only T35 change
that plausibly ran on Parallels, and it fits the T22/T26/T27 pattern where small
additive IRQ/scheduler-adjacent changes trip the CPU0 guard before the intended
experiment actually runs.

## 36C revised P9 plan

P9 should continue as live MMIO-GPU work, but against ARM64 QEMU `virt`, not
Parallels. Guard rails for the next implementation steps:

- First prove baseline QEMU MMIO GPU reachability from serial logs.
- Do not reintroduce an unconditional Parallels-visible SPI dispatch branch as
  part of a multi-change test.
- Gate any MMIO GPU IRQ work to a real initialized MMIO GPU/QEMU path.
- Check SPI collisions explicitly before enabling MMIO GPU interrupts.
- Keep any IRQ dispatch change separately bisectable from queue/observer work.

## Proposed T37 first step

Run a baseline ARM64 QEMU MMIO reachability turn with no P9 source changes:
capture serial evidence that `init_virtio_mmio()` and the existing MMIO GPU init
path run, then identify the exact GPU polling loop to convert. Only after that
should a minimal observer or polling-removal implementation be attempted.
