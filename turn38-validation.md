# Turn 38 validation: INCONCLUSIVE

## Status

INCONCLUSIVE. The dormant `gpu_mmio.rs`-only scaffold built cleanly and
preserved the QEMU MMIO pre-init baseline, but the required single Parallels
boot hit the CPU0 regression alarm. Per first-failure-abort, the source change
was reverted after preserving the attempted diff.

No source changes remain after this turn.

## Source change summary

Attempted source scope was exactly one file:

- `kernel/src/drivers/virtio/gpu_mmio.rs`
- diff stat: 78 insertions, 4 deletions

The attempted diff added:

- `VIRTIO_IRQ_BASE = 48`
- `MMIO_SLOT` / `MMIO_BASE`
- `GPU_MMIO_IRQ_COUNT`
- `GPU_MMIO_USED_IDX_ADVANCED`
- `GPU_MMIO_LAST_USED_IDX`
- `get_irq()`
- `snapshot_counters()`
- `record_mmio_irq_state()`
- unwired `handle_interrupt()`
- slot/base recording after successful GPU init

The attempted source is preserved in:

- `turn38-artifacts/attempted-source-diff-stat.txt`
- `turn38-artifacts/attempted-source-diff.txt`
- `turn38-artifacts/source-diff-stat.txt`
- `turn38-artifacts/source-diff.txt`

After the Parallels failure, `kernel/src/drivers/virtio/gpu_mmio.rs` was
reverted. Both post-revert source diff files are 0 bytes:

- `turn38-artifacts/post-revert-source-diff-stat.txt`
- `turn38-artifacts/post-revert-source-diff.txt`

## Build status

All required builds completed:

- userspace aarch64 build: `turn38-artifacts/build-userspace.log`
- ext2 image build: `turn38-artifacts/build-ext2.log`
- aarch64 kernel build: `turn38-artifacts/build-aarch64.log`
- x86 qemu-uefi build: `turn38-artifacts/build-x86.log`
- Parallels EFI build: `turn38-artifacts/build-efi.log`

Warning/error greps are both 0 bytes:

- `turn38-artifacts/build-aarch64-warning-error-grep.txt`
- `turn38-artifacts/build-x86-warning-error-grep.txt`

## QEMU boot result

QEMU matched the T37 pre-init baseline and passed the T38 QEMU criteria.

Evidence:

- `turn38-artifacts/boot-qemu/boot-1-mmio-bus-walk.txt` shows the hybrid QEMU
  path, five VirtIO MMIO devices, and GPU MMIO enumeration at lines 35-41.
- `turn38-artifacts/boot-qemu/boot-1-mmio-gpu-init.txt` shows the MMIO GPU at
  `0xa003e00`, `Display: 1280x800`, successful init, and `Test passed!`.
- `turn38-artifacts/boot-qemu/boot-1-gpu-timeout-scan.txt` is 0 bytes.
- `turn38-artifacts/boot-qemu/boot-1-pre-init-fail-scan.txt` is 0 bytes.

The expected T37 userspace exception still occurs after init launch, not during
kernel-side MMIO GPU init:

- line 167: `[boot] Launching init from pre-loaded ELF...`
- line 178: first `UNHANDLED_EC`
- line 179: `[FATAL_POSTMORTEM] cpu=0 label=UNHANDLED_EC`

The post-init exception ELR changed from T37's `0xffff0000400fc338` to
`0xffff00004010c8d8`, which is consistent with kernel text/layout shift after
adding code. The failure class and timing stayed the same: CPU0 `UNHANDLED_EC`
after `/sbin/init` launch.

The full QEMU post-init exception loop produced 785k+ matching lines during the
90s capture window. To avoid committing giant repeated logs, the committed scan
files keep the first 200 lines, and `boot-1-full-scan-counts.txt` records the
full scan counts.

## Parallels boot result

Parallels failed the T38 regression check.

The boot still used the PCI GPU path, not MMIO GPU:

- line 83: `[virtio-gpu-pci] Device features...`
- line 95: `[virtio-gpu-pci] MSI-X enabled: config_spi=53 queue_spi=54...`
- line 109: `[drivers] VirtIO GPU (PCI) initialized`

The boot reached userspace startup, then hit the CPU0 regression alarm:

- line 343: `T6[spawn] path='/bin/heartbeat'`
- line 345: `!!! CPU0 REGRESSION ALARM !!!`
- line 346: `CPU0 tick_count = 6, max peer = 30000`
- line 352: `panicked at kernel/src/arch_impl/aarch64/timer_interrupt.rs:598:17:`

See:

- `turn38-artifacts/boot-parallels/boot-1-gpu-path.txt`
- `turn38-artifacts/boot-parallels/boot-1-health-markers.txt`
- `turn38-artifacts/boot-parallels/boot-1-fail-scan.txt`

The run did not reach the 60s heartbeat, bsshd/bounce/compositor liveness
markers, or external ping validation. The CPU0 alarm is the first decisive
failure.

## Hypothesis verdict

The T38 hypothesis was wrong. Even with no `exception.rs` dispatch branch, no
GIC enable, and no `send_command()` change, adding dormant scaffold code to
`gpu_mmio.rs` was enough to trip the Parallels CPU0 guard.

This does not prove `gpu_mmio.rs` executed on Parallels; the boot evidence still
shows the PCI GPU path. The result is stronger and stranger: a Parallels-visible
binary/code-layout change in an aarch64 module that is not reached at runtime
can perturb early post-EL0 scheduling enough to reproduce the T22/T26/T27/T35
CPU0 failure pattern.

## Proposed next step

Do not proceed to T39 dispatch wiring. The next turn should bisect the dormant
scaffold itself, still with no `exception.rs` changes. Suggested split:

1. statics plus `get_irq()` only, no handler body and no init recording;
2. add init-time slot/base recording only;
3. add the handler body only after the first two pass.

Each substep should keep the Parallels boot as the decisive check. If even
statics-only fails, P9 needs a code-layout/timing investigation before any GPU
IRQ implementation can continue.
