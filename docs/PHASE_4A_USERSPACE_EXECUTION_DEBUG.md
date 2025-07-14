# Phase 4A Userspace Execution Debug - Detailed Analysis

**Date**: 2025-01-14
**Issue**: Userspace process hangs after context switch despite kernel stack fix
**Current Status**: INT3 test shows NO userspace execution occurring

## Executive Summary

After fixing the kernel stack mapping issue, we now face a different problem: the userspace process never executes even a single instruction. The process hangs at the exact moment of CR3 switch during the interrupt return path. This document details the systematic debugging process and findings.

## Table of Contents

1. [Verification Steps Completed](#verification-steps-completed)
2. [Critical Findings](#critical-findings)
3. [Assembly-Level Analysis](#assembly-level-analysis)
4. [Root Cause Hypothesis](#root-cause-hypothesis)
5. [Proposed Solutions](#proposed-solutions)
6. [Next Steps](#next-steps)

## Verification Steps Completed

### Step 1: Binary Verification ✅

**Objective**: Confirm correct instructions are in the ELF binary

**Original INT 0x80 version**:
```
10000000: b8 34 12 00 00     movl   $0x1234, %eax
10000005: cd 80              int    $0x80
10000007: 31 c0              xorl   %eax, %eax
10000009: 31 ff              xorl   %edi, %edi
1000000b: cd 80              int    $0x80
1000000d: eb fe              jmp    0x1000000d
```

**Modified INT3 test version**:
```
10000000: 50                 pushq  %rax
10000001: b8 34 12 00 00     movl   $0x1234, %eax
10000006: cc                 int3
10000007: f4                 hlt
```

**Conclusion**: Binary contains correct instructions at expected address (0x10000000)

### Step 2: CPU Execution Test ❌

**Objective**: Confirm CPU fetches and executes first instruction

**Test**: Replaced INT 0x80 with INT3 to trigger breakpoint handler

**Expected**: 
```
BREAKPOINT from USERSPACE at 0x10000006
```

**Actual**: No breakpoint handler invocation

**Conclusion**: CPU never executes ANY userspace instruction

## Critical Findings

### 1. Context Switch Logs Show Correct Setup

```
SCHED_SWITCH: 0 -> 1
ARC ACCESS id=1 state=Running
TF‑TRACE: armed (thread 1 → RIP=0x10000000)
IRET STACK: RIP=0x10000000 CS=0x33 RFLAGS=0x300 RSP=0x555555561000 SS=0x2b
USER-MAP-DEBUG: About to check RSP=0x555555561000, RIP=0x10000000
USER-MAP: RIP=0x10000000 Some(PhysAddr(0x64c000)), RSP=0x555555561000 Some(PhysAddr(0x663000))
USER-MAP: RSP-8=0x555555560ff8 Some(PhysAddr(0x662ff8))
USER-MAP: KSTACK=0x100000100a0 Some(PhysAddr(0x12d0a0))
USER-RIP: About to return to userspace at RIP=0x10000000
ASM_CR3_SWITCH: Current CR3=0x101000, switching to CR3=0x64b000
S= A
[HANG]
```

### 2. Key Observations

1. **Kernel stack is mapped**: `KSTACK=0x100000100a0 Some(PhysAddr(0x12d0a0))` ✅
2. **User pages are mapped**: Both RIP and RSP resolve to physical addresses ✅
3. **Segments are correct**: CS=0x33 (user code), SS=0x2b (user data) ✅
4. **RFLAGS=0x300**: IF flag is set (interrupts enabled) ✅
5. **Hang occurs at "S= A"**: This is output JUST BEFORE the CR3 switch

### 3. Previous Successful Runs

In earlier test runs (with the kernel stack fix), we saw:
```
TIMER_TICK: tid=1 ticks=1 quantum=5 privilege=User
```

This proves that userspace execution CAN work, but something is preventing it in current runs.

## Assembly-Level Analysis

### Timer Interrupt Return Path

From `kernel/src/interrupts/timer_entry.asm`:

```asm
; Line 246-247: Output 'A' before CR3 switch
mov al, 'A'
out dx, al

; Line 257: THE CRITICAL INSTRUCTION
mov cr3, rdx               ; Switch page table using RDX

; Line 263-264: Output 'B' after CR3 switch
mov al, 'B'
out dx, al
```

**Key Finding**: We see "A" but never "B", indicating the hang occurs at or immediately after `mov cr3, rdx`

### SWAPGS Consideration

The code includes:
```asm
; Line 339: When returning to Ring 3
swapgs                     ; restore user GS base
```

This happens AFTER the CR3 switch, so it's not the immediate cause.

## Root Cause Hypothesis

### Primary Theory: Page Table Corruption or Missing Mapping

The hang at CR3 switch suggests one of:

1. **The new CR3 value is invalid**
   - But we log it: `switching to CR3=0x64b000` which looks reasonable

2. **Critical kernel mappings missing in process page table**
   - We fixed the kernel stack (PML4 entry 2)
   - But might be missing other critical mappings

3. **The instruction immediately after CR3 switch is inaccessible**
   - The next instruction tries to output 'B' to serial port
   - This requires kernel code to be mapped

### Secondary Theory: GS Base Issue

The SWAPGS instruction assumes GS base is set up properly. If GS.base points to unmapped memory, the first instruction after SWAPGS that uses GS would fault.

## Proposed Solutions

### Solution 1: Verify ALL Critical Mappings

Add debug code to verify these are mapped in process page table:
```rust
// In ProcessPageTable::new()
1. Kernel code (where timer_entry.asm lives)
2. Serial port I/O region (if memory-mapped)
3. IDT and GDT
4. Interrupt handler code
```

### Solution 2: Clear FS/GS Base

Add to context setup:
```rust
// Before returning to userspace
unsafe {
    // Clear FS/GS base to prevent stray references
    x86_64::registers::msr::FsBase::write(VirtAddr::new(0));
    x86_64::registers::msr::GsBase::write(VirtAddr::new(0));
}
```

### Solution 3: Add More Granular Debug Output

Instead of serial output after CR3 switch, use simpler I/O:
```asm
mov cr3, rdx
; Don't use serial port, just magic I/O
mov al, 0xB0
out 0x80, al    ; This should work even with limited mappings
```

### Solution 4: Test Without SWAPGS

Temporarily comment out SWAPGS to see if that's the issue:
```asm
; swapgs    ; TEMPORARILY DISABLED FOR TESTING
```

## Next Steps

### Immediate Actions

1. **Add kernel code mapping verification**
   ```rust
   // Log which PML4 entries contain kernel code
   let kernel_code_addr = VirtAddr::new(0x200000); // From bootloader
   let entry_index = (kernel_code_addr.as_u64() >> 39) & 0x1FF;
   log::info!("Kernel code is in PML4 entry {}", entry_index);
   ```

2. **Verify timer_entry.asm is accessible**
   The code at the return path must be mapped in the new page table

3. **Try minimal I/O after CR3 switch**
   Replace serial output with simple out 0x80

4. **Check if issue is SWAPGS-related**
   Temporarily disable SWAPGS

### Debugging Priority

1. **High**: Determine exact instruction that hangs
2. **High**: Verify kernel code is mapped in process page table  
3. **Medium**: Clear FS/GS base registers
4. **Low**: Add comprehensive mapping validation

## Conclusion

The userspace execution hang occurs at the exact moment of CR3 switch in the interrupt return path. Despite having fixed the kernel stack mapping, we're missing another critical mapping that prevents even the next instruction after CR3 switch from executing. The most likely cause is that kernel code itself (specifically the interrupt return path) is not properly mapped in the process page table.

This is distinct from the previous kernel stack issue and requires ensuring ALL kernel code/data needed during interrupt handling is mapped in every process page table.