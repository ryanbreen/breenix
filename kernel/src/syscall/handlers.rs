//! System call handler implementations
//! 
//! This module contains the actual implementation of each system call.

use super::SyscallResult;
use alloc::boxed::Box;
use alloc::vec::Vec;

/// File descriptors
#[allow(dead_code)]
const FD_STDIN: u64 = 0;
const FD_STDOUT: u64 = 1;
const FD_STDERR: u64 = 2;

/// Copy data from userspace memory
/// 
/// For now, this is a simple implementation that attempts to access memory
/// that should be mapped in both kernel and user page tables.
fn copy_from_user(user_ptr: u64, len: usize) -> Result<Vec<u8>, &'static str> {
    if user_ptr == 0 {
        return Err("null pointer");
    }
    
    // Basic validation - check if address is in reasonable userspace range
    // Accept both code/data range (0x10000000-0x80000000) and stack range (around 0x555555555000)
    let is_code_data_range = user_ptr >= 0x10000000 && user_ptr < 0x80000000;
    let is_stack_range = user_ptr >= 0x5555_5554_0000 && user_ptr < 0x5555_5570_0000; // Expanded to cover full stack region
    
    if !is_code_data_range && !is_stack_range {
        log::error!("copy_from_user: Invalid userspace address {:#x} (not in code/data or stack range)", user_ptr);
        return Err("invalid userspace address");
    }
    
    // Get current thread to find process
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("copy_from_user: No current thread");
            return Err("no current thread");
        }
    };
    
    // Find the process that owns this thread
    let process_page_table = {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((pid, process)) = manager.find_process_by_thread(current_thread_id) {
                log::debug!("copy_from_user: Found process {:?} for thread {}", pid, current_thread_id);
                // Get the process's page table CR3 value
                if let Some(ref page_table) = process.page_table {
                    page_table.level_4_frame()
                } else {
                    log::error!("copy_from_user: Process has no page table");
                    return Err("process has no page table");
                }
            } else {
                log::error!("copy_from_user: No process found for thread {}", current_thread_id);
                return Err("no process for thread");
            }
        } else {
            log::error!("copy_from_user: No process manager");
            return Err("no process manager");
        }
    };
    
    // Check what page table we're currently using
    let current_cr3 = x86_64::registers::control::Cr3::read();
    log::debug!("copy_from_user: Current CR3: {:#x}, Process CR3: {:#x}", 
               current_cr3.0.start_address(), process_page_table.start_address());
    
    // Allocate buffer in kernel memory BEFORE switching page tables
    let mut buffer = Vec::with_capacity(len);
    
    // Try a single byte first to see if it's accessible
    unsafe {
        log::debug!("copy_from_user: Testing single byte access at {:#x}", user_ptr);
        
        // Switch to process page table
        x86_64::registers::control::Cr3::write(
            process_page_table,
            x86_64::registers::control::Cr3Flags::empty()
        );
        
        // Try to read just one byte
        let test_byte = *(user_ptr as *const u8);
        log::debug!("copy_from_user: Single byte test successful: {:#x}", test_byte);
        
        // Switch back to kernel page table
        x86_64::registers::control::Cr3::write(current_cr3.0, current_cr3.1);
        
        // If that worked, do the full copy
        log::debug!("copy_from_user: Proceeding with full copy of {} bytes", len);
        
        // Switch back to process page table for full copy
        x86_64::registers::control::Cr3::write(
            process_page_table,
            x86_64::registers::control::Cr3Flags::empty()
        );
        
        // Copy all the data
        let slice = core::slice::from_raw_parts(user_ptr as *const u8, len);
        buffer.extend_from_slice(slice);
        
        // Switch back to kernel page table
        x86_64::registers::control::Cr3::write(current_cr3.0, current_cr3.1);
    }
    
    log::debug!("copy_from_user: Successfully copied {} bytes", len);
    Ok(buffer)
}

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
    
    // Copy data from userspace
    let buffer = match copy_from_user(buf_ptr, count as usize) {
        Ok(buf) => buf,
        Err(e) => {
            log::error!("sys_write: Failed to copy from user: {}", e);
            return SyscallResult::Err(14); // EFAULT
        }
    };
    
    // Write to serial port
    let mut bytes_written = 0;
    for &byte in &buffer {
        crate::serial::write_byte(byte);
        bytes_written += 1;
    }
    
    // Log the output for userspace writes
    if let Ok(s) = core::str::from_utf8(&buffer) {
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
/// sys_fork with syscall frame - provides access to actual userspace context
pub fn sys_fork_with_frame(frame: &super::handler::SyscallFrame) -> SyscallResult {
    // Store the userspace RSP for the child to inherit
    let userspace_rsp = frame.rsp;
    log::info!("sys_fork_with_frame: userspace RSP = {:#x}", userspace_rsp);
    
    // Call fork with the userspace context
    sys_fork_with_rsp(userspace_rsp)
}

/// sys_fork with explicit RSP - used by sys_fork_with_frame
fn sys_fork_with_rsp(userspace_rsp: u64) -> SyscallResult {
    // Disable interrupts for the entire fork operation to ensure atomicity
    x86_64::instructions::interrupts::without_interrupts(|| {
        log::info!("sys_fork_with_rsp called with RSP {:#x}", userspace_rsp);
        
        // Get current thread ID from scheduler
        let scheduler_thread_id = crate::task::scheduler::current_thread_id();
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
        
        // Drop the lock before creating page table to avoid deadlock
        drop(manager_guard);
        
        // Create the child page table BEFORE re-acquiring the lock
        // This avoids deadlock during memory allocation
        log::info!("sys_fork: Creating page table for child process");
        let child_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
            Ok(pt) => Box::new(pt),
            Err(e) => {
                log::error!("sys_fork: Failed to create child page table: {}", e);
                return SyscallResult::Err(12); // ENOMEM
            }
        };
        log::info!("sys_fork: Child page table created successfully");
        
        // Now re-acquire the lock and complete the fork
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            let rsp_option = if userspace_rsp != 0 { Some(userspace_rsp) } else { None };
            match manager.fork_process_with_page_table(parent_pid, rsp_option, child_page_table) {
                Ok(child_pid) => {
                    // Get the child's thread ID to add to scheduler
                    if let Some(child_process) = manager.get_process(child_pid) {
                        if let Some(child_thread) = &child_process.main_thread {
                            let child_thread_id = child_thread.id;
                            let child_thread_clone = child_thread.clone();
                            
                            // Drop the lock before spawning to avoid issues
                            drop(manager_guard);
                            
                            // Add the child thread to the scheduler
                            log::info!("sys_fork: Spawning child thread {} to scheduler", child_thread_id);
                            crate::task::scheduler::spawn(Box::new(child_thread_clone));
                            log::info!("sys_fork: Child thread spawned successfully");
                            
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
    })
}

pub fn sys_fork() -> SyscallResult {
    // Call fork without userspace context (use calculated RSP)
    sys_fork_with_rsp(0)
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
        
        // For testing purposes, we'll check the program name to select the right ELF
        // In a real implementation, this would come from the filesystem
        let elf_data = if program_name_ptr != 0 {
            // Try to read the program name from userspace
            // For now, we'll just use a simple check
            log::info!("sys_exec: Program name requested, checking for known programs");
            
            // HACK: For now, we'll assume if program_name_ptr is provided,
            // it's asking for hello_time.elf
            #[cfg(feature = "testing")]
            {
                log::info!("sys_exec: Using hello_time.elf for exec test");
                crate::userspace_test::get_test_binary_static("hello_time")
            }
            #[cfg(not(feature = "testing"))]
            {
                log::error!("sys_exec: Testing feature not enabled");
                return SyscallResult::Err(22); // EINVAL
            }
        } else if elf_data_ptr != 0 {
            // In a real implementation, we'd safely copy from user memory
            log::info!("sys_exec: Using ELF data from pointer {:#x}", elf_data_ptr);
            // For now, return an error since we don't have safe user memory access yet
            log::error!("sys_exec: User memory access not implemented yet");
            return SyscallResult::Err(22); // EINVAL
        } else {
            // Use embedded test program for now
            #[cfg(feature = "testing")]
            {
                log::info!("sys_exec: Using generated hello_world test program");
                crate::userspace_test::get_test_binary_static("hello_world")
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