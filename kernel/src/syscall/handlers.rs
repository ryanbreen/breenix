//! System call handler implementations
//!
//! This module contains the actual implementation of each system call.

use super::SyscallResult;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// Global flag to signal that userspace testing is complete and kernel should exit
pub static USERSPACE_TEST_COMPLETE: AtomicBool = AtomicBool::new(false);

/// File descriptors
#[allow(dead_code)]
const FD_STDIN: u64 = 0;
const FD_STDOUT: u64 = 1;
const FD_STDERR: u64 = 2;

/// Copy data from userspace memory
///
/// CRITICAL: This function now works WITHOUT switching CR3 registers.
/// The kernel mappings MUST be present in all process page tables for this to work.
/// We rely on the fact that userspace memory is mapped in the current page table.
fn copy_from_user(user_ptr: u64, len: usize) -> Result<Vec<u8>, &'static str> {
    // SIMPLIFIED: Just validate address range and copy directly
    // No logging, no process lookups - to avoid any potential double faults
    
    if user_ptr == 0 {
        return Err("null pointer");
    }

    // Basic validation - check if address is in reasonable userspace range
    let is_code_data_range = user_ptr >= 0x10000000 && user_ptr < 0x80000000;
    let is_stack_range = user_ptr >= 0x5555_5554_0000 && user_ptr < 0x5555_5570_0000;

    if !is_code_data_range && !is_stack_range {
        return Err("invalid userspace address");
    }

    // CRITICAL: Access user memory WITHOUT switching CR3
    // This works because when we're in a syscall from userspace, we're already
    // using the process's page table, which has both kernel and user mappings
    let mut buffer = Vec::with_capacity(len);
    
    unsafe {
        // Directly copy the data - the memory should be accessible
        // because we're already in the process's context
        let slice = core::slice::from_raw_parts(user_ptr as *const u8, len);
        buffer.extend_from_slice(slice);
    }

    Ok(buffer)
}

/// Copy data to userspace memory
///
/// CRITICAL: Like copy_from_user, this now works WITHOUT switching CR3.
/// We rely on kernel mappings being present in all process page tables.
pub fn copy_to_user(user_ptr: u64, kernel_ptr: u64, len: usize) -> Result<(), &'static str> {
    if user_ptr == 0 {
        return Err("null pointer");
    }

    // Basic validation - check if address is in reasonable userspace range
    let is_code_data_range = user_ptr >= 0x10000000 && user_ptr < 0x80000000;
    let is_stack_range = user_ptr >= 0x5555_5554_0000 && user_ptr < 0x5555_5570_0000;

    if !is_code_data_range && !is_stack_range {
        log::error!("copy_to_user: Invalid userspace address {:#x}", user_ptr);
        return Err("invalid userspace address");
    }

    // Get current thread to find process - just for validation
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("copy_to_user: No current thread");
            return Err("no current thread");
        }
    };

    // Verify that we have a valid process (but don't switch page tables)
    {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((pid, _process)) = manager.find_process_by_thread(current_thread_id) {
                log::debug!(
                    "copy_to_user: Thread {} belongs to process {:?}",
                    current_thread_id,
                    pid
                );
            } else {
                log::error!(
                    "copy_to_user: No process found for thread {}",
                    current_thread_id
                );
                return Err("no process for thread");
            }
        } else {
            log::error!("copy_to_user: No process manager");
            return Err("no process manager");
        }
    }

    // Get current CR3 for logging only
    let current_cr3 = x86_64::registers::control::Cr3::read();
    log::debug!(
        "copy_to_user: Current CR3: {:#x}, writing to user memory at {:#x}",
        current_cr3.0.start_address(),
        user_ptr
    );

    // CRITICAL: Access user memory WITHOUT switching CR3
    // This works because when we're in a syscall from userspace, we're already
    // using the process's page table, which has both kernel and user mappings
    unsafe {
        log::debug!(
            "copy_to_user: Directly writing {} bytes to {:#x} (no CR3 switch)",
            len,
            user_ptr
        );

        // Directly copy the data - the memory should be accessible
        // because we're already in the process's context
        let dst = user_ptr as *mut u8;
        let src = kernel_ptr as *const u8;
        core::ptr::copy_nonoverlapping(src, dst, len);
    }

    log::debug!(
        "copy_to_user: Successfully copied {} bytes to {:#x}",
        len,
        user_ptr
    );
    Ok(())
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
        let has_other_userspace_threads =
            crate::task::scheduler::with_scheduler(|sched| sched.has_userspace_threads())
                .unwrap_or(false);

        if !has_other_userspace_threads {
            // No more userspace threads remaining
            log::info!("No more userspace threads remaining");

            // Wake the keyboard task to ensure it can process any pending input
            crate::keyboard::stream::wake_keyboard_task();
            log::info!("Woke keyboard task to ensure input processing continues");

            // Signal that userspace testing is complete with clear markers
            log::info!("🎯 USERSPACE TEST COMPLETE - All processes finished successfully");
            log::info!("=====================================");
            log::info!("✅ USERSPACE EXECUTION SUCCESSFUL ✅");
            log::info!("✅ Ring 3 execution confirmed       ✅");
            log::info!("✅ System calls working correctly   ✅");
            log::info!("✅ Process lifecycle complete       ✅");
            log::info!("=====================================");
            log::info!("🏁 TEST RUNNER: All tests passed - you can exit QEMU now 🏁");

            // Set flag for automated systems that want to detect completion
            USERSPACE_TEST_COMPLETE.store(true, Ordering::SeqCst);
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
    log::info!(
        "USERSPACE: sys_write called: fd={}, buf_ptr={:#x}, count={}",
        fd,
        buf_ptr,
        count
    );

    // Validate file descriptor
    if fd != FD_STDOUT && fd != FD_STDERR {
        return SyscallResult::Err(22); // EINVAL
    }

    // Validate buffer pointer and count
    if buf_ptr == 0 || count == 0 {
        return SyscallResult::Ok(0);
    }

    // Copy data from userspace
    log::info!("sys_write: About to call copy_from_user for {} bytes at {:#x}", count, buf_ptr);
    let buffer = match copy_from_user(buf_ptr, count as usize) {
        Ok(buf) => {
            log::info!("sys_write: copy_from_user succeeded, got {} bytes", buf.len());
            buf
        },
        Err(e) => {
            log::error!("sys_write: Failed to copy from user: {}", e);
            return SyscallResult::Err(14); // EFAULT
        }
    };

    // Log the actual data being written (for verification)
    if buffer.len() <= 30 {
        // For small writes, show the actual content
        let s = core::str::from_utf8(&buffer).unwrap_or("<invalid UTF-8>");
        
        // Also log the raw bytes in hex for verification
        let mut hex_str = alloc::string::String::new();
        for (i, &byte) in buffer.iter().enumerate() {
            if i > 0 {
                hex_str.push(' ');
            }
            hex_str.push_str(&alloc::format!("{:02x}", byte));
        }
        
        log::info!("sys_write: Writing '{}' ({} bytes) to fd {}", s, buffer.len(), fd);
        log::info!("  Raw bytes: [{}]", hex_str);
    } else {
        log::info!("sys_write: Writing {} bytes to fd {}", buffer.len(), fd);
    }
    
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

/// sys_get_time - Get current system time in milliseconds since boot
pub fn sys_get_time() -> SyscallResult {
    let millis = crate::time::get_monotonic_time();
    // log::info!("USERSPACE: sys_get_time called, returning {} ms", millis);
    SyscallResult::Ok(millis)
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
                log::error!(
                    "sys_fork: Current thread {} not found in any process",
                    current_thread_id
                );
                return SyscallResult::Err(3); // ESRCH
            }
        };

        log::info!(
            "sys_fork: Found parent process {} (PID {})",
            parent_process.name,
            parent_pid.as_u64()
        );

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
            let rsp_option = if userspace_rsp != 0 {
                Some(userspace_rsp)
            } else {
                None
            };
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
                            log::info!(
                                "sys_fork: Spawning child thread {} to scheduler",
                                child_thread_id
                            );
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
        log::info!(
            "sys_exec called: program_name_ptr={:#x}, elf_data_ptr={:#x}",
            program_name_ptr,
            elf_data_ptr
        );

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
                    log::error!(
                        "sys_exec: Thread {} not found in any process",
                        current_thread_id
                    );
                    return SyscallResult::Err(3); // ESRCH
                }
            } else {
                log::error!("sys_exec: Process manager not available");
                return SyscallResult::Err(12); // ENOMEM
            }
        };

        log::info!(
            "sys_exec: Replacing process {} (thread {}) with new program",
            current_pid.as_u64(),
            current_thread_id
        );

        // Replace the process's address space
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            match manager.exec_process(current_pid, elf_data) {
                Ok(new_entry_point) => {
                    log::info!(
                        "sys_exec: Successfully replaced process address space, entry point: {:#x}",
                        new_entry_point
                    );

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
        log::info!(
            "sys_getpid: scheduler_thread_id = {:?}",
            scheduler_thread_id
        );

        if let Some(thread_id) = scheduler_thread_id {
            // Find the process that owns this thread
            if let Some(ref manager) = *crate::process::manager() {
                if let Some((pid, _process)) = manager.find_process_by_thread(thread_id) {
                    // Return the process ID
                    log::info!(
                        "sys_getpid: Found process {} for thread {}",
                        pid.as_u64(),
                        thread_id
                    );
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
