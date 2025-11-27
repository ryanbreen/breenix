# AGENTS.md: waitpid Kernel Implementation

## Mission

Implement the `sys_waitpid` syscall in the Breenix kernel. This enables parent processes to wait for child processes to exit and retrieve their exit status.

## Prerequisites

Read the research document first:
- `docs/planning/posix-compliance/AGENTS_WAITPID_RESEARCH.md`

## Implementation Tasks

### Task 1: Add Syscall Number

**File:** `kernel/src/syscall/mod.rs`

Find the `SyscallNumber` enum and add:
```rust
WaitPid = 7,        // Linux syscall number for waitpid
```

Find the `TryFrom<u64>` implementation and add:
```rust
7 => Some(Self::WaitPid),
```

### Task 2: Add Syscall Dispatch

**File:** `kernel/src/syscall/dispatcher.rs`

In the `dispatch` function's match statement, add:
```rust
SyscallNumber::WaitPid => {
    handlers::sys_waitpid(arg1 as i64, arg2, arg3)
}
```

### Task 3: Implement sys_waitpid Handler

**File:** `kernel/src/syscall/handlers.rs`

Add this function (adjust based on existing patterns in the file):

```rust
/// sys_waitpid - Wait for child process state change
///
/// Arguments:
///   pid (arg1): Which process to wait for
///     - pid > 0: Wait for specific child with that PID
///     - pid == -1: Wait for any child
///   wstatus (arg2): Pointer to store status (0 = don't store)
///   options (arg3): Wait options
///     - WNOHANG (1): Return immediately if no child has exited
///
/// Returns:
///   >0: PID of child that changed state
///   0: WNOHANG specified and no child changed state
///   -ECHILD: No children to wait for
pub fn sys_waitpid(pid: i64, wstatus: u64, options: u64) -> SyscallResult {
    const WNOHANG: u64 = 1;
    const ECHILD: u64 = 10;

    x86_64::instructions::interrupts::without_interrupts(|| {
        log::info!(
            "sys_waitpid: pid={}, wstatus={:#x}, options={:#x}",
            pid, wstatus, options
        );

        // Get current thread ID to find the calling process
        let current_thread_id = match crate::task::scheduler::current_thread_id() {
            Some(id) => id,
            None => {
                log::error!("sys_waitpid: No current thread");
                return SyscallResult::Err(ECHILD);
            }
        };

        // Get a mutable reference to the process manager
        let mut manager_guard = crate::process::manager();
        let manager = match manager_guard.as_mut() {
            Some(m) => m,
            None => {
                log::error!("sys_waitpid: Process manager not available");
                return SyscallResult::Err(ECHILD);
            }
        };

        // Find the current process
        let (current_pid, current_process) = match manager.find_process_by_thread(current_thread_id) {
            Some(result) => result,
            None => {
                log::error!("sys_waitpid: Could not find process for thread {}", current_thread_id);
                return SyscallResult::Err(ECHILD);
            }
        };

        log::info!(
            "sys_waitpid: Current process is PID {} with {} children",
            current_pid.as_u64(),
            current_process.children.len()
        );

        // Check if we have any children
        if current_process.children.is_empty() {
            log::info!("sys_waitpid: No children, returning ECHILD");
            return SyscallResult::Err(ECHILD);
        }

        // Copy children list to avoid borrow issues
        let children: alloc::vec::Vec<_> = current_process.children.clone();

        // Look for a terminated (zombie) child
        for &child_pid in &children {
            // Check if this child matches the requested PID
            let matches_requested = pid == -1 || pid as u64 == child_pid.as_u64();

            if !matches_requested {
                continue;
            }

            // Get the child process
            let child = match manager.get_process(child_pid) {
                Some(c) => c,
                None => continue,
            };

            // Check if child is terminated (zombie)
            if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                log::info!(
                    "sys_waitpid: Found zombie child PID {} with exit code {}",
                    child_pid.as_u64(),
                    exit_code
                );

                // Encode exit status in Linux format: (exit_code << 8)
                let status_word: u32 = ((exit_code as u32) & 0xff) << 8;

                // Write status to userspace if pointer provided
                if wstatus != 0 {
                    log::info!(
                        "sys_waitpid: Writing status {:#x} to userspace addr {:#x}",
                        status_word, wstatus
                    );

                    // Get the current process's page table to translate userspace address
                    if let Some(ref page_table) = manager.get_process(current_pid).and_then(|p| p.page_table.as_ref()) {
                        if let Some(phys_addr) = page_table.translate_page(x86_64::VirtAddr::new(wstatus)) {
                            // Write the status word to the physical address
                            let ptr = (phys_addr + crate::memory::PHYS_MEM_OFFSET) as *mut u32;
                            unsafe {
                                core::ptr::write_volatile(ptr, status_word);
                            }
                            log::info!("sys_waitpid: Wrote status to physical {:#x}", phys_addr);
                        } else {
                            log::warn!("sys_waitpid: Could not translate status address {:#x}", wstatus);
                        }
                    }
                }

                // Reap the zombie: remove from parent's children list and process table
                let reaped_pid = child_pid.as_u64();
                manager.reap_process(current_pid, child_pid);

                log::info!("sys_waitpid: Reaped child PID {}, returning", reaped_pid);
                return SyscallResult::Ok(reaped_pid);
            }
        }

        // No zombie children found
        if options & WNOHANG != 0 {
            log::info!("sys_waitpid: WNOHANG set, no zombies, returning 0");
            return SyscallResult::Ok(0);
        }

        // TODO: For blocking wait, we would block here and wake when a child exits
        // For now, just return 0 like WNOHANG
        log::info!("sys_waitpid: No zombies found, returning 0 (blocking not implemented)");
        SyscallResult::Ok(0)
    })
}
```

### Task 4: Add reap_process to ProcessManager

**File:** `kernel/src/process/manager.rs`

Add this method to the `ProcessManager` impl block:

```rust
/// Reap a zombie process - remove it from parent's children list and process table
pub fn reap_process(&mut self, parent_pid: ProcessId, child_pid: ProcessId) {
    log::info!(
        "reap_process: Removing child {} from parent {}",
        child_pid.as_u64(),
        parent_pid.as_u64()
    );

    // Remove child from parent's children list
    if let Some(parent) = self.processes.get_mut(&parent_pid) {
        parent.children.retain(|&pid| pid != child_pid);
        log::info!(
            "reap_process: Parent {} now has {} children",
            parent_pid.as_u64(),
            parent.children.len()
        );
    }

    // Remove child from ready queue (should already be gone, but just in case)
    self.ready_queue.retain(|&pid| pid != child_pid);

    // Remove child from process table
    if let Some(removed) = self.processes.remove(&child_pid) {
        log::info!(
            "reap_process: Removed process {} '{}' from process table",
            child_pid.as_u64(),
            removed.name
        );

        // TODO: Free process resources (page tables, stacks, etc.)
        // For now, the Drop implementation will handle cleanup
    }
}
```

### Task 5: Add Syscall Handler Entry (if needed)

**File:** `kernel/src/syscall/handler.rs`

If there's a separate handler.rs with a match statement, add:
```rust
Some(SyscallNumber::WaitPid) => {
    super::handlers::sys_waitpid(arg1 as i64, arg2, arg3)
}
```

## Verification

After implementation, run:
```bash
cargo run -p xtask -- boot-stages
```

The build should succeed with no errors or warnings.

## Testing Notes

- The userspace test will be implemented separately
- For manual testing, you can add debug prints to see waitpid being called
- The fork_test already creates parent/child processes that can be used to verify

## Error Handling

- Return `SyscallResult::Err(10)` for ECHILD (no children)
- Return `SyscallResult::Ok(0)` for WNOHANG with no zombies
- Return `SyscallResult::Ok(child_pid)` when successfully reaping

## Memory Safety

- Use `without_interrupts` to prevent race conditions
- Clone the children list before iterating to avoid borrow checker issues
- Use the page table translation for writing to userspace

## Code Quality

- Add appropriate log statements at info level for debugging
- Follow existing code patterns in handlers.rs
- No compiler warnings allowed
