# ENOSYS Ring 3 Debug Handoff

## Your Task

Fix the kernel page fault that prevents userspace processes from executing. The ENOSYS test in CI is failing honestly - it needs actual userspace execution, not fake markers.

## Current State

The kernel crashes with a page fault when attempting to run userspace code:

```
PF @ 0xffffffff8a16252d Error: 0x0 (P=0, W=0, U=0, I=0)
```

Also seen:
```
PF @ 0x2 Error: 0x0 (P=0, W=0, U=0, I=0)
CS:RIP: 0x8:0x100000f1e4c
```

The fault address `0x2` is clearly a null pointer dereference (struct field offset). The RIP `0x100000f1e4c` is kernel code in the low-half mapping.

## What's Happening

1. Kernel boots and creates userspace processes (syscall_enosys, hello_time, etc.)
2. Kernel prints `KERNEL_POST_TESTS_COMPLETE` marker
3. Kernel enables interrupts
4. Timer fires, scheduler picks a userspace thread
5. Context switch begins - CR3 switches to process page table
6. **PAGE FAULT** - kernel crashes

## Key Files to Investigate

1. **`kernel/src/interrupts/context_switch.rs`** - CR3 switching logic
   - Look at `restore_userspace_thread_context()`
   - The CR3 switch code was previously disabled with `if false {}` and recently re-enabled
   - Check around line 569

2. **`kernel/src/memory/process_memory.rs`** - ProcessPageTable creation
   - Verify kernel mappings are correctly copied to process page tables
   - Check that PML4 entries 256-511 (kernel space) are properly set up

3. **`kernel/src/test_exec.rs`** - Test setup
   - `test_syscall_enosys()` creates the ENOSYS test process

## Likely Root Causes

1. **Missing kernel mappings in process page table**
   - After CR3 switch, kernel code/data may not be accessible
   - The kernel is at `0x100000...` (PML4[2]) not traditional high-half

2. **Bad function pointer after CR3 switch**
   - The faulting address `0xffffffff8a16252d` looks like corrupted memory
   - Could be reading through unmapped page table entries

3. **Stack not accessible after CR3 switch**
   - Kernel stack at `0xffffc90000...` must be mapped in process page table

## How to Debug

### Run locally
```bash
cargo run -p xtask -- ring3-enosys
cat target/xtask_ring3_enosys_output.txt
```

### Add debug logging
In `context_switch.rs`, add logging before/after CR3 switch:
- Print current RSP and verify it's in kernel stack region
- Print CR3 values (old and new)
- Verify kernel code is readable after switch
- Check the address that faults

### Check page table contents
In `process_memory.rs`, log what's being set up:
- Which PML4 entries are copied from kernel
- Verify PML4[2] (kernel code) is present
- Verify PML4[402] (kernel stacks) is present

## Success Criteria

The ENOSYS test passes when you see in the output:
```
ENOSYS OK
```

This means:
1. Userspace executed in Ring 3
2. syscall 999 was invoked
3. Kernel returned -38 (ENOSYS)
4. Userspace printed the result

## DO NOT

- Add fallbacks to xtask that accept weaker evidence
- Change test criteria to match broken behavior
- Accept "process created" as proof of "process executed"
- Let the test pass without actual userspace output

See `CLAUDE.md` section "Testing Integrity - CRITICAL" for why this matters.

## Recent Context

- CR3 switching was re-enabled in `context_switch.rs` (was disabled with `if false {}`)
- The CR3 fix verification doc at `docs/planning/CR3_FIX_VERIFICATION.md` claims success but the test still fails
- The smoke test passes only because markers are printed before userspace runs

## Commands to Get Started

```bash
# See the failing test output
cat target/xtask_ring3_enosys_output.txt | grep -A5 "PAGE FAULT"

# Find the CR3 switch code
grep -n "Switching CR3" kernel/src/interrupts/context_switch.rs

# Find process page table creation
grep -n "ProcessPageTable::new" kernel/src/memory/process_memory.rs

# Run with visual output
BREENIX_VISUAL_TEST=1 cargo run -p xtask -- ring3-enosys
```

Good luck. Fix it properly.
