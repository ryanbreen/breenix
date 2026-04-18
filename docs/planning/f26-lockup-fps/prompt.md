# Factory: F26 - Post-Boot Lockup + Bounce FPS Regression

## Goals

- Reproduce and bound the Parallels GUI lockup after the full bwm/telnetd/bsshd/bounce stack renders.
- Diagnose whether the lockup is kernel-level or userspace-level.
- Apply the minimal root-cause fix for the lockup.
- Remove or revisit the F25 1 ms bounce render-loop sleep and verify bounce/compositor FPS improves to at least 100 Hz.
- Validate a 120 second Parallels test with strict F23 render verdict and no soft lockups.

## Non-goals

- Do not revert F1-F25 or PRs through #319.
- Do not rewrite the GUI stack, compositor architecture, AHCI driver, or scheduler unless diagnosis proves a narrow change is required.
- Do not add polling as a fix.

## Hard Constraints

- No Tier 1 prohibited file edits without explicit user approval.
- No logging or instrumentation in interrupt/syscall hot paths.
- Build clean on aarch64 with zero warnings.
- Keep captured logs and screenshots out of committed source unless explicitly needed as documentation.
- Clean up QEMU processes before QEMU-based tests and before handing control back.

## Deliverables

- `docs/planning/f26-lockup-fps/phase1.md`: reproduction verdict, last frame timing, kernel-vs-userspace verdict, and FPS computation.
- Phase 2 diagnosis committed after identifying the first-stuck component and lockup class.
- Minimal lockup fix committed.
- Bounce FPS fix/verification committed.
- `docs/planning/f26-lockup-fps/exit.md`: final summary with Phase 1/2 verdicts, root cause, FPS before/after, validation, and PR URL.

## Done-when

- Phase 1 reproduction is committed.
- Phase 2 diagnosis is committed.
- Lockup fix is applied.
- FPS is verified at least 100 Hz, ideally near the prior 200 Hz.
- A 120 second Parallels validation has no `SOFT_LOCKUP` lines and passes strict F23 render verdict.
- PR is opened, merged, and local branch returns to `main`.
- Self-audit confirms no polling, no Tier 1 changes, and F1-F25 intact.

## Runbook

Follow `/Users/wrb/getfastr/code/fastr-ai-skills/general-dev/factory-orchestration/implement.md`.

## Reference Artifacts

- `docs/planning/f25-spawn-hang/exit.md`
- `userspace/programs/src/bounce.rs`
- `userspace/programs/src/bwm.rs`
- `kernel/src/syscall/graphics.rs`
- `scripts/parallels/capture-display.sh`
- `scripts/f23-render-verdict.sh`
