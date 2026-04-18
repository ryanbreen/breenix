# F25 Exit - Post-bwm Spawn Hang

Date: 2026-04-18

Branch: `fix/f25-post-bwm-spawn-hang`

Base: `97b441823473b81de7ba3ba89404c217903e0cd5`

## Summary

F25 narrowed the post-bwm spawn hang from a bounce-specific suspicion into an architectural exec-read problem under the active GUI workload, then fixed the storage/read amplification path enough for the boot sequence to bring up bwm, telnetd, bsshd, and the bounce window.

The final ARM64 init sequence starts the long-lived network services before the continuously animating bounce workload. Bounce still spawns after bwm, which validates the original desktop-app goal, while avoiding further service exec reads after the animation begins.

## Phase 1 Verdict

The cheap narrowing test added `/bin/hello_raw` immediately after `/bin/bwm` and before `/sbin/telnetd`.

Result:

```text
[spawn] path='/bin/bwm'
[spawn] Created child PID 2 for parent PID 1
[spawn] Success: child PID 2 scheduled
[spawn] path='/bin/hello_raw'
```

There was no `Created child PID` for `hello_raw`, no `[hello_raw] start`, and no `[syscall] exit(42)`.

Verdict: `hello_raw` also hangs after bwm, so the bug is not bounce-specific. Any post-bwm exec was vulnerable.

Details are recorded in `docs/planning/f25-spawn-hang/phase1.md`.

## Phase 2 Breadcrumbs

Temporary raw serial breadcrumbs were added and then removed from the final patch.

Spawn/exec breadcrumbs showed the stall after syscall entry and path preparation, before ELF read completion:

```text
S ... 3
```

The process manager breadcrumbs never reached page table allocation, mapping, or thread creation, so the stall was not child scheduling.

AHCI breadcrumbs during the same run showed:

```text
W=58 I=57 C=57 D=57
last marker: W
```

That means the next AHCI command was issued and the thread entered the scheduler sleep path, but no completion was delivered before the run ended. The stall localized to exec ELF file reads over ext2/AHCI under the active bwm workload.

## Phase 3 Fix

The fix reduces exec-time AHCI command pressure and removes the worst read amplification:

- Added `BlockDevice::read_blocks()` as a default multi-block API.
- Implemented native AHCI multi-sector reads in `AhciBlockDevice`, capped at the existing 128-sector / 64 KiB command-table limit.
- Taught ext2 file reads to coalesce contiguous physical blocks into runs.
- Cached the single-indirect pointer block during a file read instead of rereading it for every logical block.
- Added a 1 ms sleep in bounce's window-buffer render loop so the animated demo yields when idle between presents.
- Kept ARM64 init serialized: bwm first, then telnetd, then bsshd, then bounce.

The AHCI multi-sector limit follows the driver structure already present in this tree: the command table reserves 8 PRDT entries for 128 sectors / 64 KiB, and the DMA buffers are 64 KiB per slot. The existing `setup_read_sectors` path already accepts counts up to 128 and emits `READ DMA EXT`; F25 uses that support instead of issuing one command per 512-byte sector.

The project standard is to follow Linux/FreeBSD-grade practices (`CLAUDE.md`), and the pre-existing F18 analysis cites the relevant Linux v6.8 AHCI completion model: acknowledge port interrupt status, read `PORT_SCR_ACT` / `PORT_CMD_ISSUE`, and complete commands based on the hardware-active mask. F25 did not rewrite that completion model; it reduces the number of exposed completions needed for large exec reads.

## Phase 4 Validation

Final Parallels serial showed all target services and the app lifecycle:

```text
[spawn] path='/bin/bwm'
[spawn] Created child PID 2 for parent PID 1
[spawn] Success: child PID 2 scheduled
[spawn] path='/sbin/telnetd'
[spawn] Created child PID 3 for parent PID 1
[spawn] Success: child PID 3 scheduled
[spawn] path='/bin/bsshd'
[spawn] Created child PID 4 for parent PID 1
[spawn] Success: child PID 4 scheduled
[spawn] path='/bin/bounce'
[spawn] Created child PID 5 for parent PID 1
[spawn] Success: child PID 5 scheduled
TELNETD_STARTING
TELNETD_LISTENING
[init] Boot script completed
bsshd: listening on 0.0.0.0:2222
[init] bounce started (PID 5)
Bounce spheres demo starting (for Gus!)
[bwm] Discovered window 'Bounce' (id=1, 400x300) at (30,38)
```

No `SOFT LOCKUP` lines appeared in the final serial parse.

Capture:

```text
/tmp/f25-captures/f25-final-rerun.png
```

Strict F23 verdict:

```text
distinct=2187 dominant=(10, 10, 25) dom_frac=0.0894
big_color_buckets=12 blue_baseline=False red_baseline=False
VERDICT=PASS
```

Build validation:

```text
./userspace/programs/build.sh --arch aarch64
./scripts/create_ext2_disk.sh --arch aarch64
cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

All completed without observed compiler warnings.

## PR

PR: https://github.com/ryanbreen/breenix/pull/318

## Self-Audit

- No polling or busy-wait fallback was added.
- No Tier 1 prohibited files were modified.
- No F1-F24 changes were reverted.
- Temporary raw serial breadcrumbs were removed before commit.
- The remaining architectural limitation is explicit: the init script avoids starting more exec-heavy services after the continuously animating bounce workload begins. This keeps boot reliable and leaves any future "arbitrary exec while GUI app is saturating graphics" work as a separate scheduler/storage stress item.
