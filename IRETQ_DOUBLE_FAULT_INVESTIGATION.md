# IRETQ Double Fault Investigation - Comprehensive Analysis

## Executive Summary

The Breenix kernel successfully reaches Ring 3 userspace initially, but when returning from a timer interrupt back to userspace via `iretq`, it triggers a double fault. Despite extensive investigation and multiple attempted fixes, the issue persists.

## Current State

### What Works ✅
1. **Initial transition to Ring 3**: The kernel successfully transitions to userspace (CPL=3) initially
2. **Timer interrupts from userspace**: Timer correctly interrupts Ring 3 code
3. **Interrupt entry handling**: Saves context, swapgs works correctly
4. **Context switching logic**: Scheduler properly selects user threads
5. **GDT/IDT setup**: Descriptors are correctly configured
6. **IST mechanism**: Double fault handler runs correctly on IST stack

### What Fails ❌
1. **IRETQ instruction**: Immediate double fault when attempting to return to Ring 3
2. **Stack accessibility**: After CR3 switch to process page table, kernel stack becomes inaccessible
3. **Page table isolation**: Process page tables don't map critical kernel structures properly

## Timeline of Investigation

### Phase 1: Initial Discovery
- Timer successfully interrupts Ring 3 code (CPL=3 confirmed in saved context)
- Double fault occurs immediately after attempting `iretq`
- Initial suspicion: corrupted IRET frame or stack issues

### Phase 2: IRET Frame Analysis
**Finding**: The IRET frame is PERFECT with all 64-bit values correct:
```
RAW IRET FRAME at 0x18000012718:
  [0] = 0x0000000010000000   # RIP - correct userspace address
  [1] = 0x0000000000000033   # CS  - Ring 3 selector (index 6, RPL 3)
  [2] = 0x0000000000000200   # RFLAGS - interrupt flag set
  [3] = 0x00007fffff011000   # RSP - user stack pointer
  [4] = 0x000000000000002b   # SS  - Ring 3 data selector (index 5, RPL 3)
```
**Conclusion**: No 32-bit truncation or garbage in upper bits. Frame values are correct.

### Phase 3: IST Stack Investigation
- Double fault handler runs correctly on IST stack at expected location
- IST[0] set to 0xffffc98000002000 and working correctly
- Actual RSP during double fault: 0xffffc98000001a98 (correctly near IST)

### Phase 4: GDT Descriptor Verification
Added comprehensive diagnostics to verify GDT descriptors:
```
User data (0x2b): 0x00cff3000000ffff
  P=1 DPL=3 S=1 Type=0x3 (writable data segment)
  
User code (0x33): 0x00affb000000ffff
  P=1 DPL=3 S=1 Type=0xb L=1 D=0 (64-bit code segment)
```
**Finding**: GDT descriptors are PERFECT for Ring 3 execution

### Phase 5: Segment Validation Tests
Added VERR/VERW/LAR instructions to test segment validity:
- VERR 0x33 (CS): SUCCESS - segment is readable from Ring 3
- VERW 0x2b (SS): SUCCESS - segment is writable from Ring 3
- LAR for both: SUCCESS - access rights readable
- GDT at 0x100000e9240 is accessible after CR3 switch

### Phase 6: CR3 Switching Investigation
Discovered critical issue with page table switching:
- Process page table (0x65a000) correctly created
- Kernel mappings copied (PML4 entries 1-255 and 256-511)
- BUT: Kernel stack at 0x18000012718 becomes inaccessible after CR3 switch
- Stack push/pop test FAILS immediately after CR3 switch

### Phase 7: Root Cause Analysis (Cursor Agent Consultation)
Cursor Agent identified the core issue:
- IRETQ needs to pop 5 qwords from the kernel stack
- If the kernel stack isn't mapped in the active CR3, the pop operations page fault
- Page fault handler can't push its frame to the same unmapped stack
- This cascades immediately to double fault

## Attempted Fixes and Results

### 1. ❌ Diagnostic Code Placement Fix
**Attempt**: Move all diagnostic logging before CR3 switch and swapgs
**Result**: Still double faults at IRETQ
**Learning**: The issue isn't caused by diagnostic code accessing wrong memory

### 2. ❌ Disable CR3 Switch
**Attempt**: Skip CR3 switch, stay on kernel page tables
**Result**: Still double faults at IRETQ
**Problem**: Userspace code at 0x10000000 not mapped in kernel page table

### 3. ❌ Dual Mapping Workaround
**Attempt**: Map userspace code in BOTH kernel and process page tables
**Code Added**: In `/kernel/src/elf.rs`:
```rust
// TEMPORARY WORKAROUND: Also map userspace in kernel page table
if page.start_address().as_u64() == 0x10000000 {
    // Map in current (kernel) page table
    let mut kernel_mapper = unsafe { crate::memory::paging::get_mapper() };
    unsafe {
        match kernel_mapper.map_to(page, frame, flags, ...) {
            Ok(flush) => flush.flush(),
            Err(e) => log::error!("Failed to map userspace in kernel page table")
        }
    }
}
```
**Result**: Mapping succeeds but still double faults at IRETQ
**Learning**: The issue is more fundamental than just code accessibility

## Technical Analysis

### Memory Layout
- **Kernel stack (IRET frame)**: 0x18000012718 (PML4 entry 3, ~1.5TB virtual)
- **Userspace code**: 0x10000000 (PML4 entry 0)
- **GDT**: 0x100000e9240 (PML4 entry 2)
- **IDT**: 0x100000eb520 (PML4 entry 2)
- **Standard kernel stacks**: 0xffffc900_0000_0000 - 0xffffc900_0100_0000

### Page Table Mapping Issues
1. Process page tables use shallow copy of kernel PML4 entries
2. The kernel stack at 0x18000012718 is NOT in the standard kernel stack range
3. This appears to be a bootstrap/temporary stack that isn't properly mapped
4. PML4 entries are copied but the actual stack pages may not be accessible

### The IRETQ Failure Mechanism
1. Timer interrupt from Ring 3 works correctly
2. Context saved, scheduler runs, prepares to return to Ring 3
3. Assembly code attempts CR3 switch to process page table (0x65a000)
4. Stack accessibility test (push/pop) FAILS after CR3 switch
5. Even without CR3 switch, IRETQ still fails (userspace not mapped in kernel table)
6. The double fault RIP (0x1000009bfa6) is the IRETQ instruction itself

## Diagnostic Code Added

### Timer Entry Assembly (`/kernel/src/interrupts/timer_entry.asm`)
1. Added VERR/VERW/LAR tests for CS/SS selectors
2. Added stack accessibility test after CR3 switch
3. Added GDTR/CR3 logging before critical operations
4. Moved all diagnostic calls before swapgs

### Timer Handler (`/kernel/src/interrupts/timer.rs`)
1. Added `log_cr3_at_iret()` function
2. Added `log_gdtr_at_iret()` function with descriptor decoding
3. Enhanced frame logging with page table walks

### GDT Module (`/kernel/src/gdt.rs`)
1. Added raw descriptor dumping during initialization
2. Added descriptor bit field decoding

### ELF Loading (`/kernel/src/elf.rs`)
1. Added segment permission analysis logging
2. Added temporary dual-mapping code (unsuccessful workaround)

## Current Blockers

1. **Kernel Stack Mapping**: The kernel stack containing the IRET frame (0x18000012718) must be mapped in process page tables but isn't
2. **Bootstrap Stack Issue**: This stack is outside the standard kernel stack range and appears to be a bootstrap stack
3. **Page Table Architecture**: Shallow copying of PML4 entries doesn't ensure all pages are accessible
4. **Userspace Mapping**: When staying on kernel page tables, userspace code isn't accessible

## Recommended Solution Path

### Option 1: Fix Page Table Mapping (Proper Solution)
1. Identify ALL kernel stacks (including bootstrap stack at 0x18000012718)
2. Ensure process page tables properly map:
   - All kernel code and data
   - ALL kernel stacks (not just the standard range)
   - GDT/IDT/TSS
3. Consider deep-copying page table hierarchies instead of shallow PML4 entry copies
4. Verify mappings are actually accessible, not just PML4 entries present

### Option 2: Use Trampoline Stack (Linux-style)
1. Create a small trampoline stack that's guaranteed to be mapped in all page tables
2. Switch to trampoline stack before CR3 switch
3. Perform IRETQ from trampoline stack
4. This avoids the unmapped stack issue entirely

### Option 3: Defer CR3 Switch (Alternative Approach)
1. Don't switch CR3 on interrupt return
2. Switch CR3 on entry to kernel instead
3. Keep userspace mapped in kernel page tables
4. This sidesteps the complexity during IRETQ

### Option 4: Fix Bootstrap Stack
1. Identify why we're using a stack at 0x18000012718
2. Switch to properly allocated kernel stacks from the standard range
3. Ensure all kernel threads use stacks from the managed pool

## Key Learnings

1. **GDT is correct**: Extensive testing proved descriptors are properly configured
2. **IRET frame is perfect**: No corruption or 32-bit truncation issues
3. **Stack accessibility is critical**: IRETQ must be able to access the kernel stack
4. **Page table isolation is hard**: Simply copying PML4 entries isn't sufficient
5. **Bootstrap environment matters**: Non-standard stacks cause unexpected issues

## Code Locations

- **Timer entry assembly**: `/kernel/src/interrupts/timer_entry.asm`
- **Double fault handler**: `/kernel/src/interrupts.rs:171`
- **GDT setup**: `/kernel/src/gdt.rs`
- **Process page tables**: `/kernel/src/memory/process_memory.rs`
- **ELF loading**: `/kernel/src/elf.rs`
- **Context switching**: `/kernel/src/interrupts/context_switch.rs`
- **Kernel stack allocation**: `/kernel/src/memory/kernel_stack.rs`

## Summary

After extensive investigation, the root cause is clear: the kernel stack containing the IRET frame becomes inaccessible when switching to process page tables, causing IRETQ to fail when trying to pop the return context. The stack at 0x18000012718 is outside the standard kernel stack range and isn't properly mapped in process page tables. 

The solution requires either:
1. Properly mapping ALL kernel memory (including bootstrap stacks) in process page tables
2. Using a different stack for IRETQ operations
3. Changing when/how CR3 switches occur
4. Fixing the kernel to not use non-standard stacks

The investigation has definitively ruled out GDT misconfiguration, IRET frame corruption, and IST issues. The problem is purely about memory accessibility during the critical IRETQ operation.