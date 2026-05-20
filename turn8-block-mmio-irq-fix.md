# Turn 8: VirtIO Block MMIO IRQ-Driven Completion

## A. Status

INCONCLUSIVE.

The VirtIO MMIO block polling path was removed and replaced with IRQ-driven `Completion`. The aarch64 run confirmed block MMIO IRQ delivery, the post-init sector-0 read test, `/sbin/init` ext2 pre-load, and kernel boot completion. The required native aarch64 wrapper still failed later during userspace startup with repeated `UNHANDLED_EC` / `PC_ALIGN` / inline abort output, so this turn cannot honestly claim full aarch64 boot-test completion.

## B. Scope

Touched files:
- `kernel/src/drivers/virtio/block_mmio.rs`
- `kernel/src/arch_impl/aarch64/exception.rs`
- `kernel/src/task/completion.rs`

Frozen files left untouched:
- `kernel/src/drivers/virtio/block.rs`
- `kernel/src/drivers/virtio/sound.rs`

## C. Driver Changes

`block_mmio.rs` now uses per-device `Completion` state instead of polling `used.idx`.

Implemented:
- Per-device request gate for shared static DMA buffers.
- Per-device completion token, pending token, completed descriptor/status, and atomic `last_used_idx`.
- Split submit/wait/finish read and write flows.
- No DAIF-masked spinlock across I/O wait.
- No `spin_loop` or `while has_used` polling in the driver.
- IRQ availability precondition returning `Block MMIO IRQ completion unavailable before interrupts are enabled`.

## D. IRQ Binding

MMIO block init now stores the device MMIO slot and enables the QEMU virt SPI as `48 + slot`.

Observed in test:

```text
[virtio-blk] Block MMIO IRQ 76 enabled for device 0
```

## E. IRQ Handler

Added `block_mmio::get_irq()` and `block_mmio::handle_interrupt(device_index)`.

The handler:
- Reads and acknowledges VirtIO MMIO interrupt status.
- Drains the used-ring entry for the armed request.
- Reads the device status byte.
- Publishes descriptor/status and completes the pending token.

The IRQ path has no logging, allocation, locks, or unbounded work.

## F. Completion Fix

`Completion::wait_timeout()` previously treated any aarch64 `preempt_count() > 0` as syscall sleep context. During boot, `main_aarch64` deliberately disables preemption before the timer is initialized, then pre-loads `/sbin/init` from ext2. Sleeping the boot thread in that window stranded boot.

The completion sleep path is now available on aarch64 only when:
- preemption is disabled, and
- the timer interrupt is initialized.

Before timer init, completion waits remain IRQ-driven but use the existing yield-based boot path.

## G. Builds

Passed with no warnings:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

## H. Tests

aarch64 required test:

```text
./docker/qemu/run-aarch64-boot-test-native.sh
```

Result: failed after retries, but the target block path completed before the later failure.

Key serial evidence:

```text
[virtio-blk] Block MMIO IRQ 76 enabled for device 0
[virtio-blk] Read test passed!
[boot] Init binary pre-loaded: 296776 bytes
  Breenix ARM64 Boot Complete!
```

Failure after that:

```text
[UNHANDLED_EC] cpu=0 EC=0x0 ELR=...
[PC_ALIGN] ...
[EL1_INLINE_ABORT] ...
```

x86 regression:

```text
./docker/qemu/run-boot-parallel.sh 1
```

Result: pass.

Relevant x86 serial evidence:

```text
VirtIO block: Driver initialized with 3 device(s)
VirtIO block test: Read successful!
ext2 root filesystem mounted
KTHREAD JOIN TEST: Completed
```

The default x86 QEMU command does not attach a VirtIO sound device; the existing sound init path reports `No VirtIO sound devices found`.

## I. Honesty Greps

Recorded in `turn8-artifacts/honesty-greps.txt`.

```text
block_mmio: 0
block:      0
sound:      0
```

The requested block MMIO strings are present:

```text
Block MMIO IRQ
Block MMIO IRQ completion unavailable before interrupts are enabled
```

## J. Gold-Master Check

`kernel/src/arch_impl/aarch64/exception.rs` diff is recorded in `turn8-artifacts/exception-diff.txt`.

Only the bounded VirtIO block MMIO SPI dispatch entry was added near the existing MMIO dispatch entries. The AHCI ISB / priority-drop / nested IRQ window block was unchanged.

