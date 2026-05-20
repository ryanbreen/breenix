# Turn 6 - Defer ext2 Root Mount Until IRQ-Driven Block Works

## A. Original ext2 mount call site

The x86 root and home ext2 mounts were in `kernel_main` before PIC setup:

```text
kernel/src/main.rs:265:    let pci_device_count = drivers::init();
pre-turn kernel/src/main.rs:279:    match kernel::fs::ext2::init_root_fs() {
pre-turn kernel/src/main.rs:297:    match kernel::fs::ext2::init_home_fs() {
kernel/src/main.rs:387:    interrupts::init_pic();
```

Before this turn, `init_root_fs()` and `init_home_fs()` ran immediately after `drivers::init()` and before `interrupts::init_pic()`. With the Turn 4 IRQ-only block driver, that produced `Failed to read ext2 superblock`.

## B. New location

I used the existing Turn 5 temporary interrupt-enabled post-init window in `main.rs`.

New sequence:

1. Initialize PIC, timer, syscall infrastructure, scheduler, workqueue, and softirq.
2. Temporarily enable IF.
3. Run `drivers::run_post_init_self_tests()`.
4. Mount ext2 root with `kernel::fs::ext2::init_root_fs()`.
5. Probe `/home` only if x86 home block device index 3 exists.
6. Disable interrupts again for the remaining boot initialization.

One required dispatch fix was also needed: QEMU routes the boot virtio-blk disk on IRQ11, but the test/ext2 disks on IRQ10. `interrupts.rs` now uses a small shared helper from IRQ10 and IRQ11 to call each initialized virtio-blk device's `handle_interrupt()` once. There is no logging, allocation, or unbounded loop in the IRQ handlers.

`block.rs` was not edited.

## C. Diff summary

```text
kernel/src/drivers/mod.rs |  4 +++-
kernel/src/interrupts.rs  | 20 ++++++++++++++----
kernel/src/main.rs        | 54 +++++++++++++++++++++++++++--------------------
3 files changed, 50 insertions(+), 28 deletions(-)
```

## D. Build evidence

Both required builds completed cleanly, with no `warning` or `error` lines:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

Saved logs:

- `turn6-artifacts/x86-test/build-x86.log`
- `turn6-artifacts/x86-test/build-aarch64.log`

## E. Boot evidence

The x86 harness passed:

```text
Results: 1 passed, 0 failed out of 1
```

Serial evidence:

```text
Running driver post-init self-tests...
VirtIO block test: Reading sector 0...
VirtIO block test: Read successful!
ext2 root filesystem mounted
No home filesystem: no home block device attached
```

Failure check:

```text
Failed to read ext2 superblock / Failed to mount ext2 count: 0
```

Saved harness artifacts:

- `turn6-artifacts/x86-test/run.out`
- `turn6-artifacts/x86-test/serial_kernel.txt`
- `turn6-artifacts/x86-test/serial_user.txt`

## F. IRQ trace evidence

QEMU trace was captured with Docker QEMU plus `-d int,guest_errors -D /output/qemu-int.log`.

Counts:

```text
INT=0x2a count: 4
INT=0x2b count: 1
triple fault count: 0
```

Turn 5 had one IRQ11 delivery for the boot-disk self-test. Turn 6 adds IRQ10 deliveries because the ext2 disk is virtio-blk device index 2, and QEMU routes that device on IRQ10. The combined block IRQ count is now 5, and root ext2 mount succeeds.

Saved trace artifacts:

- `turn6-artifacts/x86-irq-trace/qemu-int.log`
- `turn6-artifacts/x86-irq-trace/serial_kernel.txt`
- `turn6-artifacts/x86-irq-trace/serial_user.txt`

## G. Honesty greps

```text
grep -nc 'spin_loop\|while.*has_used' kernel/src/drivers/virtio/block.rs
0

grep -n 'Block IRQ completion unavailable' kernel/src/drivers/virtio/block.rs
469:            return Err("Block IRQ completion unavailable before interrupts are enabled");
560:            return Err("Block IRQ completion unavailable before interrupts are enabled");
```

## H. Status

**Status: COMPLETE.** The x86 ext2 root mount now runs in an interrupt-capable window, root mount succeeds, QEMU traces multiple block IRQ completions across IRQ10/IRQ11, the precondition remains intact, and `block.rs` remains unchanged.
