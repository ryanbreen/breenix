# Plan - F11 Send SGI Boundary

## Milestones

### M1. Add Breadcrumb Sites

- Files: `kernel/src/drivers/ahci/mod.rs`,
  `kernel/src/arch_impl/aarch64/gic.rs`,
  `kernel/src/task/scheduler.rs`
- Description: Add requested AHCI ring tags and insert SGI/wake-buffer
  breadcrumbs without changing control flow.
- Validation command:
  `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f11-aarch64-build.log; test ${PIPESTATUS[0]} -eq 0; ! grep -E '^(warning|error)' /tmp/f11-aarch64-build.log && git diff --check`

### M2. Commit Diagnostic Code

- Files: code files from M1
- Description: Commit the code-only diagnostic breadcrumb patch.
- Validation command:
  `git status --short && git diff --cached --check`

### M3. Run Five-Sample Sweep

- Files: `logs/breenix-parallels-cpu0/f11-send-sgi/run{1..5}/summary.txt`
- Description: Execute five Parallels test runs, preserve serial logs and
  summarize all F10 plus F11 site counts.
- Validation command:
  `for i in 1 2 3 4 5; do test -s logs/breenix-parallels-cpu0/f11-send-sgi/run$i/summary.txt; done`

### M4. Document Results

- Files: `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md`,
  `.factory-runs/arm64-f11-send-sgi-boundary-20260416-060000/exit.md`
- Description: Append F11 verdict, verbatim extracts, and F12 recommendation.
- Validation command:
  `rg -n 'F11|SGI_|WAKEBUF_|F12' docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md .factory-runs/arm64-f11-send-sgi-boundary-20260416-060000/exit.md`
