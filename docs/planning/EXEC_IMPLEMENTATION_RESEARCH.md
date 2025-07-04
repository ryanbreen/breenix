# Linux Kernel Exec Implementation Research

**Date:** January 2025
**Purpose:** Research proper OS-standard approach for implementing exec() to fix page table switching crash in Breenix

## 🚨 CRITICAL FINDING: The Real Problem

**Our current approach is WRONG.** We're trying to switch to the process page table during ELF loading, but real operating systems **DO NOT** do this. Here's what Linux actually does:

## How Linux Kernel Implements exec()

### 1. The Core Principle: **KERNEL STAYS IN KERNEL SPACE**

**CRITICAL:** During exec(), the kernel **NEVER** switches to the process page table while loading the ELF. The kernel stays in kernel address space throughout the entire loading process.

### 2. The Linux exec() Sequence (from binfmt_elf.c)

1. **`begin_new_exec(bprm)`** - Initiates clearing current executable's memory context
2. **`flush_old_exec(bprm)`** - Clears state referring to previous program:
   - Calls `__set_task_comm()` - sets thread name
   - Calls `flush_signal_handlers()` - sets up signal handlers for new program  
   - Calls `do_close_on_exec()` - closes file descriptors with O_CLOEXEC
3. **`exec_mmap(bprm->mm)`** - **THIS IS THE KEY FUNCTION**:
   - Creates a NEW `mm_struct` (memory management structure)
   - Sets up new page tables for the process
   - Switches the process's `mm->pgd` (Page Global Directory) to new page tables
   - **BUT THE KERNEL CONTINUES RUNNING IN KERNEL SPACE**
4. **Segment Loading via `vm_mmap()`**:
   - Uses `elf_load()` to map segments
   - **ALL MAPPING HAPPENS FROM KERNEL SPACE**
   - Kernel maps pages into the **process's** address space using the process's page tables
   - **BUT KERNEL NEVER SWITCHES TO THOSE PAGE TABLES**

### 3. How Memory Mapping Works During exec()

```c
// Pseudocode of what Linux does:
// 1. Create new page tables for process
mm_struct *new_mm = exec_mmap(old_mm);

// 2. Load ELF segments - KERNEL STAYS IN KERNEL SPACE
for each PT_LOAD segment {
    // Map pages into the PROCESS address space (not kernel)
    vm_mmap(new_mm, vaddr, size, prot, flags, file, offset);
    // ^^^^^ This modifies the PROCESS page tables, but kernel doesn't switch to them
}

// 3. Only switch when returning to userspace
// The process page table switch happens during context switch back to userspace
```

### 4. The TLB/Cache Flushing Strategy

- **`flush_cache_mm(mm)`** - Flushes entire user address space from caches
- **`flush_tlb_mm(mm)`** - Flushes entire user address space from TLB  
- These are called for "whole address space page table operations such as fork and exec"

### 5. Process Address Space Management

- Each process has `mm_struct->pgd` pointing to its Page Global Directory
- On x86: Process page table loaded by copying `mm_struct->pgd` into CR3 register
- **But this only happens during context switch, NOT during ELF loading**

## What We Need to Fix in Breenix

### ❌ WRONG Approach (What We're Doing Now)
```rust
// WRONG: Switch to process page table during loading
unsafe {
    switch_to_process_page_table(page_table);  // This causes crash!
}
// Load ELF segments...
unsafe {
    switch_to_kernel_page_table();
}
```

### ✅ CORRECT Approach (Linux-Style)
```rust
// CORRECT: Stay in kernel space, map into process space
fn load_segment_into_page_table(
    data: &[u8], 
    ph: &Elf64ProgramHeader, 
    page_table: &mut ProcessPageTable
) -> Result<(), &'static str> {
    // Map pages in the PROCESS page table (not current page table)
    for page in pages {
        let frame = allocate_frame();
        page_table.map_page(page, frame, flags)?;  // Maps in process space
        
        // Copy data using PHYSICAL memory access (like Linux)
        let phys_ptr = physical_memory_offset() + frame.start_address();
        unsafe {
            // Copy directly to physical memory
            copy_nonoverlapping(src, phys_ptr, size);
        }
    }
    // NEVER switch page tables during loading!
}
```

## Key Implementation Changes Needed

1. **Remove page table switching from ELF loading entirely**
2. **Use physical memory mapping to write to process pages**
3. **Implement proper exec_mmap equivalent**:
   - Create new ProcessPageTable for the process
   - Copy kernel mappings (PML4 entries 256-511)
   - Replace process's page table atomically
4. **Only switch to process page table during context switch to userspace**

## Why Our Approach Failed

1. **Kernel stack unmapped**: When we switched to process page table, kernel stack became inaccessible
2. **Kernel code unmapped**: Process page table didn't have all necessary kernel mappings
3. **Wrong timing**: Page table switch should happen during context switch, not during loading

## The Right Implementation Pattern

Linux uses **dual address space management**:
- **Kernel space**: Where kernel code runs, has access to all physical memory
- **Process space**: Where process will run, separate page tables
- **Kernel maps process pages from kernel space**: Never switches away from kernel space during exec()

## Next Steps for Implementation

1. Revert all page table switching code from ELF loading
2. Implement physical memory access for writing to process pages  
3. Test that exec() works without any page table switching
4. Only implement page table switching during actual context switch to userspace

## References

- Linux kernel `fs/binfmt_elf.c` - `load_elf_binary()` function
- Linux kernel `fs/exec.c` - `flush_old_exec()`, `exec_mmap()` functions
- Linux VM documentation on page table management
- Cache and TLB flushing architecture documentation

**CONCLUSION: We need to completely change our approach to match Linux's dual-space design.**