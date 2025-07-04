# Page Table Switching Fix for Fork/Exec

## Date: January 2025

## Problem Summary

Fork and exec were crashing with a double fault when switching to process page tables. The system would crash immediately after switching from kernel page table to a process page table.

## Root Cause

The issue was in `ProcessPageTable::new()` in `kernel/src/memory/process_memory.rs`. The code was copying ALL entries from the current page table, including both kernel AND userspace mappings:

```rust
// WRONG - copies everything including userspace mappings
for i in 0..512 {
    if !current_l4_table[i].is_unused() {
        level_4_table[i] = current_l4_table[i].clone();
    }
}
```

This caused userspace mappings from the kernel's initial state to be present in every new process's page table, leading to conflicts when fork tried to map pages that were already mapped.

## Solution

Fixed by only copying kernel mappings (upper half of virtual address space):

```rust
// CORRECT - only copy kernel mappings
for i in 256..512 {
    if !current_l4_table[i].is_unused() {
        level_4_table[i] = current_l4_table[i].clone();
    }
}
```

### x86_64 Virtual Address Space Layout

- **PML4** has 512 entries, each covering 512GB
- **Entries 0-255**: User space (0x0000_0000_0000_0000 to 0x0000_FFFF_FFFF_FFFF)
- **Entries 256-511**: Kernel space (0xFFFF_0000_0000_0000 to 0xFFFF_FFFF_FFFF_FFFF)

By only copying entries 256-511, we ensure:
1. Kernel is accessible from all processes (required for syscalls, interrupts, etc.)
2. Each process starts with a clean userspace (no conflicts)
3. Processes are properly isolated from each other

## Implementation Details

### Page Table Switching Timing

Page table switching happens at the correct moment - in assembly code right before `iretq`:

1. **Timer Interrupts** (`kernel/src/interrupts/timer_entry.asm`):
   - Check if returning to Ring 3 (userspace)
   - Call `get_next_page_table()` to get scheduled page table
   - Switch with `mov cr3, rax` right before `iretq`

2. **System Calls** (`kernel/src/syscall/entry.asm`):
   - Similar logic before returning to userspace
   - Ensures process has correct page table when resuming

### Context Switch Integration

The `context_switch.rs` module schedules page table switches by setting `NEXT_PAGE_TABLE` when switching to a userspace thread. The actual switch happens in assembly to avoid issues with executing kernel code after switching page tables.

## Testing

Fork test (`forktest` command) now works successfully:
- Parent process (PID 1) creates child (PID 2)
- Page tables are properly isolated
- Memory regions are correctly copied
- No more "PageAlreadyMapped" errors

## Key Lessons

1. **Be precise about kernel vs userspace mappings** - copying everything can cause subtle conflicts
2. **Page table switches must happen at the right moment** - switching while executing kernel code will crash
3. **Assembly-level control is necessary** - some operations need to happen at precise points in the interrupt return path