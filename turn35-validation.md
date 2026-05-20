# Turn 35 validation

Status: INCONCLUSIVE

## Summary

T35 attempted the additive P9 Substep 1 change: MMIO GPU IRQ identity, a hard
IRQ observer, and a dispatch-only aarch64 SPI branch. The existing
`gpu_mmio.rs::send_command()` spin loop was not changed.

The attempted source diff is preserved in:

- `turn35-artifacts/source-diff.txt`
- `turn35-artifacts/source-diff-stat.txt`

After the required single Parallels boot hit a pre-existing-stage CPU0 timer
regression panic, the source changes were reverted per first-failure-abort.
Post-revert source diff is empty:

- `turn35-artifacts/post-revert-source-diff.txt` is 0 bytes
- `turn35-artifacts/post-revert-source-diff-stat.txt` is 0 bytes

No kernel source changes remain in the worktree.

## Attempted source change

Attempted diff stat:

```text
kernel/src/arch_impl/aarch64/exception.rs |   6 ++
kernel/src/drivers/virtio/gpu_mmio.rs     | 109 ++++++++++++++++++++++++++++--
2 files changed, 111 insertions(+), 4 deletions(-)
```

The attempted change added:

- `gpu_mmio.rs`: `MMIO_SLOT`, `MMIO_BASE`, `GPU_MMIO_IRQ_COUNT`,
  `GPU_MMIO_USED_IDX_ADVANCED`, `GPU_MMIO_LAST_USED_IDX`, `get_irq()`,
  `snapshot_counters()`, `handle_interrupt()`, MMIO IRQ enable at init, and a
  non-IRQ-path serial counter readout after MMIO GPU init.
- `exception.rs`: one dispatch-only branch in the SPI block for
  `gpu_mmio::get_irq()` / `gpu_mmio::handle_interrupt()`.

The IRQ handler did not log, allocate, lock, or drain the ring. It only read and
acknowledged the VirtIO MMIO interrupt status, read `CTRL_QUEUE.used.idx`, and
updated atomics.

## Build status

Builds completed before the boot:

- userspace aarch64: PASS (`turn35-artifacts/build-userspace.log`)
- ext2 aarch64 image: PASS (`turn35-artifacts/build-ext2.log`)
- aarch64 kernel: PASS (`turn35-artifacts/build-aarch64.log`)
- x86 qemu-uefi: PASS (`turn35-artifacts/build-x86.log`)
- Parallels EFI: PASS (`turn35-artifacts/build-efi.log`)

Compile warning/error grep files:

- `turn35-artifacts/build-aarch64-warning-error-grep.txt`: 0 bytes
- `turn35-artifacts/build-x86-warning-error-grep.txt`: 0 bytes

## Single boot result

Required single fresh Parallels boot artifacts:

- `turn35-artifacts/boot-1-run.out`
- `turn35-artifacts/boot-1-serial.log`
- `turn35-artifacts/boot-1-screenshot.png`
- `turn35-artifacts/boot-1-fail-marker-scan.txt`
- `turn35-artifacts/boot-1-gpu-path-lines.txt`
- `turn35-artifacts/boot-1-health-lines.txt`
- `turn35-artifacts/boot-1-gpu-irq-evidence.txt`

Boot did not satisfy health criteria. Last heartbeat reached only
`uptime_ms=38225`. The serial log then repeatedly reported:

```text
!!! CPU0 REGRESSION ALARM !!!
CPU0 tick_count = 70, max peer = 30000
KERNEL PANIC!
panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17
```

Because this is a pre-existing-stage boot regression, the directive required
first-failure-abort. External ping was not run.

## Counter evidence

No MMIO GPU counter readout appeared in the Parallels serial log:

- `turn35-artifacts/boot-1-gpu-irq-evidence.txt`: 0 bytes
- `GPU_MMIO_IRQ_COUNT`: not observed
- `GPU_MMIO_USED_IDX_ADVANCED`: not observed
- `GPU_MMIO_LAST_USED_IDX`: not observed

The boot exercised the PCI GPU path instead:

```text
[virtio-gpu-pci] MSI-X active: config_spi=53 queue_spi=54 queue_vector=1
[virtio-gpu-pci] Initialized: 1280x960
[drivers] VirtIO GPU (PCI) initialized
```

This means the required Parallels boot did not exercise `gpu_mmio::init()`.
Combined with the CPU0 panic, T35 cannot prove or disprove the single hypothesis
that the MMIO GPU emits usable queue interrupts.

## Verdict

T36 is not cleared. The additive observer patch built cleanly, but the only
allowed boot failed before the 60s health criterion and produced no MMIO GPU
counter evidence. The source change was reverted and preserved as an artifact
for review. A revised T35 should either run on a QEMU/hybrid boot path that
actually initializes VirtIO GPU MMIO, or adjust the evidence mechanism for a
Parallels PCI-GPU-only boot if Claude wants to keep Parallels as the sole live
target.
