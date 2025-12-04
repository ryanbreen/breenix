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

### 🚨 MANDATORY: All Kernel Execution Through GDB 🚨

**ALL kernel execution MUST go through GDB.** This is non-negotiable.

Even when you just want to "run the kernel and see what happens", you MUST use GDB. This ensures:
- You can intercept panics and examine state
- You can set breakpoints if something goes wrong
- You see both serial output AND have debugging capability
- You're always one command away from debugging

### Interactive GDB Session (PRIMARY WORKFLOW)

Use `gdb_session.sh` for persistent, interactive debugging:

```bash
# Start a persistent GDB session (QEMU + GDB running in background)
./breenix-gdb-chat/scripts/gdb_session.sh start

# Send individual commands and make decisions based on results
./breenix-gdb-chat/scripts/gdb_session.sh cmd "break kernel::kernel_main"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"
# See the result, think about it, then decide next command...
./breenix-gdb-chat/scripts/gdb_session.sh cmd "info registers rip rsp"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "backtrace 5"

# Get serial output at any time
./breenix-gdb-chat/scripts/gdb_session.sh serial

# Stop when done
./breenix-gdb-chat/scripts/gdb_session.sh stop
```

### Auto-Run Mode (Let Kernel Boot, Intercept on Panic)

Even when you want to just "run the kernel", do it through GDB:

```bash
# Start session, continue without breakpoints - will run until panic or completion
./breenix-gdb-chat/scripts/gdb_session.sh start
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"
# If kernel panics, GDB catches it - examine state with:
./breenix-gdb-chat/scripts/gdb_session.sh cmd "backtrace 20"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "info registers"
```

### Build Only (No Execution)

```bash
cargo build --release --features testing,external_test_bins --bin qemu-uefi
```

### Prohibited Commands (NEVER USE)

These commands run the kernel without GDB and are **forbidden**:
```bash
# ❌ FORBIDDEN - runs kernel without GDB
cargo run --release --bin qemu-uefi
cargo run -p xtask -- boot-stages
./scripts/run_breenix.sh
cargo test
```

### Logs
All runs are logged to `logs/breenix_YYYYMMDD_HHMMSS.log`

```bash
# View latest log
ls -t logs/*.log | head -1 | xargs less
```

## Development Workflow

### Agent-Based Development (MANDATORY)

**The main conversation is for ORCHESTRATION ONLY.** Never execute tests, run builds, or perform iterative debugging directly in the top-level session. This burns token context and leads to session exhaustion.

**ALWAYS dispatch to agents:**
- Build verification and compilation checks
- GDB debugging sessions (via gdb_chat.py)
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
# DON'T run kernel directly - ALWAYS use GDB
cargo run --release --bin qemu-uefi
cargo run -p xtask -- boot-stages
# DON'T grep through large outputs in main session
cat target/output.txt | grep ...
```

**Correct pattern:**
```
# DO use GDB for kernel debugging
printf 'break kernel::kernel_main\ncontinue\ninfo registers\nquit\n' | python3 breenix-gdb-chat/scripts/gdb_chat.py

# DO dispatch to an agent for GDB sessions
Task(subagent_type="general-purpose", prompt="Use gdb_chat.py to debug the
clock_gettime syscall. Set breakpoint, examine registers, report findings.")
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

## 🚨 PROHIBITED CODE SECTIONS 🚨

The following files are on the **prohibited modifications list**. Agents MUST NOT modify these files without explicit user approval.

### Tier 1: Absolutely Forbidden (ask before ANY change)
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
| `kernel/src/gdt.rs` | GDT/TSS - rarely needs changes |
| `kernel/src/per_cpu.rs` | Per-CPU data - used in hot paths |

### When Modifying Prohibited Sections

If you believe you must modify a prohibited file:

1. **Explain why GDB debugging is insufficient** for this specific problem
2. **Get explicit user approval** before making any changes
3. **Never add logging** - use GDB breakpoints instead
4. **Remove any temporary debug code** before committing
5. **Test via GDB** to verify the fix works

### Detecting Violations

Look for these red flags in prohibited files:
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

## GDB-Only Kernel Debugging - MANDATORY

**ALL kernel execution and debugging MUST be done through GDB.** This is non-negotiable.

Running the kernel directly (`cargo run`, `cargo test`, `cargo run -p xtask -- boot-stages`) without GDB:
- Provides only serial output, which is insufficient for timing-sensitive bugs
- Cannot inspect register state, memory, or call stacks
- Cannot set breakpoints to catch issues before they cascade
- Cannot intercept panics to examine state
- Burns context analyzing log output instead of actual debugging

### Interactive GDB Session (PRIMARY WORKFLOW)

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

### Anti-Patterns (NEVER DO THIS)

```bash
# DON'T run kernel directly for debugging
cargo run -p xtask -- boot-stages
cargo run --release --bin qemu-uefi

# DON'T analyze log output to debug timing issues
cat target/xtask_boot_stages_output.txt | grep ...

# DON'T add logging to debug syscalls (causes timing bugs!)
log::debug!("clock_gettime called");

# DON'T script a batch of commands hoping they work
# This prevents learning and adapting based on what you see
```

### Correct Debugging Pattern (Interactive)

```bash
# DO start a persistent GDB session
./breenix-gdb-chat/scripts/gdb_session.sh start

# DO send commands ONE AT A TIME and THINK about the result
./breenix-gdb-chat/scripts/gdb_session.sh cmd "break kernel::syscall::time::sys_clock_gettime"
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"

# Examine the result - what did we learn?
# clock_id was 0x1 (CLOCK_MONOTONIC) - is that expected?
./breenix-gdb-chat/scripts/gdb_session.sh cmd "info registers rdi rsi"

# Based on what we learned, examine more state
./breenix-gdb-chat/scripts/gdb_session.sh cmd "x/2xg \$rsi"

# Continue and see what happens
./breenix-gdb-chat/scripts/gdb_session.sh cmd "continue"

# Check return value
./breenix-gdb-chat/scripts/gdb_session.sh cmd "info registers rax"

# Stop when done
./breenix-gdb-chat/scripts/gdb_session.sh stop
```

This is **conversational debugging** - each command informs the next decision.

## Work Tracking

We use Beads (bd) instead of Markdown for issue tracking. Run `bd quickstart` to get started.
