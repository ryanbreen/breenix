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

### Agent-Based Development (MANDATORY) 🚨

**READ THIS FIRST - THIS IS THE MOST IMPORTANT RULE IN THIS ENTIRE FILE**

The main conversation is for **ORCHESTRATION ONLY**. Period. Full stop. No exceptions.

#### Why This Matters

Long debugging sessions in the main conversation cause:
1. **Token context exhaustion** - The session fills up with build output, test logs, and iterative attempts
2. **Quality degradation** - As context fills, you start cutting corners and making mistakes
3. **"Fatigue cheating"** - You weaken tests to pass, suppress warnings instead of fixing them, or accept "good enough" solutions
4. **Loss of focus** - The original task gets buried under layers of debugging output

Agents solve this by giving **fresh context** for each discrete task. They can't cheat because they don't have the fatigue. They can't cut corners because they don't have the history of failed attempts.

#### The Iron Law: What NEVER Happens in Main Session

**NEVER in the main conversation:**
- ❌ Run tests (`cargo test`, `cargo run -p xtask -- boot-stages`, etc.)
- ❌ Run builds or compilation checks
- ❌ Execute the kernel (`./scripts/run_breenix.sh`, `cargo run`)
- ❌ Perform iterative debugging or log analysis
- ❌ Write significant implementation code
- ❌ Explore the codebase with multiple file reads
- ❌ Search through large outputs with grep/sed/awk
- ❌ Any task that might require iteration or produce verbose output

**If you catch yourself typing `cargo`, `./scripts/`, or opening multiple files to understand something - STOP. Dispatch an agent.**

#### What the Orchestrator DOES Do

The main session is for:
- ✅ Planning work and decomposing into agent-dispatchable tasks
- ✅ Reviewing agent reports and synthesizing findings
- ✅ Making high-level architectural decisions
- ✅ Coordinating multiple parallel agent investigations
- ✅ Communicating summaries and decisions to the user
- ✅ Updating documentation (CLAUDE.md, PROJECT_ROADMAP.md)
- ✅ Small, targeted file edits based on agent findings (e.g., fixing a specific line an agent identified)

Think of yourself as a **project manager**, not an **implementer**. You delegate all actual work.

#### Anti-Patterns (NEVER DO THIS)

```bash
# ❌ DON'T run tests directly in main session
cargo run -p xtask -- boot-stages

# ❌ DON'T build in main session
cargo build --release

# ❌ DON'T debug iteratively in main session
./scripts/run_breenix.sh
# (check output)
# (make change)
./scripts/run_breenix.sh
# (check again)
# ... this cycle BURNS context

# ❌ DON'T grep through large outputs in main session
cat target/output.txt | grep -A50 "error"

# ❌ DON'T explore code directly in main session
Read file1.rs
Read file2.rs
Read file3.rs
# ... burns tokens trying to understand
```

#### Correct Patterns (DO THIS)

```bash
# ✅ DO dispatch to an agent with clear instructions
Task(subagent_type="general-purpose", prompt="Run boot-stages test, analyze
the output, and report which stage fails and why. Include relevant log excerpts.")

# ✅ DO dispatch debugging to an agent
Task(subagent_type="general-purpose", prompt="Debug why clock_gettime test is
failing. Run the test, analyze logs, identify root cause, and propose a fix.
Include the specific code location and error message.")

# ✅ DO dispatch code exploration to an agent
Task(subagent_type="general-purpose", prompt="Find all places where we handle
syscall arguments. Map out the flow from syscall entry to argument extraction
to handler dispatch. Report the file paths and function names involved.")

# ✅ DO dispatch implementation to an agent
Task(subagent_type="general-purpose", prompt="Implement the clock_gettime
syscall handler. Follow the existing syscall patterns in syscall/time.rs.
Ensure zero compiler warnings. Report when complete with the changes made.")

# ✅ DO coordinate multiple agents for complex tasks
Task 1: "Run boot-stages and report results"
Task 2: "Run integration tests and report failures"
Task 3: "Analyze logs from latest run and identify errors"
# Then synthesize their reports in main session
```

#### Agent Dispatch Guidelines

**When dispatching agents:**
1. **Be comprehensive** - Give the agent enough context to complete the task independently
2. **Be specific** - State exactly what you want: test results, root cause, proposed fix, etc.
3. **Request structured output** - Ask for file paths, line numbers, specific error messages
4. **Single dispatch for iterative work** - If debugging requires multiple attempts, let the AGENT iterate, not you

**Example of a good agent dispatch:**
```
Task(subagent_type="general-purpose", prompt="
The clock_gettime test is failing. Your mission:

1. Run: cargo test clock_gettime
2. Analyze the failure output and kernel logs
3. Identify the root cause (be specific: file, function, line if possible)
4. Determine if this is a test issue or an implementation bug
5. Propose a fix with rationale

Deliver:
- Root cause statement
- Relevant code excerpts
- Proposed fix or next investigation step
")
```

**When a debugging task requires multiple iterations**, dispatch it ONCE to an agent with comprehensive instructions. The agent will iterate internally and return a summary. If more investigation is needed after reviewing the agent's report, dispatch ANOTHER agent - don't bring the iteration into the main session.

#### Quality Enforcement

This rule exists to maintain quality. When you violate it:
- Your context fills with noise
- You start making mistakes
- You cut corners to "just get it working"
- You suppress warnings instead of fixing them
- You weaken tests instead of fixing bugs

**If you find yourself tempted to "just quickly run this test" in the main session, that's EXACTLY when you should dispatch an agent.** The temptation is a sign of fatigue, which is a sign you need fresh context.

#### Emergency Override

The ONLY exception: If the user explicitly says "run X in this session" or "don't use agents for this", then and only then can you violate this rule. Otherwise, it's agents all the way down.

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
