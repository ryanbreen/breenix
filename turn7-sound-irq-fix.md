# Turn 7: PCI virtio-sound IRQ completion conversion

## A. Reference pattern recap

Turn 4's PCI virtio-block conversion provided the working pattern: serialize shared DMA buffers with a request gate, arm a monotonic completion token before submission, release the virtqueue mutex before `Completion::wait_timeout()`, let the IRQ handler drain the used ring and complete the waiter, then reacquire the queue only to free the completed descriptor. The request path has an explicit "IRQ completion unavailable" precondition instead of a polling fallback.

## B. Sound.rs differences

`kernel/src/drivers/virtio/sound.rs` has two independent PCI queues:

- control queue 0 for SET_PARAMS/PREPARE/START commands, sharing `cmd` and `resp` DMA buffers
- TX queue 2 for PCM writes, sharing `tx_xfer`, `tx_pcm`, and `tx_status` DMA buffers

The old driver stored the whole device behind a global mutex and polled each queue's used ring. It had no `handle_interrupt()` and did not enable PCI INTx.

## C. Fix shape applied

The driver now stores the initialized device as `Arc<VirtioSoundDevice>` so public audio calls do not hold the global mutex while waiting. Each queue has its own request gate, `Completion`, pending token, completed descriptor slot, and mutex-protected `Virtqueue`.

`send_ctrl_locked()` and `do_write_pcm()` now submit, notify, and wait via `Completion::wait_timeout()` with no queue mutex held. The IRQ handler reads the legacy ISR once, then drains both control and TX queues if they have pending waiters. `pci_dev.enable_intx()` is called during init, and the IRQ10/IRQ11 shared handlers dispatch sound alongside block.

A sound post-init self-test was added, but it only runs when `sound::init()` actually initialized a PCI sound device.

## D. Diff summary

- `kernel/src/drivers/virtio/sound.rs`
  - Removed control/TX polling loops.
  - Added per-queue completion and request-gate state.
  - Added IRQ handler that checks both queues.
  - Added INTx enablement and IRQ availability preconditions.
  - Added optional silence-write self-test for sound-present boots.
- `kernel/src/interrupts.rs`
  - IRQ10 and IRQ11 now call `dispatch_virtio_sound_interrupts()`.
- `kernel/src/drivers/mod.rs`
  - Post-init hook keeps enabling IRQ10/IRQ11 and runs the optional sound self-test only when initialized.

## E. Build evidence

Both required builds completed with no `warning` or `error` lines:

- `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`

Logs:

- `turn7-artifacts/build-x86.log`
- `turn7-artifacts/build-aarch64.log`

## F. Boot + IRQ trace evidence

Default Docker boot:

- `./docker/qemu/run-boot-parallel.sh 1` passed.
- `turn7-artifacts/x86-test/serial_kernel.txt` shows:
  - `VirtIO sound driver initialization failed: No VirtIO sound devices found`
  - `VirtIO block test: Read successful!`
  - `ext2 root filesystem mounted`
  - `No home filesystem: no home block device attached`
  - `KTHREAD JOIN TEST: Completed`

Default QEMU IRQ trace:

- `turn7-artifacts/x86-irq-trace/default-no-sound/qemu-int.log`
- IRQ counts in `turn7-artifacts/irq-counts.txt`:
  - `INT=0x2a`: 4
  - `INT=0x2b`: 1
  - `check_exception`: 0

Explicit `virtio-sound-pci` QEMU attempts:

- `turn7-artifacts/x86-irq-trace/with-virtio-sound/serial_kernel.txt`
- `turn7-artifacts/x86-irq-trace/with-virtio-sound-forced-legacy/serial_kernel.txt`
- Both enumerate `PCI: 00:07.0 [1af4:1059] VirtIO Multimedia/0x01 IRQ=11`.
- Both fail init with `No I/O BAR found`.

This means the Docker QEMU sound device available here is modern PCI virtio-sound (`1af4:1059`) without the legacy I/O BAR required by this driver, even with `disable-modern=on,disable-legacy=off,x-disable-legacy-check=on`.

## G. Regression checks

Block and ext2 regressions passed in the default Docker boot:

- block self-test still passes
- ext2 root still mounts
- `/home` no-device path remains quiet
- no `Failed to read ext2 superblock`
- no `Failed to mount ext2`

`kernel/src/drivers/virtio/block.rs` was not modified in this turn.

## H. Honesty greps

Saved in `turn7-artifacts/honesty-greps.txt`:

```text
sound polling:
0
block polling:
0
block precondition:
469:            return Err("Block IRQ completion unavailable before interrupts are enabled");
560:            return Err("Block IRQ completion unavailable before interrupts are enabled");
sound precondition:
476:            return Err("Sound IRQ completion unavailable before interrupts are enabled");
541:            return Err("Sound IRQ completion unavailable before interrupts are enabled");
```

## I. Status

INCONCLUSIVE.

The code conversion is complete and the default x86 regression path passes, but sound IRQ delivery could not be confirmed because the available QEMU PCI sound device is modern-only from this legacy I/O-port driver's perspective. It enumerates on IRQ11 but fails before queue setup with `No I/O BAR found`, so no sound queue submission can occur and no sound IRQ can fire.
