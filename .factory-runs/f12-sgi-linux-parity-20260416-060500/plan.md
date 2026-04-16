# Plan - F12 SGI Linux Parity

## Milestones

### M1. Reference and Patch
- Files: `kernel/src/arch_impl/aarch64/gic.rs`, run scratchpad
- Description: Compare Linux v6.8 SGI emission with Breenix, then add only the missing Linux ordering and per-CPU SRE audit.
- Validation command: `cargo build --release --target aarch64-breenix.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -p kernel --bin kernel-aarch64 2>&1 | tee /tmp/f12-aarch64-build.log; ! grep -E "^(warning|error)" /tmp/f12-aarch64-build.log && git diff --check`
- Maps to deliverable: code change and clean aarch64 build.

### M2. Code Commit
- Files: git history
- Description: Commit the Linux-parity code patch with Linux and Breenix file:line citations in the commit body.
- Validation command: `git log -1 --format=%B | grep -q 'drivers/irqchip/irq-gic-v3.c:1350-1387' && git log -1 --format=%B | grep -q 'kernel/src/arch_impl/aarch64/gic.rs'`
- Maps to deliverable: commit 1.

### M3. Parallels Sweep
- Files: `logs/breenix-parallels-cpu0/f12-sgi-parity/run1` through `run5`
- Description: Run the required five Parallels samples, summarize bsshd/AHCI/corruption/SRE results, and preserve logs in the requested directory.
- Validation command: `test -f logs/breenix-parallels-cpu0/f12-sgi-parity/run5/summary.txt`
- Maps to deliverable: validation sweep.

### M4. Documentation and Final Commit
- Files: `docs/planning/ARM64_CPU0_SMP_INVESTIGATION.md`, `.factory-runs/f12-sgi-linux-parity-20260416-060500/exit.md`
- Description: Append the 2026-04-16 investigation section, write exit documentation, close/update Beads, commit and push.
- Validation command: `git status --short && test -f .factory-runs/f12-sgi-linux-parity-20260416-060500/exit.md`
- Maps to deliverable: commit 2, exit docs, pushed branch.
