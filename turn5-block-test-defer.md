# Turn 5 - Defer VirtIO Block Self-Test Until IRQs Work

## A. Boot-order analysis

The x86 boot path initialized and tested VirtIO block too early:

```text
kernel/src/main.rs:265:    let pci_device_count = drivers::init();
kernel/src/main.rs:387:    interrupts::init_pic();
```

Inside `drivers::init()`, the old `virtio::block::test_read()` ran before PIC remap/init. Turn 4's IRQ-only `block.rs` correctly refused that path with `Block IRQ completion unavailable before interrupts are enabled`.

There was a second ordering bug: `drivers::init()` also called `enable_virtio_irq()` before `init_pic()`, so PIC init later re-masked IRQ11. The post-init hook now enables IRQ11 after PIC setup.

## B. Fix shape

I used Option A: added `drivers::run_post_init_self_tests()`.

For x86, `drivers::init()` now only initializes the PCI drivers. The new post-init hook enables the VirtIO legacy INTx line after PIC setup and runs the existing `virtio::block::test_read()` unchanged. `kernel/src/main.rs` calls that hook after PIC/timer/scheduler setup in a controlled window where IF is temporarily enabled, then disables interrupts again before the rest of boot initialization continues.

For aarch64, the MMIO block self-test was moved into the same `run_post_init_self_tests()` API and called from `main_aarch64.rs` after driver init. That keeps the test moved rather than deleted while preserving the existing aarch64 boot shape.

`block.rs` was not edited.

## C. Diff summary

```text
kernel/src/drivers/mod.rs  | 48 ++++++++++++++++++++++++++++++----------------
kernel/src/main.rs         |  8 ++++++++
kernel/src/main_aarch64.rs |  6 +++++-
3 files changed, 45 insertions(+), 17 deletions(-)
```

## D. Build evidence

Both required builds completed cleanly, with no `warning` or `error` lines:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

Saved logs:

- `turn5-artifacts/x86-test/build-x86.log`
- `turn5-artifacts/x86-test/build-aarch64.log`

## E. IRQ-actually-fires evidence

The Docker QEMU trace was captured with the same boot shape as `run-boot-parallel.sh`, plus `-d int,guest_errors -D /output/qemu-int.log`.

Evidence:

```text
INT=0x2b count: 1
triple fault count: 0
```

Trace excerpt:

```text
Servicing hardware INT=0x2b
99: v=2b e=0000 i=0 cpl=0 IP=0008:000001000023dfe2
```

Saved trace:

- `turn5-artifacts/x86-irq-trace/qemu-int.log`
- `turn5-artifacts/x86-irq-trace/serial_kernel.txt`
- `turn5-artifacts/x86-irq-trace/serial_user.txt`

Given Turn 4's honesty grep, `handle_interrupt()` is the only place in `block.rs` that drains `get_used()` and completes the token. The successful read plus IRQ11 vector delivery confirms the new IRQ completion path was exercised.

## F. Block test result

The directed x86 harness still reports the kthread gate as passing:

```text
Results: 1 passed, 0 failed out of 1
```

The block-specific lines now show the moved self-test completing:

```text
Running driver post-init self-tests...
VirtIO block test: Reading sector 0...
VirtIO block test: Read successful!
First 16 bytes: [00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00, 00]
```

Saved harness artifacts:

- `turn5-artifacts/x86-test/run.out`
- `turn5-artifacts/x86-test/serial_kernel.txt`
- `turn5-artifacts/x86-test/serial_user.txt`

Residual note: ext2 root still attempts to mount earlier in `kernel_main` and fails before the post-init IRQ window. That is outside this turn's self-test move, but should be handled in a follow-up if the objective is to make all early block-backed filesystems use the IRQ-only path.

## G. Honesty greps

```text
grep -nc 'spin_loop\|while.*has_used' kernel/src/drivers/virtio/block.rs
0

grep -n 'Block IRQ completion unavailable' kernel/src/drivers/virtio/block.rs
469:            return Err("Block IRQ completion unavailable before interrupts are enabled");
560:            return Err("Block IRQ completion unavailable before interrupts are enabled");
```

## H. Status

**Status: COMPLETE for Turn 5.** The block self-test was moved rather than removed, x86 IRQ11 is enabled after PIC setup, both architecture builds are clean, the x86 block read now succeeds, and QEMU interrupt tracing confirms IRQ11 delivery without the prior triple-fault signature.
