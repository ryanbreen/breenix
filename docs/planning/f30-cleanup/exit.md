# F30 Exit — AHCI Diagnostic Scaffolding Cleanup

PR: https://github.com/ryanbreen/breenix/pull/323

## Summary

Removed F8-F17/F19 diagnostic-only scaffolding that was no longer needed after the IRQ admission, AHCI completion, and post-bwm spawn fixes.

Live source sweep after cleanup:

```text
rg -n "AHCI_TRACE|STUCK_SPI34|STUCK_SPI|GIC_CPU_AUDIT|GICR_MAP|GICR_STATE|UNBLOCK_SCAN|LAST_USER_CTX_WRITE|CI_LOOP|TTWU_LOCAL|RESCHED_CHECK|WAKEBUF_|SGI_ENTRY|SGI_BEFORE_MSR|AHCI_RING|push_ahci_event|hello_raw|hello_println|hello_nostd_padded" kernel userspace libs tests src.legacy xtask Cargo.toml scripts --glob '!target/**'
```

Result: no matches in live source paths.

## Per-Family Results

1. Survey
   - Commit: `ebb4db1e docs(f30): survey diagnostic scaffolding`
   - Output: `docs/planning/f30-cleanup/survey.md`

2. Scheduler/AHCI wake breadcrumbs
   - Commit: `f573ba8a cleanup(f30): remove scheduler AHCI breadcrumbs`
   - Removed `UNBLOCK_*`, `TTWU_LOCAL_*`, `RESCHED_CHECK_*`, `WAKEBUF_*`, and related AHCI event pushes from scheduler/completion/IRQ-tail paths.
   - Validation: clean build and 120s Parallels run, strict verdict PASS, init complete, bounce rendered, no SOFT_LOCKUP.

3. SGI breadcrumbs
   - Commit: `0b266d9c cleanup(f30): remove SGI trace breadcrumbs`
   - Removed SGI boundary trace calls/helper.
   - Validation: clean aarch64 kernel build, clean qemu-uefi build, isolated 120s Parallels run `f30cleanup-sgi-1776511094`, strict verdict PASS, init complete, bounce rendered, no removed markers.

4. GIC diagnostic dumps
   - Commit: `ea22ada0 cleanup(f30): remove GIC diagnostic dumps`
   - Removed `[STUCK_SPI*]`, `[GIC_CPU_AUDIT]`, `[GICR_MAP]`, and `[GICR_STATE]` output/dump scaffolding.
   - Retained post-enable ICC readbacks in `init_gicv3_cpu_interface`; a validation run without those readbacks stalled before init completion, so that ordering is load-bearing rather than removable diagnostic output.
   - Validation: clean aarch64 kernel build, clean qemu-uefi build, isolated 120s Parallels run `f30cleanup-gic-rd-1776512219`, strict verdict PASS, init complete, bounce rendered, no SOFT_LOCKUP.

5. AHCI trace ring
   - Commit: `b2ba104d cleanup(f30): remove AHCI trace ring`
   - Removed `AHCI_TRACE_*` constants, per-CPU ring structures, `push_ahci_event`, `dump_recent_ahci_events`, SGI target collection, and AHCI ISR trace push sites.
   - Validation: clean aarch64 kernel build, clean qemu-uefi build, isolated 120s Parallels run `f30cleanup-ahci-ring-1776512676`, strict verdict PASS, init complete, bounce rendered, frame #19000 by tick 115000, no removed markers.

6. `hello_raw` probe binary
   - Commit: `674c85e2 cleanup(f30): remove hello_raw probe binary`
   - Removed `userspace/programs/src/hello_raw.rs`, Cargo bin registration, and userspace install-list entry.
   - Validation: `userspace/programs/build.sh --arch aarch64` clean; regenerated ext2 disk had no `hello_raw` entry; clean aarch64 kernel build; clean qemu-uefi build; isolated 120s Parallels run `f30cleanup-hello-1776513117`, strict verdict PASS, init complete, bounce rendered, frame #22000 by tick 140000, no removed markers.

## Final Sweep

Final isolated 120s Parallels sweep:

- VM: `f30cleanup-final-1776513691`
- Strict verdict: PASS
- Serial markers:
  - `[init] Boot script completed`
  - `[init] bounce started`
  - no `SOFT_LOCKUP`, panic, or removed diagnostic markers
- Frame/tick result: last frame `32500`, last tick `195000`, derived FPS `166.67`
- Final serial line count: `504`

The serial line count is not directly comparable to early failed/shorter runs because the final sweep ran long enough to emit many frame/timer progress lines. The diagnostic marker families themselves were reduced to zero live runtime lines in the final serial output.

## Notes

`./run.sh --parallels` deletes `breenix-*` VMs globally. During this work, sibling factory VMs were active, so validation used isolated `f30cleanup-*` Parallels VMs with the same EFI/ext2 disks and 120s runtime window to avoid disrupting unrelated factory work.
