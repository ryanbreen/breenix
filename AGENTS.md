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

## Build & Run

### Standard Workflow: Boot Stages Testing

For normal development, use the boot stages test to verify kernel health:

```bash
# Run boot stages test - verifies kernel progresses through all checkpoints
cargo run -p xtask -- boot-stages

# Build only (no execution)
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

The boot stages test (`xtask boot-stages`) monitors serial output for expected markers at each boot phase. Add new stages to `xtask/src/main.rs` when adding new subsystems.

### GDB Debugging (For Deep Technical Issues)

Use GDB when you need to understand **why** something is failing, not just **that** it failed. GDB is the right tool when:
- You need to examine register state or memory at a specific point
- A panic occurs and you need to inspect the call stack
- You're debugging timing-sensitive issues that log output can't capture
- You need to step through code instruction-by-instruction

**Do NOT use GDB** for routine testing or to avoid writing proper boot stage markers. If you find yourself adding debug log statements in a loop, that's a sign you should use GDB instead.

```bash
# Start interactive GDB session
./breenix-gdb-chat/scripts/gdb_session.sh start
./breenix-gdb-chat/scripts/gdb_session.sh cmd "break kernel::kernel_main"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "info registers"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "backtrace 10"
./breenix-gdb-chat/scripts/gdb_session.sh serial
./breenix-gdb-chat/scripts/gdb_session.sh stop
```

### Logs
All runs are logged to `logs/breenix_YYYYMMDD_HHMMSS.log`

```bash
# View latest log
ls -t logs/*.log | head -1 | xargs less
```

## Development Workflow

### Agent-Based Development

Use agents when it’s helpful for long or iterative investigations, but it is no longer mandatory to route all work through agents. The main session may run commands, do code exploration, or perform debugging directly as needed.

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
# Build must complete with 0 warnings
cargo build --release --features testing,external_test_bins --bin qemu-uefi 2>&1 | grep -E "^(warning|error)"
# Should produce no output (no warnings/errors)
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

### Visual Progress Dashboard
**Public Dashboard**: https://v0-breenix-dashboard.vercel.app/
- Interactive visualization of POSIX compliance progress
- 12 subsystem regions with completion percentages
- Phase timeline showing current position (Phase 8.5)

**Updating the Dashboard**: Use the `collaboration:ux-research` skill to update the v0.dev dashboard.
When features are completed, invoke the skill to update progress percentages and feature lists.

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

## Userland Development Stages

The path to full POSIX libc compatibility is broken into 5 stages:

### Stage 1: libbreenix (Rust) - ✅ ~80% Complete
Location: `libs/libbreenix/`

Provides syscall wrappers for Rust programs:
- `process.rs` - exit, fork, exec, getpid, gettid, yield
- `io.rs` - read, write, stdout, stderr
- `time.rs` - clock_gettime (REALTIME, MONOTONIC)
- `memory.rs` - brk, sbrk
- `errno.rs` - POSIX errno definitions
- `syscall.rs` - raw syscall primitives (syscall0-6)

**Usage in test programs:**
```rust
use libbreenix::{io::println, process::exit, time::now_monotonic};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    println("Hello from userspace!");
    let ts = now_monotonic();
    exit(0);
}
```

### Stage 2: Rust Runtime - 📋 Planned
- Panic handler for userspace
- Global allocator (using brk/sbrk)
- `#[no_std]` program template
- Core abstractions (File, Process types)

### Stage 3: C libc Port - 📋 Planned
- C-compatible ABI wrappers
- stdio (printf, scanf, etc.)
- stdlib (malloc, free, etc.)
- string.h, unistd.h functions
- Option: Port musl-libc or write custom

### Stage 4: Shell - 📋 Planned
Requires: Stage 3, filesystem syscalls, pipe/dup
- Command parsing
- Built-in commands (cd, exit, echo)
- External command execution
- Piping and redirection
- Job control (requires signals)

### Stage 5: Coreutils - 📋 Planned
Requires: Stage 4, full filesystem
- Basic: cat, echo, true, false
- File ops: ls, cp, mv, rm
- Dir ops: mkdir, rmdir
- Text: head, tail, wc

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

## 🚨 PROHIBITED CODE SECTIONS 🚨

The following files carry the tightest constraints in the tree. They are hot paths: what is prohibited in them is *what you put there*, not the act of editing them.

### Tier 1: Highest Scrutiny (edit only to repair a defect that lives here)

**Standing policy (operator, 2026-09-04).** A Tier-1 file MAY be edited when the producing defect lives in it. Do not contort a fix to route around the file, and do not ship a known-live defect because its home is on this list -- that is the "any failure you find is your problem" rule, and it applies here too. The conditions are:

1. **The defect must actually live here.** A Tier-1 edit is for repairing this file's own behaviour, not for instrumenting it, not for convenience, and not for working around a defect that lives elsewhere.
2. **Minimal, and committed alone.** The smallest change that repairs the defect, in its own commit touching no other file, so the diff can be read on its own.
3. **Separately reviewed, and explained in the PR body.** The PR says which file, why the defect lives there, why no non-Tier-1 change repairs it, and what the change costs on the path. A reviewer signs off on that commit specifically.
<!-- claim-lint:ok: 6 of 6 constraints listed in the next item (logging macros,
     locks, heap allocation, string formatting, page-table/mapping operations,
     I/O) are stated verbatim elsewhere in this same file and predate the
     2026-09-04 ruling: the "Detecting Violations" red-flag list below and the
     "MANDATORY RULES" list in the interrupt/syscall section. The ruling
     relaxed the edit gate and 0 of those 6. #737. -->
4. **The absolute constraints are unchanged.** In these files, still never: `log::*` / `serial_println!` / any logging macro, any lock (`try_lock()` with a direct-hardware fallback only), any heap allocation, any string formatting, any page-table walk or memory-mapping operation, any I/O. The timing budgets still bind -- interrupt handlers target <1000 cycles, and the syscall entry/exit path stays minimal. A change that adds any of those is refused whatever its justification.
<!-- claim-lint:ok: the next item is an instruction to a future author, not a
     measurement of this tree. The worked example it points at is #737's, whose
     reproduction and mutation-tested ratchet are recorded at
     docs/planning/green-program/nic-bus/737-DF-ORACLE-2026-09-04.md sections 3
     and 4. #737. -->
5. **Prove it on the hardware path.** A Tier-1 edit lands with boot evidence, not with a build. Use GDB and the tracing framework to understand the defect; never add a log statement to one of these files to find it.

Worked example: #737's fix is a single `cld` in `kernel/src/interrupts/timer_entry.asm` -- the direction flag is inherited across an interrupt gate, so the defect has no other home. It shipped as one instruction, in one commit, with an oracle boot and a source-level ratchet either side of it.

| File | Reason |
|------|--------|
| `kernel/src/syscall/handler.rs` | Syscall hot path - ANY logging breaks timing tests |
| `kernel/src/syscall/time.rs` | clock_gettime precision - called in tight loops |
| `kernel/src/syscall/entry.asm` | Assembly syscall entry - must be minimal |
| `kernel/src/interrupts/timer.rs` | Timer fires every 1ms - <1000 cycles budget |
| `kernel/src/interrupts/timer_entry.asm` | Assembly timer entry - must be minimal |

### Tier 2: High Scrutiny (explain why GDB is insufficient)
| File | Reason |
|------|--------|
| `kernel/src/interrupts/context_switch.rs` | Context switch path - timing sensitive |
| `kernel/src/interrupts/mod.rs` | Interrupt dispatch - timing sensitive |
| `kernel/src/task/kthread.rs` | kthread_entry runs with interrupts enabled - log deadlocks |
| `kernel/src/task/workqueue.rs` | worker_thread_fn runs with interrupts - log deadlocks |
| `kernel/src/gdt.rs` | GDT/TSS - rarely needs changes |
| `kernel/src/per_cpu.rs` | Per-CPU data - used in hot paths |

### When Modifying These Files

<!-- claim-lint:ok: the approval gate this list used to carry ("Get explicit
     user approval before making any changes") was removed for BOTH tiers on
     2026-09-04. Each removal rests on its own operator ruling, and neither
     ruling is this round's invention:
       * Tier 1 -- R156, operator, 2026-09-04, verbatim "make tier 1 changes if
         necessary". What replaces the gate is the five conditions above.
       * Tier 2 -- operator, 2026-08-12, "Tier-2 files are editable when the
         approach needs it; do not contort to avoid them", recorded in-repo at
         docs/planning/teardown-unification/P3-RERATIFICATION-2026-08-15.md
         lines 82-83, 99 and 180. That ruling predates this file's wording by
         three weeks; this edit is CLAUDE.md catching up to it, and R156 is not
         what authorises it.
     Item 2's narrowing to diagnostic-only additions is likewise a second
     relaxation, disclosed at the item itself rather than folded into the
     first. #737. -->

Before editing a Tier-1 or Tier-2 file:

1. **Say where the defect lives** - a Tier-1 or Tier-2 edit repairs this file's own behaviour; if the defect lives elsewhere, fix it there
2. **Explain why GDB debugging is insufficient** for this specific problem, if what you are adding is diagnostic rather than a repair. A repair does not owe this explanation; anything you are adding in order to *observe* the path does, and the answer had better not be "logging". (Narrowed 2026-09-04 from an unconditional requirement, which read as if a one-instruction repair had to argue against a debugger first.)
<!-- claim-lint:ok: the next item is unchanged in substance from the list it
     replaces; 1 of 1 edit to it in this round added "and the tracing
     framework" to the remedy half of the sentence. The prohibition itself is
     restated below under "Detecting Violations" and in the interrupt/syscall
     section. #737. -->
3. **Never add logging** - use GDB breakpoints and the tracing framework instead
4. **Remove any temporary debug code** before committing
5. **Test via GDB, and land with boot evidence** - a passing build is not acceptance for a change on these paths

**Neither tier requires operator approval as a precondition any more** (Tier 1: operator ruling R156, 2026-09-04; Tier 2: operator ruling of 2026-08-12). What replaces it is the rest of this section: Tier 1 additionally carries the five numbered conditions above - defect lives here, minimal and committed alone, explained in the PR body and separately reviewed, absolute constraints unchanged, and it lands with boot evidence rather than with a build. Tier 2 carries this list. The absolute constraints below bind on both tiers regardless.
<!-- claim-lint:ok: 2 of 2 rulings named here are cited, not asserted: R156 is
     quoted verbatim in docs/planning/green-program/nic-bus/
     737-FIX-2026-09-04.md section 2, and the 2026-08-12 Tier-2 ruling is
     recorded at docs/planning/teardown-unification/
     P3-RERATIFICATION-2026-08-15.md:82. The boot-evidence clause is condition
     5 of the Tier-1 list restated, an instruction to a future author, not a
     measurement. #737. -->

### Detecting Violations

Look for these red flags in these files:
- `log::*` macros
- `serial_println!`
- `format!` or string formatting
- Raw serial port writes (`out dx, al` to 0x3F8)
- Any I/O operations

## Interrupt and Syscall Development - CRITICAL PATH REQUIREMENTS

**The interrupt and syscall paths MUST remain pristine.** This is non-negotiable architectural guidance.

### Why This Matters

Timer interrupts fire every ~1ms (1000 Hz). At 3 GHz, that's only 3 million cycles between interrupts. If the timer handler takes too long:
- Nested interrupts pile up
- Stack overflow occurs
- Userspace never executes (timer fires before IRETQ completes)

Real-world example: Adding 230 lines of page table diagnostics to `trace_iretq_to_ring3()` caused timer interrupts to fire within 100-500 cycles after IRETQ, before userspace could execute a single instruction. Result: 0 syscalls executed, infinite kernel loop.

### MANDATORY RULES

**In interrupt handlers (`kernel/src/interrupts/`):**
- NO serial output (`serial_println!`, `log!`, `debug!`)
- NO page table walks or memory mapping operations
- NO locks that might contend (use `try_lock()` with direct hardware fallback)
- NO heap allocations
- NO string formatting
- Target: <1000 cycles total

**In syscall entry/exit (`kernel/src/syscall/entry.asm`, `handler.rs`):**
- NO logging on the hot path
- NO diagnostic tracing by default
- Frame transitions must be minimal

**Stub functions for assembly references:**
If assembly code calls logging functions that were removed, provide empty `#[no_mangle]` stubs rather than modifying assembly. See `kernel/src/interrupts/timer.rs` for examples.

### Approved Debugging Alternatives

1. **QEMU interrupt tracing**: `BREENIX_QEMU_DEBUG_FLAGS="int,cpu_reset"` logs to file without affecting kernel timing
2. **GDB breakpoints**: `BREENIX_GDB=1` enables GDB server
3. **Post-mortem analysis**: Analyze logs after crashes, not during execution
4. **Dedicated diagnostic threads**: Run diagnostics in separate threads with proper scheduling

### Code Review Checklist

Before approving changes to interrupt/syscall code:
- [ ] No `serial_println!` or logging macros
- [ ] No page table operations
- [ ] No locks without try_lock fallback
- [ ] No heap allocations
- [ ] Timing-critical paths marked with comments

## GDB Debugging - Recommended (Not Required)

GDB is the preferred tool for root-cause debugging of timing-sensitive or low-level issues. Boot stages and end-to-end boot task tests are the default for verification and CI.

Running without GDB provides only serial output; that's often sufficient for boot-stage verification, but it won't help when you need register state, memory inspection, or breakpoints.

### Interactive GDB Session (Optional Workflow)

Use `gdb_session.sh` for persistent, interactive debugging sessions:

```bash
# Start a persistent session (keeps QEMU + GDB running)
./breenix-gdb-chat/scripts/gdb_session.sh start

# Send commands one at a time, making decisions based on results
./breenix-gdb-chat/scripts/gdb_session.sh cmd "break kernel::syscall::time::sys_clock_gettime"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"
# Examine what happened, then decide next step...
./breenix-gdb-chat/scripts/gdb_session.sh cmd "info registers rax rdi rsi"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "print/x \$rdi"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "backtrace 10"

# Get all serial output (kernel print statements)
./breenix-gdb-chat/scripts/gdb_session.sh serial

# Stop when done
./breenix-gdb-chat/scripts/gdb_session.sh stop
```

This is **conversational debugging** - you send a command, see the result, think about it, and decide what to do next. Just like a human sitting at a GDB terminal.

### GDB Chat Tool (Underlying Engine)

The session wrapper uses `breenix-gdb-chat/scripts/gdb_chat.py`:

```bash
# Can also use directly for scripted debugging
printf 'break kernel::kernel_main\ncontinue\ninfo registers\nquit\n' | python3 breenix-gdb-chat/scripts/gdb_chat.py
```

The tool:
1. Starts QEMU with GDB server enabled (`BREENIX_GDB=1`)
2. Starts GDB and connects to QEMU on localhost:1234
3. Loads kernel symbols at the correct PIE base address (0x10000000000)
4. Accepts commands via stdin, returns JSON responses with serial output included
5. **No automatic interrupt** - you control the timeout per command

### Essential GDB Commands

**Setting breakpoints:**
```
break kernel::kernel_main              # Break at function
break kernel::syscall::time::sys_clock_gettime
break *0x10000047b60                   # Break at address
info breakpoints                       # List all breakpoints
delete 1                               # Delete breakpoint #1
```

**Execution control:**
```
continue                               # Run until breakpoint or interrupt
stepi                                  # Step one instruction
stepi 20                               # Step 20 instructions
next                                   # Step over function calls
finish                                 # Run until current function returns
```

**Inspecting state:**
```
info registers                         # All registers
info registers rip rsp rax rdi rsi     # Specific registers
backtrace 10                           # Call stack (10 frames)
x/10i $rip                             # Disassemble 10 instructions at RIP
x/5xg $rsp                             # Examine 5 quad-words at RSP
x/2xg 0x7fffff032f98                   # Examine memory at address
print/x $rax                           # Print register in hex
```

**Kernel-specific patterns:**
```
# Check if syscall returned correctly (RAX = 0 for success)
info registers rax

# Examine userspace timespec after clock_gettime
x/2xg $rsi                             # tv_sec, tv_nsec

# Check stack frame integrity
x/10xg $rsp

# Verify we're in userspace (CS RPL = 3)
print $cs & 3
```

### Debugging Workflow

1. **Set breakpoints BEFORE continuing:**
   ```
   break kernel::syscall::time::sys_clock_gettime
   continue
   ```

2. **Examine state at breakpoint:**
   ```
   info registers rip rdi rsi          # RIP, syscall args
   backtrace 5                          # Where did we come from?
   ```

3. **Step through problematic code:**
   ```
   stepi 10                             # Step through instructions
   info registers rax                   # Check return value
   ```

4. **Inspect memory if needed:**
   ```
   x/2xg 0x7fffff032f98                 # Examine user buffer
   ```

### Symbol Loading

The PIE kernel loads at base address `0x10000000000` (1 TiB). The gdb_chat.py tool handles this automatically via `add-symbol-file` with correct section offsets:

- `.text` offset: varies by build
- Runtime address = `0x10000000000 + elf_section_offset`

If symbols don't resolve, verify with:
```
info address kernel::kernel_main
```

### When to Use GDB vs Boot Stages

**Use boot stages** (`cargo run -p xtask -- boot-stages`) for:
- Verifying a fix works
- Checking that all subsystems initialize
- CI/continuous testing
- Quick sanity checks

**Use GDB** for:
- Understanding why a specific failure occurs
- Examining register/memory state at a crash
- Stepping through complex code paths
- Debugging timing-sensitive issues where adding logs would change behavior

### Anti-Patterns

```bash
# DON'T add logging to hot paths (syscalls, interrupts) to debug issues
log::debug!("clock_gettime called");  # This changes timing!

# DON'T loop on adding debug prints - use GDB breakpoints instead
# If you're on your 3rd round of "add log, rebuild, run", switch to GDB
```

### GDB Debugging Example

```bash
# Start interactive GDB session
./breenix-gdb-chat/scripts/gdb_session.sh start
./breenix-gdb-chat/scripts/gdb_session.sh cmd "break kernel::syscall::time::sys_clock_gettime"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"

# Examine state at breakpoint
./breenix-gdb-chat/scripts/gdb_session.sh cmd "info registers rdi rsi"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "backtrace 10"

# Stop when done
./breenix-gdb-chat/scripts/gdb_session.sh stop
```

## QEMU Process Cleanup - MANDATORY

**Agents MUST clean up stray QEMU processes.** This is non-negotiable.

QEMU processes frequently get orphaned during testing, debugging, or when agents are interrupted. These orphaned processes:
- Hold locks on disk images, preventing new QEMU instances from starting
- Consume system resources
- Cause confusing errors like "Failed to get write lock"

### Cleanup Requirements

1. **Before handing control back to the user**: Always run QEMU cleanup
2. **Before running any QEMU command**: Kill any existing QEMU processes first
3. **When debugging fails or times out**: Clean up QEMU before reporting results

### Cleanup Command

```bash
pkill -9 qemu-system-x86 2>/dev/null; killall -9 qemu-system-x86_64 2>/dev/null; pgrep -l qemu || echo "All QEMU processes killed"
```

### When to Clean Up

- After any `xtask boot-stages` or `xtask interactive` run
- After GDB debugging sessions
- When the user reports "cannot acquire lock" errors
- Before starting any new QEMU-based test
- When handing results back to the user after kernel work

This is the agent's responsibility - do not wait for the user to ask.

## Work Tracking

We use GitHub Issues (not Beads/bd, and not Markdown TODO files) for issue tracking in this repo.

### Quick Reference

```bash
gh issue list                          # Find available work
gh issue view <number>                 # View issue details
gh issue create --title ... --body ... # File a new issue
gh issue close <number>                # Complete work
```

### Rules

- Use `gh issue` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Use MEMORY.md / project memory for persistent agent knowledge

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create GitHub issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work (`gh issue close`), update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
