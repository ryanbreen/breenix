# Turn 9: Userspace Startup Regression Bisect

## A. Bisection Methodology + Results

Status: BLOCKED.

I first reproduced the current HEAD behavior with Turn 8 applied:

```text
turn9-artifacts/baseline-with-turn8/
```

Current HEAD still reaches the Turn 8 block-MMIO success points:

```text
[virtio-blk] Block MMIO IRQ 76 enabled for device 0
[virtio-blk] Read test passed!
  Breenix ARM64 Boot Complete!
```

It then fails in userspace startup with repeated `UNHANDLED_EC` / `PC_ALIGN` output.

I then tested isolated worktrees created from `1e40154f` (pre-Turn-8), with per-worktree target directories so Cargo rebuilt each case from the correct source:

| Case | Applied change | Result |
| --- | --- | --- |
| Base | no Turn 8 patch | Reproduces after boot complete |
| A | `completion.rs` only | Reproduces after boot complete |
| B | `exception.rs` dispatch only | Does not compile alone; it references `block_mmio::get_irq()` / `handle_interrupt()` introduced by the block patch |
| C | `block_mmio.rs` only | Fails earlier at block self-test because IRQ dispatch is absent; does not reach the userspace regression |

The important result is the base case: `1e40154f` without any Turn 8 changes already fails the same native aarch64 userspace-startup check in this environment.

## B. Root Cause Analysis

Turn 8 is not the root cause of the userspace `UNHANDLED_EC` / `PC_ALIGN` failure.

Evidence:

```text
turn9-artifacts/base-no-turn8/
```

Base serial evidence:

```text
[virtio-blk] Read test passed!
  Breenix ARM64 Boot Complete!
[UNHANDLED_EC] cpu=0 EC=0x0 ELR=...
```

This means the failing behavior predates Turn 8. The Turn 8 changes still make the block-MMIO IRQ path work, but they are not the source of the post-boot userspace/context-switch exception seen by the native wrapper.

## C. Narrower Fix

No Turn 8 fix was applied.

The requested narrowing found no Turn 8 culprit. Reverting or weakening the Turn 8 block-MMIO conversion would not fix the userspace exception, because the pre-Turn-8 base reproduces it.

## D. Verification Evidence

Artifacts:

- `turn9-artifacts/baseline-with-turn8/`
- `turn9-artifacts/base-no-turn8/`
- `turn9-artifacts/bisect-A/`
- `turn9-artifacts/bisect-B/`
- `turn9-artifacts/bisect-C/`
- `turn9-artifacts/patches/`

Large serial logs for `base-no-turn8` and `bisect-A` are stored as `serial.txt.gz`.

No code was changed in Turn 9, so x86 regression and honesty greps were not rerun as a post-fix gate.

## E. Status

BLOCKED.

The regression targeted by Turn 9 is in code outside Turn 8, or in the current aarch64 native test environment. A separate debugging turn should start from the pre-Turn-8 base reproduction and investigate the userspace/context-switch `UNHANDLED_EC` / `PC_ALIGN` loop directly.

