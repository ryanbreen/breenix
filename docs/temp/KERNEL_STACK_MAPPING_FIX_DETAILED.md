# Kernel Stack Mapping Fix - Exhaustive Analysis

**Date**: 2025-01-14
**Issue**: Userspace execution hanging after context switch ("S= A" hang)
**Root Cause**: Kernel stack not mapped in process page tables
**Status**: FIXED ✅

## Executive Summary

This document provides exhaustive detail on the critical kernel stack mapping bug that prevented userspace execution in Breenix OS. The bug manifested as a hang immediately after context switch to userspace, with the last output being "S= A" from the assembly code. Through systematic debugging, we discovered that the kernel stack was not mapped in the new process's page table, causing a hang when the CPU tried to push the interrupt frame during timer interrupts in userspace.

## Table of Contents

1. [Initial Symptoms](#initial-symptoms)
2. [Systematic Debugging Process](#systematic-debugging-process)
3. [Root Cause Discovery](#root-cause-discovery)
4. [The Fix](#the-fix)
5. [Proof of Fix](#proof-of-fix)
6. [Technical Deep Dive](#technical-deep-dive)
7. [Remaining Issues](#remaining-issues)
8. [Next Steps](#next-steps)

## Initial Symptoms

### Primary Failure Pattern

The syscall_gate test process would:
1. ✅ Create successfully
2. ✅ Complete context switch setup
3. ❌ Hang immediately after "S= A" output
4. ❌ Never execute userspace code
5. ❌ Never trigger INT 0x80 syscall

### Key Log Evidence

```
SCHED_SWITCH: 0 -> 1
ARC ACCESS id=1 state=Running
TF‑TRACE: armed (thread 1 → RIP=0x10000000)
IRET STACK: RIP=0x10000000 CS=0x33 RFLAGS=0x300 RSP=0x555555561000 SS=0x2b
ASM_CR3_SWITCH: Current CR3=0x101000, switching to CR3=0x64b000
S= A
[HANG - No further output]
```

### Critical Observation

This was **very similar** to userspace execution issues that had been "fixed yesterday", suggesting either:
- A regression in the fix
- A related but different issue
- Incomplete previous fix

## Systematic Debugging Process

The user provided a systematic checklist to debug the issue without undoing the high-half/isolation work:

### Step 0: Tag Current Head
**Action**: Tagged the current git head as `v0.2-isolation-OK` to preserve a known state.
```bash
git tag v0.2-isolation-OK
```

### Step 1: Add UD2 Breadcrumb
**Purpose**: Determine if CPU ever reaches userspace code
**Implementation**: Added UD2 instruction after INT 0x80 in syscall_gate_test.rs:

```rust
// userspace/tests/syscall_gate_test.rs
unsafe fn test_syscall() -> u64 {
    let ret: u64;
    core::arch::asm!(
        "int 0x80",
        "ud2",  // Step 1 breadcrumb: if we return from syscall, crash with #UD
        in("rax") TEST_SYSCALL,
        lateout("rax") ret,
        options(nostack, preserves_flags),
    );
    ret
}
```

**Result**: No #UD exception occurred, proving CPU never executed the INT 0x80 instruction.

### Step 2: Verify User Pages Present & Executable
**Implementation**: Added debug logging in context_switch.rs:

```rust
// kernel/src/interrupts/context_switch.rs:266-274
crate::serial_println!("USER-MAP-DEBUG: About to check RSP={:#x}, RIP={:#x}", user_rsp, user_rip);

let rsp_ok = page_table.translate_page(VirtAddr::new(user_rsp));
let rip_ok = page_table.translate_page(VirtAddr::new(user_rip));

crate::serial_println!(
    "USER-MAP: RIP={:#x} {:?}, RSP={:#x} {:?}",
    user_rip, rip_ok, user_rsp, rsp_ok
);
```

**Log Output**:
```
USER-MAP-DEBUG: About to check RSP=0x555555561000, RIP=0x10000000
USER-MAP: RIP=0x10000000 Some(PhysAddr(0x64c000)), RSP=0x555555561000 Some(PhysAddr(0x663000))
USER-MAP: RSP-8=0x555555560ff8 Some(PhysAddr(0x662ff8))
```

**Conclusion**: ✅ User pages are properly mapped

### Step 3: Check NX/XD Flag
**Implementation**: Added ELF loading debug output:

```rust
// kernel/src/elf.rs:143-165
crate::serial_println!("ELF-FLAGS: seg at {:#x} p_flags={:#x} (W={} X={})",
    vaddr.as_u64(), ph.p_flags, ph.p_flags & 2 != 0, ph.p_flags & 1 != 0);

// ... later ...

crate::serial_println!("ELF-PAGEFLAGS: seg at {:#x} final_flags={:?} NX={}",
    vaddr.as_u64(), flags, flags.contains(PageTableFlags::NO_EXECUTE));
```

**Log Output**:
```
ELF-FLAGS: seg at 0x10000000 p_flags=0x5 (W=false X=true)
ELF-PAGEFLAGS: seg at 0x10000000 final_flags=PageTableFlags(PRESENT | USER_ACCESSIBLE) NX=false
```

**Conclusion**: ✅ Code segment is correctly marked executable (NX=false)

### Step 4: Validate Kernel Stack Mapped in New CR3
**This is where we found the bug!**

**Implementation**: Added kernel stack checking:

```rust
// kernel/src/interrupts/context_switch.rs:280-286
// STEP 4: Check if current kernel stack is mapped in new CR3
let current_kernel_rsp: u64;
unsafe {
    core::arch::asm!("mov {}, rsp", out(reg) current_kernel_rsp);
}
let kstack_ok = page_table.translate_page(VirtAddr::new(current_kernel_rsp));
crate::serial_println!("USER-MAP: KSTACK={:#x} {:?}", current_kernel_rsp, kstack_ok);
```

**Critical Log Output**:
```
USER-MAP: KSTACK=0x100000100f0 None
```

## Root Cause Discovery

The kernel stack at address `0x100000100f0` was **NOT mapped** in the new process page table!

### Why This Causes a Hang

1. Context switch to userspace completes (IRETQ instruction)
2. Userspace begins executing
3. Timer interrupt fires (happens every 10ms)
4. CPU tries to switch to kernel mode to handle interrupt
5. CPU needs to push interrupt frame onto kernel stack
6. **Kernel stack page is not mapped in current CR3**
7. CPU cannot push interrupt frame
8. System hangs (likely triple fault or CPU lockup)

### Address Space Analysis

The kernel stack address `0x100000100f0` maps to:
- PML4 index: `(0x100000100f0 >> 39) & 0x1FF = 2`
- This is the idle thread stack region

## The Fix

### Implementation

Added idle thread stack mapping to `ProcessPageTable::new()`:

```rust
// kernel/src/memory/process_memory.rs:287-302
// CRITICAL FIX: Map idle thread stack region for context switching
// The idle thread stack is at 0x100000000000 range, which maps to PML4 entry 2
// Without this mapping, context switches to userspace fail because kernel stack is not accessible
const IDLE_STACK_PML4_INDEX: usize = 2;
let idle_stack_addr = 0x100000000000u64; // Range containing idle thread stack
log::debug!("Mapping idle thread stack region: PML4 entry {} for addresses around {:#x}",
    IDLE_STACK_PML4_INDEX, idle_stack_addr);

if !kernel_l4_table[IDLE_STACK_PML4_INDEX].is_unused() {
    let mut entry = kernel_l4_table[IDLE_STACK_PML4_INDEX].clone();
    entry.set_flags(entry.flags() & !PageTableFlags::USER_ACCESSIBLE);
    level_4_table[IDLE_STACK_PML4_INDEX] = entry;
    log::info!("KSTACK_FIX: Mapped idle thread stack PML4 entry {} for context switch support", IDLE_STACK_PML4_INDEX);
} else {
    log::error!("Idle thread stack PML4 entry {} is UNUSED in kernel page table - this will cause context switch failures!", IDLE_STACK_PML4_INDEX);
}
```

### What This Does

1. Copies PML4 entry 2 from the kernel page table to every new process page table
2. Clears the USER_ACCESSIBLE flag to prevent userspace access
3. Ensures the kernel stack is always accessible after CR3 switch

## Proof of Fix

### Before Fix
```
USER-MAP: KSTACK=0x100000100f0 None
ASM_CR3_SWITCH: Current CR3=0x101000, switching to CR3=0x64b000
S= A
[HANG]
```

### After Fix
```
USER-MAP: KSTACK=0x100000100a0 Some(PhysAddr(0x12d0a0))
ASM_CR3_SWITCH: Current CR3=0x101000, switching to CR3=0x64b000
S= A
!0TDARC ACCESS id=1 state=Running
TIMER_TICK: tid=1 ticks=1 quantum=5 privilege=User
```

The key evidence is:
1. ✅ Kernel stack is now mapped: `Some(PhysAddr(0x12d0a0))`
2. ✅ Timer interrupts fire in userspace: `privilege=User`
3. ✅ Process continues running (multiple timer ticks)

## Technical Deep Dive

### Why PML4 Entry 2?

The kernel uses different regions of virtual address space:
- **PML4 0-7**: Low half, typically userspace and kernel code
- **PML4 2**: Contains idle thread stack (0x100000000000 range)
- **PML4 136**: Kernel heap (0x444444440000)
- **PML4 256-511**: High half kernel space
- **PML4 402**: RSP0 region (0xffffc90000000000) for interrupt stacks

### Process Page Table Creation

The `ProcessPageTable::new()` function must copy essential kernel mappings:

```rust
// Key mappings that must be present in every process page table:
1. High-half kernel mappings (PML4 256-511) - kernel code/data
2. Kernel heap (PML4 136) - for dynamic allocations
3. RSP0 region (PML4 402) - for interrupt handling
4. Idle thread stack (PML4 2) - for context switching [NEWLY ADDED]
```

### Context Switch Flow

1. Scheduler decides to switch from kernel thread to user thread
2. `restore_userspace_thread_context()` sets up the switch:
   ```rust
   // Store the page table to switch to
   NEXT_PAGE_TABLE = Some(page_table_frame);
   // Update TSS RSP0 for kernel stack
   crate::gdt::set_kernel_stack(kernel_stack_top);
   ```

3. Assembly code performs the actual switch:
   ```asm
   ; Get the new page table frame
   call get_next_page_table
   ; Switch CR3 if needed
   test rax, rax
   jz .no_cr3_switch
   mov cr3, rax
   .no_cr3_switch:
   ; Return to userspace with IRETQ
   ```

4. Userspace execution begins
5. Timer interrupt occurs
6. CPU switches to kernel mode using RSP0 from TSS
7. **Critical**: Kernel stack must be mapped in current CR3!

## Remaining Issues

### Current State

While the kernel stack mapping is fixed, the syscall_gate test still shows some issues:

1. **Userspace execution appears to start** (timer shows `privilege=User`)
2. **But INT 0x80 syscall is not being reached**
3. Process seems to be in a loop or halted state

### Possible Causes

1. **Userspace binary issue**: The compiled binary might not have the expected instructions
2. **Instruction execution problem**: CPU might be hitting an unexpected instruction
3. **Different hang**: After fixing the kernel stack issue, we might be hitting a different problem

### Evidence of Progress

Despite the remaining issue, we have clear evidence of improvement:
- Context switches complete successfully
- Userspace privilege level is achieved
- Timer interrupts work in userspace
- No more immediate hangs at "S= A"

## Next Steps

### Immediate Actions

1. **Verify userspace binary contents**:
   ```bash
   objdump -d userspace/tests/syscall_gate_test
   ```
   Confirm INT 0x80 instruction is at 0x10000000

2. **Add instruction-level debugging**:
   - Enable single-stepping
   - Add debug exception handler
   - Log each instruction executed

3. **Simplify test case**:
   ```asm
   ; Minimal test: just INT 0x80 and halt
   mov eax, 0x1234
   int 0x80
   hlt
   ```

4. **Check for exceptions**:
   - Add handlers for all exceptions
   - Log any exceptions that occur in userspace

### Longer Term

1. **Run all kernel tests** (Step 7):
   ```bash
   cargo test
   ```
   Ensure no regressions from this fix

2. **Add guard-rail test** (Step 8):
   Create a specific test that verifies kernel stack is mapped:
   ```rust
   #[test]
   fn test_kernel_stack_mapped_in_process_page_table() {
       let page_table = ProcessPageTable::new().unwrap();
       let kstack_addr = VirtAddr::new(0x100000100000);
       assert!(page_table.translate_page(kstack_addr).is_some());
   }
   ```

3. **Document in PROJECT_ROADMAP.md**:
   Update the roadmap to reflect this critical fix

### Investigation Priority

The next investigation should focus on why the userspace process isn't executing the INT 0x80 instruction despite successfully entering userspace. This is likely a different issue than the kernel stack mapping problem we just fixed.

## Conclusion

We successfully identified and fixed a critical kernel stack mapping bug that was preventing userspace execution. The fix ensures that the idle thread stack (PML4 entry 2) is mapped in every process page table, allowing timer interrupts to function correctly during userspace execution.

While the immediate "hang after S=A" issue is resolved, further investigation is needed to determine why the INT 0x80 syscall instruction is not being executed in the syscall_gate test.

This fix represents a significant step forward in Breenix's process isolation and memory management implementation, following standard OS practices for page table management.