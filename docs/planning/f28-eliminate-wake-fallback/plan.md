# Plan - F28 Eliminate the 5ms Wake Fallback

## Milestones

### M1. Instrument Wake Counters

- Files: `kernel/src/syscall/graphics.rs`, `docs/planning/f28-eliminate-wake-fallback/phase1.md`, `docs/planning/f28-eliminate-wake-fallback/scratchpad.md`
- Description: Add atomic counters for client frame waits completed by compositor upload wake versus by the 5 ms fallback timeout. Emit low-frequency serial summaries so the Parallels test log records the counts.
- Validation command: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f28-kernel-build.log; build_status=${pipestatus[1]}; if [ "$build_status" -ne 0 ]; then exit "$build_status"; fi; ! grep -E "^(warning|error)" /tmp/f28-kernel-build.log`
- Maps to deliverable: Counter instrumentation committed.

### M2. Reproduce Baseline Ratio

- Files: `docs/planning/f28-eliminate-wake-fallback/phase2.md`, `logs/f28/baseline/serial.log`
- Description: Run the 120 second Parallels GUI workload on the instrumentation-only build. Record event wake count, fallback count, fallback percentage, and FPS estimate.
- Validation command: `test -s docs/planning/f28-eliminate-wake-fallback/phase2.md && test -s logs/f28/baseline/serial.log`
- Maps to deliverable: Phase 2 reproduction numbers recorded.

### M3. Fix Wake Race

- Files: `kernel/src/syscall/graphics.rs`, `docs/planning/f28-eliminate-wake-fallback/phase3.md`
- Description: Fix the identified missed-wake race without polling, busy-waiting, or Tier 1 edits.
- Validation command: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f28-kernel-build.log; build_status=${pipestatus[1]}; if [ "$build_status" -ne 0 ]; then exit "$build_status"; fi; ! grep -E "^(warning|error)" /tmp/f28-kernel-build.log`
- Maps to deliverable: Phase 3 fix committed with root-cause analysis.

### M4. Final 120s Validation

- Files: `docs/planning/f28-eliminate-wake-fallback/phase4.md`, `logs/f28/final/serial.log`, `docs/planning/f28-eliminate-wake-fallback/exit.md`
- Description: Run clean userspace/kernel builds and a final 120 second Parallels GUI workload. Confirm fallback counter below 0.1%, FPS at or above 160 Hz, render verdict passes, and fault-marker grep is empty.
- Validation command: `cargo build --release --features testing,external_test_bins --bin qemu-uefi 2>&1 | tee /tmp/f28-x86-build.log; build_status=${pipestatus[1]}; if [ "$build_status" -ne 0 ]; then exit "$build_status"; fi; ! grep -E "^(warning|error)" /tmp/f28-x86-build.log`
- Maps to deliverable: Final validation, PR opened and merged, back on main.
