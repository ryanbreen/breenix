# Turn 4 - PCI VirtIO Block IRQ-Driven Completion

## A. virtio-gpu reference pattern

`gpu_pci.rs` uses a single `Completion` keyed by the expected control-queue used index: the submitter resets the completion, publishes the request, drops queue/device locks, and waits with `GPU_COMPLETION.wait_timeout(expected_token, GPU_COMPLETION_TIMEOUT_NS)`. The MSI-X IRQ handler does not poll in the submitter path; it samples the control queue used index at interrupt time, stores the completed used index, and calls `GPU_COMPLETION.complete(ctrlq_completion_token(used_idx))`. After wake, the submitter validates that the used index advanced and finishes response parsing/freeing outside the IRQ handler.

## B. Fix shape applied to block.rs

`kernel/src/drivers/virtio/block.rs` now has a per-device `Completion`, a monotonic nonzero completion token, pending/completed atomics, and a `BlockRequestGate` that serializes the existing shared DMA buffers without forcing the IRQ handler to take that gate. `read_sector()` and `write_sector()` now set up the descriptor chain under the queue lock, drop that lock before `notify_queue(0)`, wait via `Completion::wait_timeout()`, then reacquire the queue lock only to free the descriptor chain after the IRQ handler has drained the used entry.

`handle_interrupt()` now reads and acknowledges the VirtIO ISR, checks the armed token, briefly `try_lock()`s the virtqueue, drains one used-ring entry with `queue.get_used()`, reads the status byte, stores the completed descriptor/status, and calls `self.completion.complete(token)`. There is no logging, allocation, polling loop, or fallback path in the IRQ handler.

One safety precondition was added after debugging: if x86 has no current scheduler thread and IF is disabled, block I/O returns `Block IRQ completion unavailable before interrupts are enabled` before queueing anything. This avoids submitting a DMA request that cannot be IRQ-completed in the current boot order.

## C. Diff summary

`git diff --stat` for the code file:

```text
kernel/src/drivers/virtio/block.rs | 381 ++++++++++++++++++++++++++++---------
1 file changed, 293 insertions(+), 88 deletions(-)
```

Primary code changes:

- Added `Completion`-based wait state and token tracking.
- Added request gate for the single shared block DMA buffer set.
- Removed the two `while !queue.has_used()` submitter polling loops.
- Moved used-ring draining to `handle_interrupt()`.
- Enabled PCI legacy INTx on device init with `pci_dev.enable_intx()`.
- Added the x86 interruptible-context precondition to avoid unsafe early-boot submission before PIC setup.

## D. Build evidence

Both required builds completed cleanly with no `warning` or `error` lines:

```text
cargo build --release --features testing,external_test_bins --bin qemu-uefi
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
```

Saved logs:

- `turn4-artifacts/x86-test/build-x86.log`
- `turn4-artifacts/x86-test/build-aarch64.log`

## E. Test evidence

Directed x86 command:

```text
./docker/qemu/run-boot-parallel.sh 1
```

Harness result in `turn4-artifacts/x86-test/run.out`:

```text
Results: 1 passed, 0 failed out of 1
```

That harness pass is only the kthread marker gate. The block-specific serial output is not passing:

```text
VirtIO block test: Reading sector 0...
VirtIO block test failed: Block IRQ completion unavailable before interrupts are enabled
Failed to mount ext2 root: "Failed to read ext2 superblock"
```

Saved logs:

- `turn4-artifacts/x86-test/run.out`
- `turn4-artifacts/x86-test/serial_kernel.txt`
- `turn4-artifacts/x86-test/serial_user.txt`

The requested `virtio_blk_*` registry tests were not reached by this harness. The early boot block read and ext2 mount fail first because `drivers::init()` runs before PIC initialization/remapping.

## F. Honesty grep verification

```text
grep -n 'spin_loop' kernel/src/drivers/virtio/block.rs
# no output

grep -n 'while.*has_used' kernel/src/drivers/virtio/block.rs
# no output

rg -n "get_used|wait_timeout|complete\(" kernel/src/drivers/virtio/block.rs
380:        let result = self.completion.wait_timeout(token, timeout_ns);
651:        if let Some((completed_desc, _bytes)) = queue.get_used() {
658:            self.completion.complete(token);
```

Submit paths wait only through `Completion::wait_timeout()`. The only `get_used()` call in `block.rs` is in `handle_interrupt()`.

## G. Debug evidence and status

GDB probe summary: `turn4-artifacts/x86-test/gdb-irq11-summary.md`. Breakpoints on `irq11_handler` and `VirtioBlockDevice::handle_interrupt()` did not fire during the first sector-0 request; after the timeout, execution reached a later ext2 `read_sector()` call.

QEMU interrupt trace: `turn4-artifacts/x86-test/qemu-int.log`. Before removing the temporary early IF enable, QEMU logged:

```text
Servicing hardware INT=0x02
RIP=00000100002b69e3 RFL=00000202
check_exception old: 0xffffffff new 0xb
v=0b e=0012
check_exception old: 0xb new 0xb
v=08 e=0000
```

That is consistent with enabling interrupts before the PIC has been remapped/initialized: a hardware interrupt arrives on a low vector instead of the installed IRQ11 vector. The block driver now avoids that unsafe path and returns an honest precondition error before submitting.

**Status: INCONCLUSIVE.** `block.rs` is converted away from synchronous used-ring polling and builds cleanly, but the x86 boot test cannot prove the IRQ path because the first block I/O happens before `interrupts::init_pic()`. Turn 5 should authorize the minimal boot-order change needed to initialize/remap/unmask PIC IRQ11 before `drivers::init()` performs block I/O, or defer block test/ext2 mount until after PIC setup, then rerun the same block IRQ completion test.
