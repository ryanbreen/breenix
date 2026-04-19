# Factory: F32f Immediate Wake Under Waitqueue Lock

## Goals
- Audit Linux waitqueue wake semantics against Breenix's current waitqueue wake path.
- Implement immediate task-context waitqueue wake while preserving the deferred ISR wake path.
- Validate that waitqueue stress remains race-free and that Parallels boot no longer stalls.

## Hard Constraints
- Preserve F32e's lock scope: hold the waitqueue lock only across enqueue/state publication, never across schedule.
- Do not modify Tier 1 prohibited files.
- Do not add timer-driven wake fallbacks, arbitrary timeouts, CPU routing workarounds, or Parallels-specific workarounds.
- Cite Linux file:line evidence for semantic choices.
- If validation does not reach the requested pass criteria, stop and write honest exit documentation.

## Deliverables
- `docs/planning/f32f-immediate-wake/audit.md` with Linux and Breenix wake-path findings.
- A task-context immediate waitqueue wake implementation, if the audit confirms the deferred gap.
- Validation evidence for `wait_stress` and Parallels boot gates.
- `docs/planning/f32f-immediate-wake/exit.md` with final status, evidence, and PR URL if merged.

## Done When
- Audit committed with Linux citations.
- Fix committed with Linux citations.
- `wait_stress` 60 seconds passes with zero stalls.
- 5 out of 5 Parallels 120-second boots pass the requested lifecycle, CPU tick, FPS, render, and AHCI checks.
- PR is opened, merged, and the checkout is back on `main`; otherwise exit documentation explains the stopping point.

## Runbook
Follow `/Users/wrb/getfastr/code/fastr-ai-skills/general-dev/factory-orchestration/implement.md`.
