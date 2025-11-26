# Stage 28 Debugging Handoff

## Session Context

**Date**: 2025-11-25
**Previous session**: Fixed triple fault caused by unmapped memory regions
**Current failure**: Stage 28 (clock_gettime tests) times out

## What Was Fixed Last Session

### Root Cause: 3 Unmapped Memory Regions

The kernel was triple-faulting immediately after IRETQ to Ring 3 because:

| Address | What | Problem | Fix |
|---------|------|---------|-----|
| `0xffffc90000033000` | TSS.RSP0 (kernel stack) | PT[51] empty | Stop copying bootloader PDPTs |
| `0xffffc98000004000` | IST[1] (page fault stack) | PT[4] empty | Fresh PDPTs for PML4[402]/[403] |
| `0x7fffff011008` | User RSP | PT[17] empty | Use stack_top-16 for RSP |

### Fixes Applied

1. **kernel/src/memory/kernel_page_table.rs**: Removed bootloader PDPT copying for PML4[402] and [403]
2. **kernel/src/process/manager.rs**: Fixed user RSP to point within mapped region
3. **kernel/src/interrupts/context_switch.rs**: Added pre-IRETQ validation that halts on unmapped addresses

### Validation Added

Before every first IRETQ to userspace, the kernel now:
- Walks page tables for TSS.RSP0, IST[1], and User RSP
- Halts with clear error if any are unmapped
- Emits `[CHECKPOINT:PAGETABLE_VALIDATED]` on success

## Current State

```
Boot Stage Validator - 33 stages to check
=========================================
[1/33] Kernel entry point... PASS
...
[27/33] Interrupts enabled... PASS
[28/33] Clock gettime tests passed... FAIL (timeout after 30s)
```

**Stages 1-27**: All pass (including the new page table validation)
**Stage 28**: Times out waiting for `"clock_gettime tests passed"`

## What We Know

1. **Page table mappings are correct** - validation passes
2. **IRETQ to Ring 3 succeeds** - we see `"Returning to Ring 3 (CPL=3) with IF=1"`
3. **Timer IRQs were stopping after IRETQ** - this was the triple fault symptom
4. **Timer IRQs should work now** - page tables are fixed

## What We Don't Know Yet

1. Is userspace actually executing code?
2. Are timer IRQs firing after IRETQ now?
3. Is userspace attempting syscalls?
4. Is the clock_gettime test code being reached?

## Files of Interest

- `kernel/src/main.rs:655-657` - Where clock_gettime test should run
- `kernel/src/syscall/handlers.rs` - Syscall implementations
- `kernel/src/interrupts/timer.rs` - Timer interrupt handler
- `kernel/src/interrupts/context_switch.rs` - IRETQ and context switch code
- `xtask/src/main.rs:211-215` - Stage 28 definition

## Recommended Debugging Approach

Use the same systematic 3-step approach that worked for the page table issue:

### Step 1: Verify Timer IRQs Continue After IRETQ

Add diagnostic to timer handler:
```rust
static TIMER_COUNT_POST_IRETQ: AtomicU64 = AtomicU64::new(0);
// Log every 50th timer to confirm they're firing in userspace context
```

**Expected**: We should now see timer IRQs after IRETQ (page tables are fixed)

### Step 2: Verify Userspace Is Executing

Check if we see any evidence of userspace code running:
- Syscall handler entries
- `USERSPACE OUTPUT:` messages
- Any output from hello_time.elf

### Step 3: Trace Clock Gettime Execution Path

The clock_gettime test is at `kernel/src/main.rs:655`:
```rust
log::info!("Testing clock_gettime syscall implementation...");
clock_gettime_test::test_clock_gettime();
log::info!("clock_gettime tests passed");  // <-- Stage 28 marker
```

Add diagnostics to see if this code is even being reached.

## Test Commands

```bash
# Run boot-stages test
cargo run -p xtask -- boot-stages

# Check output for diagnostics
grep -E "\[DIAG:" target/xtask_boot_stages_output.txt

# Check last 50 lines of output
tail -50 target/xtask_boot_stages_output.txt

# Look for timer IRQ activity
grep -E "Timer|timer|IRQ" target/xtask_boot_stages_output.txt | tail -20
```

## Important Notes

1. **Use agents for running tests** - per CLAUDE.md, the main session is for orchestration only
2. **Add validation tests** for any fixes to prevent regression
3. **Be methodical** - don't jump to conclusions, add one diagnostic at a time
4. **Stage 28 requires kernel code to run** - it's NOT a userspace test, so the issue may be that kernel execution never resumes after IRETQ

## Hypothesis

The most likely issue: After fixing page tables, we now successfully IRETQ to userspace, but the kernel scheduler/timer path back to kernel code may have issues. The `clock_gettime_test::test_clock_gettime()` is kernel code that should run AFTER scheduler initialization, but if the scheduler never returns control to the kernel's main initialization path, we'd see this exact symptom.

Check if `scheduler::schedule()` returns control to continue kernel initialization, or if it permanently switches to userspace.
