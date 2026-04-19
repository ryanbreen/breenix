# Plan - F32j Idle Sleep Gate + GIC SGI Admission

## Milestones

### M1. Baseline and Evidence Review

- Files: F32i docs, Linux v6.8 idle/GIC sources, current AArch64 idle/GIC/scheduler code.
- Description: Confirm the exact current sleep path and SGI configuration gap before editing.
- Validation command: `rg -n "idle_loop_arm64|GICR_ISENABLER0|SGI_RESCHEDULE|check_and_clear_need_resched" kernel/src/arch_impl/aarch64 kernel/src/task/scheduler.rs`
- Maps to deliverable: root-cause basis for Options 1 and 3.

### M2. Idle Sleep Gate

- Files: `kernel/src/arch_impl/aarch64/context_switch.rs`.
- Description: Gate WFI on `need_resched` and pending ISR wake depth with Linux-style ordering, then call the existing scheduler entry instead of sleeping when work is visible.
- Validation command: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- Maps to deliverable: Phase 1 commit.

### M3. SGI Admission Fix

- Files: `kernel/src/arch_impl/aarch64/gic.rs`.
- Description: Enable Breenix's SGI lines in each CPU redistributor after the blanket SGI/PPI disable, matching Linux's SGI/PPI CPU configuration and fixing disabled-pending SGI admission.
- Validation command: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64`
- Maps to deliverable: Phase 2 commit.

### M4. Platform Validation

- Files: validation logs and `docs/planning/f32j-idle-sgi-admission/exit.md`.
- Description: Run wait-stress and the required 5 x 120s Parallels boot sweep; stop and document `exit.md` if any required gate fails.
- Validation command: `BREENIX_WAIT_STRESS=1 ./run.sh --parallels --test 150 && ./run.sh --parallels --test 120`
- Maps to deliverable: Phase 3 validation.

### M5. Merge

- Files: git metadata, PR.
- Description: If all gates pass, open PR, merge to main, push all code and Beads state.
- Validation command: `git status --short --branch`
- Maps to deliverable: Phase 4 PR and merge.
