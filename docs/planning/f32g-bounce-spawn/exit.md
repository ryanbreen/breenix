# F32g Exit - Bounce-Spawn Stall

## Status

Stopped after Phase 1 diagnosis. No behavior-changing fix was attempted.

Committed:

- `e5aef478 docs(kernel): F32g bounce-spawn diagnosis`

## What I Built

- `docs/planning/f32g-bounce-spawn/diagnosis.md` - Phase 1 diagnosis with
  serial evidence from uninstrumented Parallels runs and a timing-perturbation
  check using temporary raw UART breadcrumbs.

## Original Ask

Diagnose the post-F32f Parallels stall that appeared to stop after
`[spawn] path='/bin/bounce'`, prove where bounce stops without changing
behavior, then fix the root cause only after the exact syscall/line is known,
and validate with wait_stress plus 5x 120s Parallels boots.

## How This Meets the Ask

- Phase 1 diagnosis: **implemented** in
  `docs/planning/f32g-bounce-spawn/diagnosis.md`.
- Phase 1 evidence from at least two Parallels runs: **implemented** using:
  - `.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-uninstrumented-run1.serial.log`
  - `.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-uninstrumented-run2.serial.log`
- Exact syscall/line: **identified** as init's parent-side ARM64 `Spawn`
  syscall, stuck at `kernel/src/arch_impl/aarch64/syscall_entry.rs:1556`
  while calling `load_elf_from_ext2(&program_path)`.
- Phase 2 root-cause fix: **not implemented**.
- Phase 3 validation sweep: **not run**.
- Phase 4 PR/merge: **not done**.

## Phase 1 Evidence

The original F32f run stopped after:

```text
[init] bsshd started (PID 4)
[spawn] path='/bin/bounce'
```

F32g uninstrumented reproductions showed the same failure class one or more
spawns earlier:

```text
[init] Boot script completed
[spawn] path='/bin/bsshd'
```

and:

```text
[init] Breenix init starting (PID 1)
[spawn] path='/bin/bwm'
```

In all failing cases, the serial lacks the next process-manager entry print and
lacks `[spawn] Created child PID ...`, so the child is never created. Bounce
does not reach `_start`; it has no first userspace syscall in the failing path.

Temporary raw UART breadcrumbs around `/bin/bounce` spawn made both runs pass,
which means serial breadcrumbs perturb this timing window. Those runs were used
only as perturbation evidence, not as proof of a fix.

## Fix Rationale

No fix was made. The next root-cause work belongs under the ext2/AHCI completion
path reached by `load_elf_from_ext2`, specifically:

- `kernel/src/arch_impl/aarch64/syscall_entry.rs:1556`
- `kernel/src/arch_impl/aarch64/syscall_entry.rs:1373`
- `kernel/src/fs/ext2/file.rs:342`
- `kernel/src/drivers/ahci/mod.rs:2574`
- `kernel/src/drivers/ahci/mod.rs:742`
- `kernel/src/task/completion.rs:306`
- `kernel/src/drivers/ahci/mod.rs:2436`

Phase 2 must cite Linux AHCI/block/completion semantics before changing code.

## Sweep Table

| Gate | Result | Evidence |
| --- | --- | --- |
| Standard x86_64 build after temporary diagnostics | PASS | `.factory-runs/f32g-bounce-spawn-20260418-200610/build-m2.log` |
| AArch64 build after temporary diagnostics | PASS | `.factory-runs/f32g-bounce-spawn-20260418-200610/build-m2-aarch64.log` |
| AArch64 build after removing diagnostics | PASS | `.factory-runs/f32g-bounce-spawn-20260418-200610/build-uninstrumented-aarch64.log` |
| Instrumented Parallels run 1 | PASS but perturbed | `.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-run1.serial.log` |
| Instrumented Parallels run 2 | PASS but perturbed | `.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-run2.serial.log` |
| Uninstrumented Parallels run 1 | FAIL | `.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-uninstrumented-run1.serial.log` |
| Uninstrumented Parallels run 2 | FAIL | `.factory-runs/f32g-bounce-spawn-20260418-200610/parallels-uninstrumented-run2.serial.log` |
| wait_stress 60s | NOT RUN | Stopped before Phase 2 fix |
| 5x 120s Parallels sweep | NOT RUN | Stopped before Phase 2 fix |

## PR

No PR opened. This branch contains diagnosis only and must not be merged as a
completed F32g fix.

## Known Risks and Gaps

- The exact parent syscall line is identified, but the AHCI completion state at
  the moment of stall still needs a non-perturbing probe.
- The diagnosis does not prove whether the AHCI waiter is blocked without an ISR,
  whether the ISR fires but does not complete the waiter, or whether the path
  stalls before the completion is armed.
- No Linux-parity fix has been selected.

## Next Steps

1. Add or use non-serial AHCI/completion state capture that can be inspected
   without perturbing the spawn timing.
2. Determine whether `Completion::wait_timeout` is reached for the stuck ELF
   read and whether `AHCI_COMPLETIONS[port][0].complete(...)` fires.
3. Compare the confirmed failure against Linux AHCI/block completion semantics.
4. Only then implement the minimal Linux-parity fix.
