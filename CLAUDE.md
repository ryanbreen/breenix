# Breenix OS

## Project Overview

Breenix is a production-quality x86_64 operating system kernel written in Rust. This is not a toy or learning project - we follow Linux/FreeBSD standard practices and prioritize quality over speed.

## Project Structure

```
kernel/          # Core kernel (no_std, no_main)
src.legacy/      # Previous implementation (being phased out)
libs/            # libbreenix, tiered_allocator
tests/           # Integration tests
docs/planning/   # Numbered phase directories (00-15)
```

## Build & Test

### Quick Start
```bash
# Run with automatic logging
./scripts/run_breenix.sh

# Run tests (21 tests with shared QEMU, ~45 seconds)
cargo test

# Run specific test
cargo test memory

# Visual testing (shows QEMU window)
BREENIX_VISUAL_TEST=1 cargo test
```

### Direct Cargo Commands
```bash
# UEFI mode
cargo run --release --bin qemu-uefi -- -serial stdio -display none

# BIOS mode
cargo run --release --bin qemu-bios -- -serial stdio -display none

# With runtime testing
cargo run --features testing --bin qemu-uefi -- -serial stdio
```

### Logs
All runs are logged to `logs/breenix_YYYYMMDD_HHMMSS.log`

```bash
# View latest log
ls -t logs/*.log | head -1 | xargs less

# Search logs (avoids approval prompts)
echo '-A50 "Creating user process"' > /tmp/log-query.txt
./scripts/find-in-logs
```

## Development Workflow

### Agent-Based Development (MANDATORY)

**The main conversation is for ORCHESTRATION ONLY.** Never execute tests, run builds, or perform iterative debugging directly in the top-level session. This burns token context and leads to session exhaustion.

**ALWAYS dispatch to agents:**
- Running tests (`cargo test`, `cargo run -p xtask -- boot-stages`, etc.)
- Build verification and compilation checks
- Debugging sessions that involve iterative log analysis
- Code exploration and codebase research
- Any task that may require multiple iterations or produce verbose output

**The orchestrator session should only:**
- Plan and decompose work into agent-dispatchable tasks
- Review agent reports and synthesize findings
- Make high-level decisions based on agent results
- Coordinate multiple parallel agent investigations
- Communicate summaries and next steps to the user

**Anti-pattern (NEVER DO THIS):**
```
# DON'T run tests directly in main session
cargo run -p xtask -- boot-stages
# DON'T grep through large outputs in main session
cat target/output.txt | grep ...
```

**Correct pattern:**
```
# DO dispatch to an agent with clear instructions
Task(subagent_type="general-purpose", prompt="Run boot-stages test, analyze
the output, and report which stage fails and why. Include relevant log excerpts.")
```

When a debugging task requires multiple iterations, dispatch it ONCE to an agent with comprehensive instructions. The agent will iterate internally and return a summary. If more investigation is needed, dispatch another agent - don't bring the iteration into the main session.

### Feature Branches (REQUIRED)
Never push directly to main. Always:
```bash
git checkout main
git pull origin main
git checkout -b feature-name
# ... do work ...
git push -u origin feature-name
gh pr create --title "Brief description" --body "Details"
```

### Code Quality - ZERO TOLERANCE FOR WARNINGS

**Every build must be completely clean.** Zero warnings, zero errors. This is non-negotiable.

When you run any build or test command and observe warnings or errors in the compile stage, you MUST fix them before proceeding. Do not continue with broken builds.

**Honest fixes only.** Do NOT suppress warnings dishonestly:
- `#[allow(dead_code)]` is NOT acceptable for code that should be removed or actually used
- `#[allow(unused_variables)]` is NOT acceptable for variables that indicate incomplete implementation
- Prefixing with `_` is NOT acceptable if the variable was meant to be used
- These annotations hide problems instead of fixing them

**When to use suppression attributes:**
- `#[allow(dead_code)]` ONLY for legitimate public API functions that are intentionally available but not yet called (e.g., `SpinLock::try_lock()` as part of a complete lock API)
- `#[cfg(never)]` for code intentionally disabled for debugging (must be in Cargo.toml check-cfg)
- Never use suppressions to hide incomplete work or actual bugs

**Proper fixes:**
- Unused variable? Either use it (complete the implementation) or remove it entirely
- Dead code? Either call it or delete it
- Unnecessary `mut`? Remove the `mut`
- Unnecessary `unsafe`? Remove the `unsafe` block

**Before every commit, verify:**
```bash
cargo run -p xtask -- boot-stages  # Must show 0 warnings in compile output
```

### Testing Integrity - CRITICAL

**NEVER fake a passing test.** If a test fails, it fails. Do not:
- Add fallbacks that accept weaker evidence than the test requires
- Change test criteria to match broken behavior
- Accept "process was created" as proof of "process executed correctly"
- Let CI pass by detecting markers printed before the actual test runs

If a test cannot pass because the underlying code is broken:
1. **Fix the underlying code** - this is the job
2. Or disable the test explicitly with documentation explaining why
3. NEVER make the test pass by weakening its criteria

A test that passes without testing what it claims to test is worse than a failing test - it gives false confidence and hides real bugs.

### Testing
- Most tests use shared QEMU (`tests/shared_qemu.rs`)
- Special tests marked `#[ignore]` require specific configs
- Tests wait for: `🎯 KERNEL_POST_TESTS_COMPLETE 🎯`
- BIOS test: `cargo test test_bios_boot -- --ignored`

### Commits
All commits co-authored by Ryan Breen and Claude Code.

## Documentation

### Master Roadmap
`docs/planning/PROJECT_ROADMAP.md` tracks:
- Current development status
- Completed phases (✅)
- In progress (🚧)
- Planned work (📋)

Update after each PR merge and when starting new work.

### Structure
- `docs/planning/00-15/` - Phase directories
- `docs/planning/legacy-migration/FEATURE_COMPARISON.md` - Track migration progress
- Cross-cutting dirs: `posix-compliance/`, `legacy-migration/`

## Legacy Code Removal

When new implementation reaches parity:
1. Remove code from `src.legacy/`
2. Update `FEATURE_COMPARISON.md`
3. Include removal in same commit as feature completion

## Build Configuration

- Custom target: `x86_64-breenix.json`
- Nightly Rust with `rust-src` and `llvm-tools-preview`
- Panic strategy: abort
- Red zone: disabled for interrupt safety
- Features: `-mmx,-sse,+soft-float`

## Work Tracking

We use Beads (bd) instead of Markdown for issue tracking. Run `bd quickstart` to get started.
