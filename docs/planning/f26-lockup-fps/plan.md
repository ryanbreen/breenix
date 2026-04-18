# Plan - F26 Post-Boot Lockup + Bounce FPS Regression

## Milestones

### M1. Reproduce and Bound

- Files: `docs/planning/f26-lockup-fps/phase1.md`, `docs/planning/f26-lockup-fps/scratchpad.md`
- Description: Run the 120 second Parallels GUI test, capture screenshots at 60/90/110 seconds, inspect serial cadence, classify kernel-vs-userspace lockup, compute FPS, and commit Phase 1 findings.
- Validation command: `test -s docs/planning/f26-lockup-fps/phase1.md && test -s logs/f26/reproduce/serial.log`
- Maps to deliverable: Phase 1 reproduction committed.

### M2. Diagnose Lockup

- Files: diagnosis notes under `docs/planning/f26-lockup-fps/`, source files only if non-hot-path instrumentation is required.
- Description: Use serial cadence and, if needed, nonintrusive GDB or safe subsystem-level instrumentation to identify the first-stuck process/subsystem and root-cause class.
- Validation command: `grep -E "Verdict|Root cause|Evidence" docs/planning/f26-lockup-fps/phase2.md`
- Maps to deliverable: Phase 2 diagnosis committed.

### M3. Fix Lockup

- Files: source files identified by M2.
- Description: Apply the smallest production fix for the diagnosed lockup without polling or Tier 1 edits.
- Validation command: `./run.sh --parallels --test 120`
- Maps to deliverable: lockup fix applied and revalidated.

### M4. Restore Bounce FPS

- Files: `userspace/programs/src/bounce.rs` and docs.
- Description: Remove or replace the F25 bounce 1 ms sleep band-aid, then verify compositor frame rate is at least 100 Hz without reintroducing lockup.
- Validation command: `./run.sh --parallels --test 120`
- Maps to deliverable: FPS verified at least 100 Hz.

### M5. Final Validation and Merge

- Files: `docs/planning/f26-lockup-fps/exit.md`, Beads metadata.
- Description: Run clean builds/quality gates, final 120 second Parallels validation, open PR, merge, return to main.
- Validation command: `cargo build --release --features testing,external_test_bins --bin qemu-uefi 2>&1 | grep -E "^(warning|error)" && exit 1 || exit 0`
- Maps to deliverable: PR opened, merged, back on main.
