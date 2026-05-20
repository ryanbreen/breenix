## Summary

Converts all four polling virtio drivers from synchronous used-ring polling to IRQ-driven `Completion` waits, matching the pattern established by PR #343 (virtio-gpu).

## Drivers converted

| Driver | Source-level polling | Runtime verification |
|---|---|---|
| `virtio/block.rs` (PCI x86) | DELETED | ✓ End-to-end: IRQ fires, ext2 root mount works, virtio-blk test passes |
| `virtio/sound.rs` (PCI x86) | DELETED | Structural only — modern QEMU virtio-sound exposes Memory BARs, this is a legacy I/O-BAR driver |
| `virtio/block_mmio.rs` (MMIO aarch64) | DELETED | ✓ Serial confirms `Block MMIO IRQ 76 enabled`, `Read test passed!`, `Breenix ARM64 Boot Complete!` |
| `virtio/sound_mmio.rs` (MMIO aarch64) | DELETED | Structural only — QEMU virt doesn't expose virtio-sound MMIO device |

**Source-level polling is zero across all four drivers** (verified by `grep 'spin_loop\|while.*has_used' kernel/src/drivers/virtio/{block,sound,block_mmio,sound_mmio}.rs`).

## Pattern

Each driver follows the gpu_pci.rs template established in PR #343:

1. Per-device `Completion` (or per-queue for two-queue drivers like sound)
2. Submit / wait / finish split — no DAIF-masked spinlock held across `Completion::wait_timeout()`
3. IRQ handler reads used ring, completes the waiter via `isr_unblock_for_io(tid)`
4. **Honest precondition** that errors out if interrupts aren't enabled — NO silent polling fallback. Example: `Block IRQ completion unavailable before interrupts are enabled`.

## Boot-order fixes

`drivers::init()` ran some block self-tests BEFORE `interrupts::init_pic()` was called on x86. With the new IRQ-driven path, that's structurally impossible — the precondition correctly errors out. Two fixes preserved the production path:

- Turn 5: `drivers::run_post_init_self_tests()` hook runs after PIC init
- Turn 6: ext2 root mount call also moved to post-PIC-init window
- Turn 6 discovered: 3 x86 virtio-blk devices route across BOTH IRQ10 and IRQ11; `interrupts.rs` shared dispatcher iterates devices

## Gold-master safety

`exception.rs` got two surgical additions (`+8` for block_mmio in Turn 8, `+6` for sound_mmio in Turn 10), both placed in `dispatch_irq_action` between existing net_mmio and XHCI dispatch. Gold-master ISB-before-dispatch ERET block at L1338 is **never** touched — verified by `turn8-artifacts/exception-diff.txt` and `turn10-artifacts/exception-diff.txt`.

## Out-of-scope follow-up (documented honestly)

A pre-existing aarch64 userspace boot issue exists on `main` before this PR's first commit. It surfaces as `UNHANDLED_EC` + `PC_ALIGN ELR=0x1` repeating after `Breenix ARM64 Boot Complete!`, before userspace starts. Turn 9 proved via bisection that this regression existed in the `1e40154f` baseline (before any of this PR's commits). The new IRQ-driven block_mmio path proves correctness up to that pre-existing wall.

## Latent bug fix included

`sound_mmio.rs::write_pcm()` previously returned `Ok(len)` without inspecting `TX_STATUS.status`. Turn 10 fixes this — TX errors now surface to the caller.

## Test plan

- [x] x86 5-boot stress gate passed
- [x] virtio-blk test passes via IRQ path
- [x] ext2 root mount works via IRQ path
- [x] Both arch builds clean (zero warnings/errors)
- [x] All honesty greps pass (0 polling in any converted driver, preconditions present)
- [x] Gold-master ISB block verified unchanged across both exception.rs edits
- [ ] Operator review

## Turn-by-turn evidence

The full Ralph 1 transcript is in `~/Downloads/Ralph/breenix-virtio-block-sound-irq-completion-1779234347/inbox.md` (not pushed). Each turn has its own `turn{N}-*.md` writeup in the branch.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
