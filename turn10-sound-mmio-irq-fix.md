# Turn 10: virtio-sound MMIO IRQ-driven completion

## A. Template references

- `kernel/src/drivers/virtio/block_mmio.rs` was used for the aarch64 MMIO IRQ pattern:
  device slot IRQ mapping as `48 + slot`, `get_irq()`, `handle_interrupt()`, MMIO interrupt-status acknowledge, and dispatch from `kernel/src/arch_impl/aarch64/exception.rs`.
- `kernel/src/drivers/virtio/sound.rs` was used for the two-queue sound pattern:
  separate completion state for control queue 0 and TX queue 2, and draining both queues from the interrupt handler.

## B. sound_mmio differences

- `sound_mmio.rs` now has independent request gates and `Completion` state for the control queue and TX queue.
- Control commands submit queue-0 descriptors, notify the device, and block on IRQ completion with a hard timeout/error path. No polling fallback remains.
- TX writes submit queue-2 descriptors, notify the device, and block on IRQ completion with a hard timeout/error path. No polling fallback remains.
- The latent TX status bug is fixed: `write_pcm()` now reads `TX_STATUS.status` after completion and returns an error unless it is `resp::OK`.
- MMIO IRQ wiring uses `VIRTIO_IRQ_BASE + slot`, enables that GIC IRQ after device init, exposes `get_irq()`, and acknowledges the device interrupt status in the hard IRQ handler.

## C. Gold-master diff

`kernel/src/arch_impl/aarch64/exception.rs` was changed only to dispatch the sound MMIO IRQ next to the existing block MMIO dispatch:

```rust
if let Some(sound_irq) = crate::drivers::virtio::sound_mmio::get_irq() {
    if irq_id == sound_irq {
        crate::drivers::virtio::sound_mmio::handle_interrupt();
    }
}
```

The gold-master ISB block was not touched. The saved diff is in `turn10-artifacts/exception-diff.txt`.

## D. Diff summary

- `kernel/src/drivers/virtio/sound_mmio.rs`
  - Removed polling loops and last-used fields from request state.
  - Added per-queue IRQ completion objects.
  - Added per-queue request gates.
  - Added MMIO interrupt handler and IRQ accessor.
  - Added TX status validation.
- `kernel/src/arch_impl/aarch64/exception.rs`
  - Added sound MMIO SPI dispatch only.

## E. Build evidence

Commands run:

```bash
cargo build --release --features testing,external_test_bins --bin qemu-uefi
cargo build --release --target aarch64-breenix.json \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem \
  -p kernel --bin kernel-aarch64
```

Results:

- x86 release build: PASS, no warnings/errors in `turn10-artifacts/build-x86.log`.
- aarch64 release build: PASS, no warnings/errors in `turn10-artifacts/build-aarch64.log`.
- `git diff --check`: PASS, saved in `turn10-artifacts/git-diff-check.txt`.
- `rustfmt --check kernel/src/drivers/virtio/sound_mmio.rs`: PASS.

Repo-wide `cargo fmt --check` was not used as a gate because unrelated pre-existing files fail formatting/trailing-whitespace checks.

## F. Boot evidence

Commands run:

```bash
./docker/qemu/run-boot-parallel.sh 1
./docker/qemu/run-aarch64-boot-test-native.sh
```

Results:

- x86 parallel boot: PASS, `1 passed, 0 failed out of 1`.
- x86 serial shows kthread lifecycle and join tests completed.
- x86 QEMU config still has no PCI sound device: `VirtIO sound driver initialization failed: No VirtIO sound devices found`.
- aarch64 native boot: FAILED after retries at the pre-existing userspace/exception issue identified in Turn 9.
- aarch64 serial reaches MMIO driver initialization and sound probe cleanly:
  - `Found 5 VirtIO MMIO devices`
  - `[virtio-sound] Searching for sound device...`
  - `VirtIO sound driver init failed: No VirtIO Sound device found`

The default aarch64 virt machine did not expose a VirtIO sound MMIO device, so the new sound MMIO IRQ handler was compiled and wired but not runtime-triggered.

## G. Honesty greps

Saved in `turn10-artifacts/honesty-greps.txt`.

Polling greps:

```text
kernel/src/drivers/virtio/block.rs:0
kernel/src/drivers/virtio/sound.rs:0
kernel/src/drivers/virtio/block_mmio.rs:0
kernel/src/drivers/virtio/sound_mmio.rs:0
```

Completion preconditions:

```text
kernel/src/drivers/virtio/block.rs:2
kernel/src/drivers/virtio/block_mmio.rs:2
kernel/src/drivers/virtio/gpu_pci.rs:1
kernel/src/drivers/virtio/sound.rs:2
kernel/src/drivers/virtio/sound_mmio.rs:2
```

## H. Status: INCONCLUSIVE

Structural conversion is complete:

- All four target drivers have zero `spin_loop` / `while.*has_used` polling matches.
- `sound_mmio.rs` has IRQ-driven completion for both control and TX queues.
- `sound_mmio.rs` now validates `TX_STATUS.status`.
- x86 regression boot passed.
- aarch64 build passed and serial reached the sound MMIO probe.
- Gold-master ISB block remained untouched.

Runtime IRQ verification for `sound_mmio.rs` is inconclusive because the tested aarch64 QEMU virt configuration exposes no VirtIO sound MMIO device, and the later aarch64 userspace failure remains the pre-existing issue proven in Turn 9.
