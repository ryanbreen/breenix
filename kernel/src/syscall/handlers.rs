//! System call handler implementations
//! 
//! This module contains the actual implementation of each system call.

use super::SyscallResult;
use core::slice;
use alloc::boxed::Box;

/// File descriptors
#[allow(dead_code)]
const FD_STDIN: u64 = 0;
const FD_STDOUT: u64 = 1;
const FD_STDERR: u64 = 2;

/// sys_exit - Terminate the current process
pub fn sys_exit(exit_code: i32) -> SyscallResult {
    log::info!("USERSPACE: sys_exit called with code: {}", exit_code);
    
    // Get current thread ID from scheduler
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        log::debug!("sys_exit: Current thread ID from scheduler: {}", thread_id);
        
        // Handle thread exit through ProcessScheduler
        crate::task::process_task::ProcessScheduler::handle_thread_exit(thread_id, exit_code);
        
        // Mark current thread as terminated
        crate::task::scheduler::with_scheduler(|scheduler| {
            if let Some(thread) = scheduler.current_thread_mut() {
                thread.set_terminated();
            }
        });
        
        // Check if there are any other userspace threads to run
        let has_other_userspace_threads = crate::task::scheduler::with_scheduler(|sched| {
            sched.has_userspace_threads()
        }).unwrap_or(false);
        
        if !has_other_userspace_threads {
            // No more userspace threads remaining
            log::info!("No more userspace threads remaining");
            
            // Wake the keyboard task to ensure it can process any pending input
            crate::keyboard::stream::wake_keyboard_task();
            log::info!("Woke keyboard task to ensure input processing continues");
        }
    } else {
        log::error!("sys_exit: No current thread in scheduler");
    }
    
    // Force an immediate reschedule by setting the need_resched flag
    // This ensures the terminated thread won't continue executing
    crate::task::scheduler::set_need_resched();
    
    // The terminated thread should never run again
    // The reschedule will happen when we return from the syscall
    SyscallResult::Ok(0)
}

/// Perform context switch after process exit
/// This should never return if there's another process to run
// Note: perform_process_exit_switch function removed as part of spawn mechanism cleanup
// Process switching now happens through the scheduler and new timer interrupt system

/// sys_write - Write to a file descriptor
/// 
/// Currently only supports stdout/stderr writing to serial port.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
    log::info!("USERSPACE: sys_write called: fd={}, buf_ptr={:#x}, count={}", fd, buf_ptr, count);
    
    // Validate file descriptor
    if fd != FD_STDOUT && fd != FD_STDERR {
        return SyscallResult::Err(22); // EINVAL
    }
    
    // Validate buffer pointer and count
    if buf_ptr == 0 || count == 0 {
        return SyscallResult::Ok(0);
    }
    
    // TODO: Once we have userspace, validate that buf_ptr is in user memory
    // For now, assume it's a valid kernel pointer
    
    // Safety: We're trusting the caller for now (kernel mode only)
    let buffer = unsafe {
        slice::from_raw_parts(buf_ptr as *const u8, count as usize)
    };
    
    // Write to serial port
    let mut bytes_written = 0;
    for &byte in buffer {
        crate::serial::write_byte(byte);
        bytes_written += 1;
    }
    
    // Log the output for userspace writes
    if let Ok(s) = core::str::from_utf8(buffer) {
        log::info!("USERSPACE OUTPUT: {}", s.trim_end());
    }
    
    SyscallResult::Ok(bytes_written)
}

/// sys_read - Read from a file descriptor
/// 
/// Currently returns 0 (no data available) as keyboard is async-only.
#[allow(dead_code)]
pub fn sys_read(fd: u64, _buf_ptr: u64, _count: u64) -> SyscallResult {
    // Validate file descriptor
    if fd != FD_STDIN {
        return SyscallResult::Err(22); // EINVAL
    }
    
    // TODO: Implement synchronous keyboard reading
    // For now, always return 0 (no data available)
    SyscallResult::Ok(0)
}

/// sys_yield - Yield CPU to another task
pub fn sys_yield() -> SyscallResult {
    // log::trace!("sys_yield called");
    
    // Yield to the scheduler
    crate::task::scheduler::yield_current();
    
    // Note: The actual context switch will happen on the next timer interrupt
    // We don't force an immediate switch here because:
    // 1. Software interrupts from userspace context are complex
    // 2. The timer interrupt will fire soon anyway (every 10ms)
    // 3. This matches typical OS behavior where yield is a hint, not a guarantee
    
    SyscallResult::Ok(0)
}

/// sys_get_time - Get current system time in ticks
pub fn sys_get_time() -> SyscallResult {
    let ticks = crate::time::get_ticks();
    // log::info!("USERSPACE: sys_get_time called, returning {} ticks", ticks);
    SyscallResult::Ok(ticks)
}

/// sys_fork - Basic fork implementation
pub fn sys_fork() -> SyscallResult {
    // Disable interrupts for the entire fork operation to ensure atomicity
    // This prevents race conditions when accessing process manager and scheduler
    x86_64::instructions::interrupts::without_interrupts(|| {
        log::info!("sys_fork called - implementing basic fork");
        
        // Get current thread ID from scheduler (not TLS, since we're in kernel mode after SWAPGS)
        let scheduler_thread_id = crate::task::scheduler::current_thread_id();
        
        log::info!("sys_fork: Scheduler thread ID: {:?}", scheduler_thread_id);
        
        // Use scheduler thread ID as the authoritative source
        let current_thread_id = match scheduler_thread_id {
            Some(id) => id,
            None => {
                log::error!("sys_fork: No current thread in scheduler");
                return SyscallResult::Err(22); // EINVAL
            }
        };
    
    if current_thread_id == 0 {
        log::error!("sys_fork: Cannot fork from idle thread");
        return SyscallResult::Err(22); // EINVAL
    }
    
    // Find the current process by thread ID
    let manager_guard = crate::process::manager();
    let process_info = if let Some(ref manager) = *manager_guard {
        manager.find_process_by_thread(current_thread_id)
    } else {
        log::error!("sys_fork: Process manager not available");
        return SyscallResult::Err(12); // ENOMEM
    };
    
    let (parent_pid, parent_process) = match process_info {
        Some((pid, process)) => (pid, process),
        None => {
            log::error!("sys_fork: Current thread {} not found in any process", current_thread_id);
            return SyscallResult::Err(3); // ESRCH
        }
    };
    
    log::info!("sys_fork: Found parent process {} (PID {})", parent_process.name, parent_pid.as_u64());
    
    // Perform the actual fork
    drop(manager_guard); // Drop the lock before calling fork to avoid deadlock
    
    let mut manager_guard = crate::process::manager();
    if let Some(ref mut manager) = *manager_guard {
        match manager.fork_process(parent_pid) {
            Ok(child_pid) => {
                // Get the child's thread ID to add to scheduler
                if let Some(child_process) = manager.get_process(child_pid) {
                    if let Some(child_thread) = &child_process.main_thread {
                        let child_thread_id = child_thread.id;
                        let child_thread_clone = child_thread.clone();
                        
                        // Add the child thread to the scheduler
                        crate::task::scheduler::spawn(Box::new(child_thread_clone));
                        
                        log::info!("sys_fork: Fork successful - parent {} gets child PID {}, thread {}", 
                            parent_pid.as_u64(), child_pid.as_u64(), child_thread_id);
                        
                        // Return the child PID to the parent
                        SyscallResult::Ok(child_pid.as_u64())
                    } else {
                        log::error!("sys_fork: Child process has no main thread");
                        SyscallResult::Err(12) // ENOMEM
                    }
                } else {
                    log::error!("sys_fork: Failed to find newly created child process");
                    SyscallResult::Err(12) // ENOMEM
                }
            }
            Err(e) => {
                log::error!("sys_fork: Failed to fork process: {}", e);
                SyscallResult::Err(12) // ENOMEM
            }
        }
    } else {
        log::error!("sys_fork: Process manager not available");
        SyscallResult::Err(12) // ENOMEM
    }
    }) // End of without_interrupts block
}

/// sys_exec - Replace the current process with a new program
/// 
/// This implements the exec() family of system calls, which replace the current
/// process's address space with a new program. The process ID remains the same,
/// but the program code, data, and stack are completely replaced.
/// 
/// Parameters:
/// - arg1: pointer to program name (currently unused in this simple implementation)
/// - arg2: pointer to ELF data in memory (for embedded programs)
/// 
/// Returns: Never returns on success (process is replaced)
/// Returns: Error code on failure
pub fn sys_exec(program_name_ptr: u64, elf_data_ptr: u64) -> SyscallResult {
    x86_64::instructions::interrupts::without_interrupts(|| {
        log::info!("sys_exec called: program_name_ptr={:#x}, elf_data_ptr={:#x}", 
                   program_name_ptr, elf_data_ptr);
        
        // Get current process and thread
        let current_thread_id = match crate::task::scheduler::current_thread_id() {
            Some(id) => id,
            None => {
                log::error!("sys_exec: No current thread");
                return SyscallResult::Err(22); // EINVAL
            }
        };
        
        // For now, we'll implement a simplified exec that loads from embedded ELF data
        // In a real implementation, we would:
        // 1. Parse the program name from user memory
        // 2. Load the program from filesystem
        // 3. Validate permissions
        
        // For testing purposes, we'll use a hardcoded ELF program
        // This would normally come from disk or be passed as a parameter
        let elf_data = if elf_data_ptr != 0 {
            // In a real implementation, we'd safely copy from user memory
            log::info!("sys_exec: Using ELF data from pointer {:#x}", elf_data_ptr);
            // For now, return an error since we don't have safe user memory access yet
            log::error!("sys_exec: User memory access not implemented yet");
            return SyscallResult::Err(22); // EINVAL
        } else {
            // Use embedded test program for now
            #[cfg(feature = "testing")]
            {
                log::info!("sys_exec: Using embedded hello_world test program");
                crate::userspace_test::HELLO_WORLD_ELF
            }
            #[cfg(not(feature = "testing"))]
            {
                log::error!("sys_exec: No ELF data provided and testing feature not enabled");
                return SyscallResult::Err(22); // EINVAL
            }
        };
        
        // Find current process
        let current_pid = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((pid, _)) = manager.find_process_by_thread(current_thread_id) {
                    pid
                } else {
                    log::error!("sys_exec: Thread {} not found in any process", current_thread_id);
                    return SyscallResult::Err(3); // ESRCH
                }
            } else {
                log::error!("sys_exec: Process manager not available");
                return SyscallResult::Err(12); // ENOMEM
            }
        };
        
        log::info!("sys_exec: Replacing process {} (thread {}) with new program", 
                   current_pid.as_u64(), current_thread_id);
        
        // Replace the process's address space
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            match manager.exec_process(current_pid, elf_data) {
                Ok(new_entry_point) => {
                    log::info!("sys_exec: Successfully replaced process address space, entry point: {:#x}", 
                               new_entry_point);
                    
                    // CRITICAL OS-STANDARD VIOLATION:
                    // exec() should NEVER return on success - the process is completely replaced
                    // In a proper implementation, exec_process would:
                    // 1. Replace the address space
                    // 2. Update the thread context
                    // 3. Jump directly to the new program (never returning here)
                    // 
                    // For now, we return success, but this violates POSIX semantics
                    // The interrupt return path will handle the actual switch
                    SyscallResult::Ok(0)
                }
                Err(e) => {
                    log::error!("sys_exec: Failed to exec process: {}", e);
                    SyscallResult::Err(12) // ENOMEM
                }
            }
        } else {
            log::error!("sys_exec: Process manager not available");
            SyscallResult::Err(12) // ENOMEM
        }
    })
}

/// sys_getpid - Get the current process ID
pub fn sys_getpid() -> SyscallResult {
    // Disable interrupts when accessing process manager
    x86_64::instructions::interrupts::without_interrupts(|| {
        log::info!("sys_getpid called");
        
        // Get current thread ID from scheduler
        let scheduler_thread_id = crate::task::scheduler::current_thread_id();
        log::info!("sys_getpid: scheduler_thread_id = {:?}", scheduler_thread_id);
        
        if let Some(thread_id) = scheduler_thread_id {
            // Find the process that owns this thread
        if let Some(ref manager) = *crate::process::manager() {
            if let Some((pid, _process)) = manager.find_process_by_thread(thread_id) {
                // Return the process ID
                log::info!("sys_getpid: Found process {} for thread {}", pid.as_u64(), thread_id);
                return SyscallResult::Ok(pid.as_u64());
            }
        }
        
        // If no process found, we might be in kernel/idle thread
        if thread_id == 0 {
            log::info!("sys_getpid: Thread 0 is kernel/idle thread");
            return SyscallResult::Ok(0); // Kernel/idle process
        }
        
        log::warn!("sys_getpid: Thread {} has no associated process", thread_id);
        return SyscallResult::Ok(0); // Return 0 as fallback
    }
    
    log::error!("sys_getpid: No current thread");
    SyscallResult::Ok(0) // Return 0 as fallback
    }) // End of without_interrupts block
}

/// sys_gettid - Get the current thread ID
pub fn sys_gettid() -> SyscallResult {
    // Get current thread ID from scheduler
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        // In Linux, the main thread of a process has TID = PID
        // For now, we just return the thread ID directly
        return SyscallResult::Ok(thread_id);
    }
    
    log::error!("sys_gettid: No current thread");
    SyscallResult::Ok(0) // Return 0 as fallback
}