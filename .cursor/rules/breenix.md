## Breenix Cursor Rules

These rules govern how the assistant works in this repository. Follow them strictly. Quality and correctness outweigh speed.

### Project Overview
- **Breenix** is a production-quality x86_64 OS kernel written in Rust. Not a toy.
- Kernel is `#![no_std]`, runs on bare metal with a custom target.
- Repo structure highlights:
  - `kernel/` core kernel implementation
  - `libs/` supporting libraries
  - `tests/` integration tests
  - `docs/planning/` roadmap and design docs

### Critical Command Line Policy
- **Never** generate unique ad-hoc shell commands that require user approval.
- **Always** use the provided reusable scripts/utilities; add a new script first if needed.
- **Log searching**: Use `./scripts/find-in-logs` configured via `/tmp/log-query.txt`.
- Prefer non-interactive, deterministic commands. Avoid prompts and background long-lived processes.

Examples for log searches:
```bash
echo '-A50 "Creating user process"' > /tmp/log-query.txt
./scripts/find-in-logs

echo '-E "Fork succeeded|exec succeeded"' > /tmp/log-query.txt
./scripts/find-in-logs
```

### Critical Mindset: No Time Constraints
- **There are no time constraints — only quality matters.**
- Iterate until changes are accepted. Address all feedback thoroughly.
- Do not take shortcuts due to complexity; build production-grade solutions.

### Critical Design Principle: Follow OS-Standard Practices
- Use Linux/FreeBSD patterns as the standard.
- No quick hacks; implement correct mechanisms that scale.
- Required patterns include (non-exhaustive):
  - Proper page table switching for `exec()` ELF loading (no double-mapping)
  - Correct copy-on-write `fork()`
  - Standard syscall interfaces/semantics
  - Real virtual memory isolation
  - Proper interrupt/exception handling

### Running Breenix
- Preferred wrappers (these auto-manage logs):
  - `./scripts/run_breenix.sh`
  - `./scripts/run_test.sh`

Direct commands (console-only logs):
```bash
cargo run --release --bin qemu-uefi -- -serial stdio -display none
cargo run --release --bin qemu-bios -- -serial stdio -display none
cargo run --release --features testing --bin qemu-uefi -- -serial stdio
```

### Logs
- All kernel runs produce timestamped logs in `logs/` (e.g., `breenix_YYYYMMDD_HHMMSS.log`).
- Use `./scripts/find-in-logs` for all searches (configure via `/tmp/log-query.txt`).

Typical patterns to search for:
- **Success**: words like "succeeded" or `✓`
- **Failures**: "failed", "ERROR", "DOUBLE FAULT"
- **Userspace execution proof**: explicit logs showing usermode instructions and syscalls

### Development Workflow
1. Make code changes in `kernel/src/` and related components.
2. Run via scripts or tests; logs go to `logs/`.
3. Analyze with `./scripts/find-in-logs`.
4. Compare against known-good patterns; investigate any regressions.

Automated testing (preferred during development):
```bash
./scripts/breenix_runner.py > /dev/null 2>&1 &
sleep 15  # wait for boot + tests
```

### Testing and Test Infrastructure
- Most tests use a shared QEMU instance for speed (~45s total).
- Standard test entry point:
```bash
cargo test
```

Test categories include (non-exhaustive):
- `boot_post_test.rs`, `interrupt_tests.rs`, `memory_tests.rs`, `logging_tests.rs`, `timer_tests.rs`, `simple_kernel_test.rs`, `kernel_build_test.rs`, `system_tests.rs`

Special tests (ignored by default):
```bash
cargo test test_bios_boot -- --ignored
cargo test test_runtime_testing_feature -- --ignored
cargo run --features testing --bin qemu-uefi -- -serial stdio
```

Visual testing:
```bash
BREENIX_VISUAL_TEST=1 cargo test
BREENIX_VISUAL_TEST=1 cargo test memory
```

Interactive manual testing utility:
```bash
./scripts/test_kernel.sh
```

### Coding Practices
- Rust nightly; custom target `x86_64-breenix.json`; panic strategy: abort; red zone disabled.
- Clear module organization; const-correct hardware constants; explicit error handling.
- Code style: descriptive names, early returns, minimal nesting, meaningful comments only where needed.

#### Build Quality Requirements
- Treat all warnings as errors; code must compile cleanly with `cargo build`.
- Fix all clippy warnings when available.
- Use `#[allow(dead_code)]` only for legitimate soon-to-be-used APIs.

### Pull Request Workflow
- Never push directly to `main`.
- Always branch from latest `main` and use feature branches.
- Create PRs with GitHub CLI; include summary, implementation details, testing results, legacy parity improvements, and co-authorship credit.

Example flow:
```bash
git checkout main && git pull origin main
git checkout -b feature-name
# ... changes ...
git push -u origin feature-name
gh pr create --title "Brief description" --body "Detailed description with testing results"
```

### Critical Debugging Requirement: Proof via Logs
- Never declare success without definitive log evidence.
- Proof of userspace execution requires logs like:
```text
[INFO] Userspace instruction executed at 0x10000000
[INFO] Syscall 0x80 received from userspace
[INFO] Returning to userspace at 0x10000005
```
- A crash (e.g., DOUBLE FAULT) is not proof of execution.
- Critical baseline: ensure "Hello from userspace!" output in direct test before deeper debugging.

### Validation Requirement
- Always present implementation details and log evidence for validation.
- Request review/verification and iterate until acceptance.
- This file intentionally avoids MCP-specific agent invocation details.

### Documentation and Roadmap
- Master roadmap: `docs/planning/PROJECT_ROADMAP.md`
  - Update after each PR merge (Recently Completed)
  - Update when starting new work (Currently Working On)
  - Weekly review (Immediate Next Steps)
- Additional docs:
  - `docs/planning/legacy-migration/FEATURE_COMPARISON.md`
  - `docs/planning/06-userspace-execution/USERSPACE_SUMMARY.md`
  - `docs/planning/posix-compliance/POSIX_COMPLIANCE.md`

### Legacy Code Removal Policy
- Remove legacy code from `src.legacy/` once the new implementation reaches parity or better and is verified.
- Update `FEATURE_COMPARISON.md` accordingly and do removal in the same commit when practical.

### Cleanup Utilities
```bash
pkill -f qemu-system-x86_64
ls -t logs/*.log | tail -n +11 | xargs rm -f
```

### Context Compression Reminder
- If conversation context is compressed, immediately re-read this `.cursor/rules` file to refresh critical project instructions.

### Development Notes
- Commits should be co-developed by Ryan Breen and the assistant when appropriate.

