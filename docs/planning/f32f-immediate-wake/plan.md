# Plan - F32f Immediate Wake Under Waitqueue Lock

## Milestones

### M1. Linux/Breenix Wake Audit
- Files: `docs/planning/f32f-immediate-wake/audit.md`, `scratchpad.md`, `decisions.md`
- Description: Read Linux `try_to_wake_up`, Linux waitqueue wake code, and Breenix waitqueue/scheduler/interrupt paths. Document where state changes occur, who drains deferred wakes, and whether task-context callers are unnecessarily routed through the ISR ring.
- Validation command: `test -s docs/planning/f32f-immediate-wake/audit.md && rg "Linux citations|Breenix findings|Conclusion" docs/planning/f32f-immediate-wake/audit.md`
- Maps to deliverable: Phase 1 audit.

### M2. Immediate Task-Context Wake
- Files: `kernel/src/task/waitqueue.rs`, `kernel/src/task/scheduler.rs`, possible narrowly scoped scheduler helpers
- Description: Add a task-context wake path that removes waiters under the waitqueue lock and transitions/enqueues them immediately, keeping genuine IRQ wakeups on the existing deferred path.
- Validation command: `cargo build --release --features testing,external_test_bins --bin qemu-uefi`
- Maps to deliverable: Phase 2 fix.

### M3. Required Validation
- Files: `docs/planning/f32f-immediate-wake/exit.md`, `scratchpad.md`
- Description: Run wait-stress and Parallels boot gates. If pass criteria are not met, document the evidence and stop.
- Validation command: `cargo run -p xtask -- boot-stages`
- Maps to deliverable: Phase 3 validation and Phase 4 decision.

### M4. Merge Or Stop
- Files: issue tracker, git branch, PR
- Description: If all gates pass, push, open PR, merge, return to `main`, and close Beads. If not, commit exit documentation and stop.
- Validation command: `git status --short --branch`
- Maps to deliverable: Phase 4 merge or honest stop.
