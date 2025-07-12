# Fork Implementation Fix Requirements

Date: 2025-01-11

## Current State (BROKEN)

The current fork implementation in `ProcessManager::fork_process()` and `fork_process_with_page_table()` is fundamentally broken:

```rust
// HACK: Load fork_test.elf into child instead of copying parent's memory
#[cfg(feature = "testing")]
{
    let elf_data = crate::userspace_test::FORK_TEST_ELF;
    let loaded_elf = crate::elf::load_elf_into_page_table(elf_data, child_page_table.as_mut())?;
    child_process.entry_point = loaded_elf.entry_point;
    log::info!("fork_process: Loaded fork_test.elf into child, entry point: {:#x}", loaded_elf.entry_point);
}
```

This causes:
- Child processes run completely different code than the parent
- Page faults when child accesses parent's data structures
- All fork-based tests fail
- waitpid tests cannot run properly

## Required Fork Behavior (POSIX Standard)

Fork must create an **exact copy** of the parent process with:

1. **Identical Memory Layout**:
   - All code segments at same addresses
   - All data segments with same contents
   - Stack with same contents (but independent copy)
   - Same entry point and instruction pointer

2. **Independent Address Space**:
   - Child gets its own page tables
   - Modifications in child don't affect parent
   - Modifications in parent don't affect child

3. **Shared Execution State**:
   - Child continues from same instruction as parent
   - Same register values (except fork return value)
   - Same stack pointer
   - Same program counter

## Implementation Options

### Option 1: Immediate Copy (Simple but Inefficient)
```
1. Create new page table for child
2. For each mapped page in parent:
   - Allocate new physical frame
   - Copy contents from parent frame to child frame
   - Map in child's page table with same virtual address
3. Copy register state
4. Return 0 in child, child PID in parent
```

**Pros**: Simple, correct, no complex tracking
**Cons**: Expensive (copies all memory immediately)

### Option 2: Copy-on-Write (Efficient but Complex)
```
1. Create new page table for child
2. For each mapped page in parent:
   - Map same physical frame in child (read-only)
   - Mark page as COW in both parent and child
3. On write fault:
   - Allocate new frame
   - Copy page contents
   - Update faulting process's mapping to new frame
   - Mark as writable
4. Track reference counts for shared frames
```

**Pros**: Efficient, standard Unix approach
**Cons**: Complex, requires page fault handling changes

### Option 3: Hybrid Approach (Recommended for Breenix)
```
1. Create new page table for child
2. For each segment type:
   - Code: Share read-only (never modified)
   - Data: Immediate copy
   - Stack: Immediate copy
3. Copy register state
4. Return appropriate values
```

**Pros**: Simpler than full COW, more efficient than full copy
**Cons**: Still copies data/stack immediately

## Specific Code Changes Required

### 1. Remove ELF Loading Hack
In `kernel/src/process/manager.rs`, remove:
```rust
#[cfg(feature = "testing")]
{
    let elf_data = crate::userspace_test::FORK_TEST_ELF;
    let loaded_elf = crate::elf::load_elf_into_page_table(elf_data, child_page_table.as_mut())?;
    // ...
}
```

### 2. Implement Memory Copying
Replace with actual memory copy logic:
```rust
// Pseudo-code for immediate copy approach
pub fn copy_parent_memory_to_child(
    parent_page_table: &ProcessPageTable,
    child_page_table: &mut ProcessPageTable,
    parent_pid: ProcessId,
    child_pid: ProcessId,
) -> Result<(), &'static str> {
    // 1. Get parent's mapped pages
    let parent_mappings = get_user_mappings(parent_page_table)?;
    
    // 2. For each mapped page in parent
    for (virt_addr, parent_frame, flags) in parent_mappings {
        // 3. Allocate new frame for child
        let child_frame = allocate_frame()?;
        
        // 4. Copy contents from parent to child frame
        unsafe {
            copy_frame_contents(parent_frame, child_frame)?;
        }
        
        // 5. Map in child's page table
        child_page_table.map_page(virt_addr, child_frame, flags)?;
    }
    
    Ok(())
}
```

### 3. Required Helper Functions

```rust
// Get all user-space mappings from a page table
fn get_user_mappings(page_table: &ProcessPageTable) -> Result<Vec<(VirtAddr, PhysFrame, PageTableFlags)>, &'static str>

// Copy contents of one physical frame to another
unsafe fn copy_frame_contents(src: PhysFrame, dst: PhysFrame) -> Result<(), &'static str>

// Walk page tables to find all mapped pages
fn walk_page_table(page_table: &ProcessPageTable) -> PageTableWalker
```

### 4. Update Thread Context Copy
Ensure the child thread has correct context:
```rust
// Child's context should be identical except:
child_thread.context.rax = 0;  // Fork returns 0 in child
// All other registers including RIP must be identical
```

### 5. Handle Special Memory Regions

- **Stack**: Must be copied (not shared)
- **Heap**: Must be copied if exists
- **Code**: Can be shared read-only (optimization)
- **Kernel Stack**: Already handled separately

## Testing Requirements

Once fork is fixed, these should work:

1. **Simple Fork Test**:
   ```c
   pid_t pid = fork();
   if (pid == 0) {
       printf("Child\n");
       exit(0);
   } else {
       printf("Parent\n");
       wait(NULL);
   }
   ```

2. **Memory Independence Test**:
   ```c
   int x = 42;
   pid_t pid = fork();
   if (pid == 0) {
       x = 99;  // Shouldn't affect parent
       exit(x);
   } else {
       wait(NULL);
       assert(x == 42);  // Parent's x unchanged
   }
   ```

3. **Stack Test**:
   ```c
   void recursive(int n) {
       int local = n;
       if (n > 0) {
           if (fork() == 0) {
               recursive(n - 1);
               exit(local);
           }
       }
   }
   ```

## Critical Constraints

1. **Must use existing Breenix infrastructure**:
   - ProcessPageTable for page table management
   - Frame allocator for physical memory
   - Existing ELF loader patterns for memory access

2. **Must maintain safety**:
   - No data races during copy
   - Proper locking of process manager
   - Handle allocation failures gracefully

3. **Must be compatible with waitpid**:
   - Parent-child relationships properly maintained
   - Exit status properly propagated
   - Process cleanup on wait

## Debugging Aids

Add logging to verify:
```rust
log::info!("Fork: Copying page {:#x} from parent frame {:#x} to child frame {:#x}", 
          virt_addr, parent_frame, child_frame);
log::info!("Fork: Child process {} has {} mapped pages", 
          child_pid, mapped_page_count);
log::info!("Fork: Parent RIP={:#x}, Child RIP={:#x}", 
          parent_rip, child_rip);
```

## Success Criteria

Fork is fixed when:
1. ✅ Child runs same code as parent (not fork_test.elf)
2. ✅ Child can access parent's data (as a copy)
3. ✅ No page faults from child accessing parent memory
4. ✅ waitpid tests pass
5. ✅ Memory modifications are independent
6. ✅ Both processes continue from fork call site