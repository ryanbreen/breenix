# Factory: F28 - Eliminate the 5ms Wake Fallback

## Goals

- Instrument the client/compositor frame wake path in `kernel/src/syscall/graphics.rs`.
- Reproduce the 120 second Parallels GUI workload and record fallback-to-event wake numbers.
- Fix the race that lets clients wake by the 5 ms fallback instead of by compositor upload completion.
- Validate a sustained 120 second GUI run with fallback wakeups below 0.1% and FPS at or above 160 Hz.

## Non-goals

- Do not add polling or busy-waiting.
- Do not revert F1-F26 or regress PR #320.
- Do not modify Tier 1 prohibited files.

## Hard Constraints

- The event-driven path must be made reliable; the fallback must not be extended into the normal mechanism.
- ARM64/aarch64 builds must stay clean.
- QEMU processes must be cleaned before handoff.

## Deliverables

- Counter instrumentation committed before the race fix.
- Phase 2 reproduction numbers recorded.
- Phase 3 root-cause analysis and fix committed.
- Phase 4 120 second validation with fallback counter below 0.1% and FPS at or above 160 Hz.
- PR opened, merged, and local checkout returned to `main`.

## Runbook

Follow `/Users/wrb/getfastr/code/fastr-ai-skills/general-dev/factory-orchestration/implement.md`.

