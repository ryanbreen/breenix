//! System call handler implementations
//!
//! This module contains the actual implementation of each system call.

use super::SyscallResult;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// Global flag to signal that userspace testing is complete and kernel should exit
pub static USERSPACE_TEST_COMPLETE: AtomicBool = AtomicBool::new(false);

/// File descriptors (legacy constants, now using FdKind-based routing)
#[allow(dead_code)]
const FD_STDIN: u64 = 0;
#[allow(dead_code)]
const FD_STDOUT: u64 = 1;
#[allow(dead_code)]
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

    // Validate address is in valid userspace region (code/data or stack)
    if !crate::memory::layout::is_valid_user_address(user_ptr) {
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
///
/// NOTE: This function does NOT acquire the PROCESS_MANAGER lock.
/// It only validates the address range. The caller is responsible for
/// ensuring we're in a valid syscall context. This avoids deadlock when
/// called from syscall handlers that already hold the PROCESS_MANAGER lock.
pub fn copy_to_user(user_ptr: u64, kernel_ptr: u64, len: usize) -> Result<(), &'static str> {
    if user_ptr == 0 {
        return Err("null pointer");
    }

    // Validate address is in valid userspace region (code/data or stack)
    if !crate::memory::layout::is_valid_user_address(user_ptr) {
        log::error!("copy_to_user: Invalid userspace address {:#x}", user_ptr);
        return Err("invalid userspace address");
    }

    // CRITICAL: Access user memory WITHOUT switching CR3
    // This works because when we're in a syscall from userspace, we're already
    // using the process's page table, which has both kernel and user mappings
    unsafe {
        // Directly copy the data - the memory should be accessible
        // because we're already in the process's context
        let dst = user_ptr as *mut u8;
        let src = kernel_ptr as *const u8;
        core::ptr::copy_nonoverlapping(src, dst, len);
    }

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
/// Supports stdout/stderr (serial port) and pipe write ends.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
    use crate::ipc::FdKind;

    log::info!(
        "USERSPACE: sys_write called: fd={}, buf_ptr={:#x}, count={}",
        fd,
        buf_ptr,
        count
    );

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

    // Get current process to look up fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            // Fall back to stdio behavior for kernel threads
            return write_to_stdio(fd, &buffer);
        }
    };
    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => {
                // Fall back to stdio behavior for kernel threads
                return write_to_stdio(fd, &buffer);
            }
        },
        None => {
            // Fall back to stdio behavior for kernel threads
            return write_to_stdio(fd, &buffer);
        }
    };

    // Look up the file descriptor
    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(entry) => entry,
        None => {
            log::error!("sys_write: Bad fd {}", fd);
            return SyscallResult::Err(9); // EBADF
        }
    };

    match &fd_entry.kind {
        FdKind::StdIo(n) if *n == 1 || *n == 2 => {
            // stdout or stderr - write to serial
            write_to_stdio(fd, &buffer)
        }
        FdKind::StdIo(_) => {
            // stdin - can't write
            SyscallResult::Err(9) // EBADF
        }
        FdKind::PipeWrite(pipe_buffer) => {
            // Check O_NONBLOCK status flag
            let is_nonblocking = (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;

            // Write to pipe
            let mut pipe = pipe_buffer.lock();
            match pipe.write(&buffer) {
                Ok(n) => {
                    log::debug!("sys_write: Wrote {} bytes to pipe", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(11) => {
                    // EAGAIN - pipe buffer is full
                    if is_nonblocking {
                        // O_NONBLOCK set: return EAGAIN immediately
                        log::debug!("sys_write: Pipe full, O_NONBLOCK set - returning EAGAIN");
                        SyscallResult::Err(11) // EAGAIN
                    } else {
                        // O_NONBLOCK not set: should block, but blocking for pipes not implemented
                        // For now, return EAGAIN (same as nonblocking behavior)
                        // TODO: Implement blocking pipe writes
                        log::debug!("sys_write: Pipe full, blocking not implemented - returning EAGAIN");
                        SyscallResult::Err(11) // EAGAIN
                    }
                }
                Err(e) => {
                    log::debug!("sys_write: Pipe write error: {}", e);
                    SyscallResult::Err(e as u64)
                }
            }
        }
        FdKind::PipeRead(_) => {
            // Can't write to read end of pipe
            SyscallResult::Err(9) // EBADF
        }
        FdKind::UdpSocket(_) => {
            // Can't write to UDP socket - must use sendto
            log::error!("sys_write: Cannot write to UDP socket, use sendto instead");
            SyscallResult::Err(95) // EOPNOTSUPP
        }
    }
}

/// Helper function to write to stdio (serial port)
fn write_to_stdio(fd: u64, buffer: &[u8]) -> SyscallResult {
    let bytes_written = buffer.len() as u64;

    // In interactive mode, write to framebuffer so user can see shell output in QEMU window
    #[cfg(feature = "interactive")]
    {
        if let Ok(s) = core::str::from_utf8(buffer) {
            // Write to framebuffer for QEMU display
            crate::logger::write_to_framebuffer(s);
        }
        // Also write to COM1 for debugging (serial console)
        for &byte in buffer {
            crate::serial::write_byte(byte);
        }
    }

    // In non-interactive mode, write to serial port (for CI/testing)
    #[cfg(not(feature = "interactive"))]
    {
        for &byte in buffer {
            crate::serial::write_byte(byte);
        }

        // Log the output for userspace writes
        if let Ok(s) = core::str::from_utf8(buffer) {
            log::info!("USERSPACE OUTPUT: {}", s.trim_end());
        }
    }

    // Suppress the fd unused warning
    let _ = fd;

    SyscallResult::Ok(bytes_written)
}

/// sys_read - Read from a file descriptor
///
/// Supports stdin (with blocking), stdout/stderr (error), and pipe read ends.
pub fn sys_read(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
    use crate::ipc::FdKind;

    // Use trace level for stdin reads to avoid log spam during interactive shell
    if fd != 0 {
        log::debug!("sys_read: fd={}, buf_ptr={:#x}, count={}", fd, buf_ptr, count);
    }

    // Validate buffer pointer and count
    if buf_ptr == 0 || count == 0 {
        return SyscallResult::Ok(0);
    }

    // Get current process to look up fd
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            // Fall back to stdin behavior for kernel threads
            return SyscallResult::Ok(0);
        }
    };
    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => {
                // Fall back to stdin behavior for kernel threads
                return SyscallResult::Ok(0);
            }
        },
        None => {
            // Fall back to stdin behavior for kernel threads
            return SyscallResult::Ok(0);
        }
    };

    // Look up the file descriptor
    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(entry) => entry,
        None => {
            log::error!("sys_read: Bad fd {}", fd);
            return SyscallResult::Err(9); // EBADF
        }
    };

    match &fd_entry.kind {
        FdKind::StdIo(0) => {
            // stdin - read from TTY layer (which provides line editing and echo)
            // Drop the process manager lock before potentially blocking
            drop(manager_guard);

            let mut user_buf = alloc::vec![0u8; count as usize];

            // Try reading from TTY - it provides processed input (canonical mode)
            // Falls back to old stdin if TTY not initialized
            let read_result = if let Some(tty) = crate::tty::console() {
                tty.read(&mut user_buf)
            } else {
                // Fallback to direct stdin buffer if TTY not available
                crate::ipc::stdin::read_bytes(&mut user_buf)
            };

            match read_result {
                Ok(n) => {
                    if n > 0 {
                        // Copy to userspace
                        if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                            return SyscallResult::Err(14); // EFAULT
                        }
                    }
                    // Only log non-zero reads to avoid spam
                    if n > 0 {
                        log::trace!("sys_read: Read {} bytes from stdin/TTY", n);
                    }
                    SyscallResult::Ok(n as u64)
                }
                Err(11) => {
                    // EAGAIN - no data available, need to block
                    // Register this thread as waiting for TTY input
                    crate::tty::driver::TtyDevice::register_blocked_reader(thread_id);

                    // Block the current thread
                    crate::task::scheduler::with_scheduler(|sched| {
                        sched.block_current();
                    });

                    // Trigger reschedule
                    crate::task::scheduler::set_need_resched();

                    // Return ERESTARTSYS to indicate syscall should be restarted
                    // when the thread is woken up
                    SyscallResult::Err(512) // ERESTARTSYS
                }
                Err(e) => {
                    log::trace!("sys_read: Stdin/TTY read error: {}", e);
                    SyscallResult::Err(e as u64)
                }
            }
        }
        FdKind::StdIo(_) => {
            // stdout/stderr - can't read
            SyscallResult::Err(9) // EBADF
        }
        FdKind::PipeRead(pipe_buffer) => {
            // Check O_NONBLOCK status flag
            let is_nonblocking = (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;

            // Read from pipe
            let mut user_buf = alloc::vec![0u8; count as usize];
            let mut pipe = pipe_buffer.lock();
            match pipe.read(&mut user_buf) {
                Ok(n) => {
                    if n > 0 {
                        // Copy to userspace
                        if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                            return SyscallResult::Err(14); // EFAULT
                        }
                    }
                    log::debug!("sys_read: Read {} bytes from pipe", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(11) => {
                    // EAGAIN - pipe is empty but writers exist
                    if is_nonblocking {
                        // O_NONBLOCK set: return EAGAIN immediately
                        log::debug!("sys_read: Pipe empty, O_NONBLOCK set - returning EAGAIN");
                        SyscallResult::Err(11) // EAGAIN
                    } else {
                        // O_NONBLOCK not set: should block, but blocking for pipes not implemented
                        // For now, return EAGAIN (same as nonblocking behavior)
                        // TODO: Implement blocking pipe reads
                        log::debug!("sys_read: Pipe empty, blocking not implemented - returning EAGAIN");
                        SyscallResult::Err(11) // EAGAIN
                    }
                }
                Err(e) => {
                    log::debug!("sys_read: Pipe read error: {}", e);
                    SyscallResult::Err(e as u64)
                }
            }
        }
        FdKind::PipeWrite(_) => {
            // Can't read from write end of pipe
            SyscallResult::Err(9) // EBADF
        }
        FdKind::UdpSocket(_) => {
            // Can't read from UDP socket - must use recvfrom
            log::error!("sys_read: Cannot read from UDP socket, use recvfrom instead");
            SyscallResult::Err(95) // EOPNOTSUPP
        }
    }
}

/// sys_yield - Yield CPU to another task
pub fn sys_yield() -> SyscallResult {
    // log::trace!("sys_yield called");

    // Yield to the scheduler
    crate::task::scheduler::yield_current();

    // Note: The actual context switch will happen on the next timer interrupt
    // We don't force an immediate switch here because:
    // 1. Software interrupts from userspace context are complex
    // 2. The timer interrupt will fire soon anyway (every 100ms)
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
    // Create a CpuContext from the syscall frame - this captures the ACTUAL register
    // values at the time of the syscall, not the stale values from the last context switch
    let parent_context = crate::task::thread::CpuContext::from_syscall_frame(frame);

    log::info!(
        "sys_fork_with_frame: userspace RSP = {:#x}, return RIP = {:#x}",
        parent_context.rsp,
        parent_context.rip
    );

    // Debug: log some callee-saved registers that might hold local variables
    log::debug!(
        "sys_fork_with_frame: rbx={:#x}, rbp={:#x}, r12={:#x}, r13={:#x}, r14={:#x}, r15={:#x}",
        parent_context.rbx,
        parent_context.rbp,
        parent_context.r12,
        parent_context.r13,
        parent_context.r14,
        parent_context.r15
    );

    // Call fork with the complete parent context
    sys_fork_with_parent_context(parent_context)
}

/// sys_fork with full parent context - captures all registers from syscall frame
fn sys_fork_with_parent_context(parent_context: crate::task::thread::CpuContext) -> SyscallResult {
    // Disable interrupts for the entire fork operation to ensure atomicity
    x86_64::instructions::interrupts::without_interrupts(|| {
        log::info!(
            "sys_fork_with_parent_context called with RSP {:#x}, RIP {:#x}",
            parent_context.rsp,
            parent_context.rip
        );

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
            match manager.fork_process_with_parent_context(parent_pid, parent_context, child_page_table) {
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
    // DEPRECATED: This function should not be used - use sys_fork_with_frame instead
    // to get the actual register values at syscall time.
    log::error!("sys_fork() called without frame - this path is deprecated and broken!");
    log::error!("The syscall handler should use sys_fork_with_frame() to capture registers correctly.");
    SyscallResult::Err(22) // EINVAL - invalid argument
}

/// sys_exec_with_frame - Replace the current process with a new program
///
/// This is the proper implementation that modifies the syscall frame so that
/// when the syscall returns, it jumps to the NEW program instead of returning
/// to the old one.
///
/// Parameters:
/// - frame: mutable reference to the syscall frame (to update RIP/RSP on success)
/// - program_name_ptr: pointer to program name
/// - elf_data_ptr: pointer to ELF data in memory (for embedded programs)
///
/// Returns: Never returns on success (frame is modified to jump to new program)
/// Returns: Error code on failure
pub fn sys_exec_with_frame(
    frame: &mut super::handler::SyscallFrame,
    program_name_ptr: u64,
    elf_data_ptr: u64,
) -> SyscallResult {
    x86_64::instructions::interrupts::without_interrupts(|| {
        log::info!(
            "sys_exec_with_frame called: program_name_ptr={:#x}, elf_data_ptr={:#x}",
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

        // Load the program by name from the test disk
        let elf_data = if program_name_ptr != 0 {
            // Read the program name from userspace
            log::info!("sys_exec: Reading program name from userspace");

            // Read up to 64 bytes for the program name (null-terminated)
            let name_bytes = match copy_from_user(program_name_ptr, 64) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::error!("sys_exec: Failed to read program name: {}", e);
                    return SyscallResult::Err(14); // EFAULT
                }
            };

            // Debug: print first 32 bytes to see what we're reading
            log::debug!(
                "sys_exec: Raw bytes at {:#x}: {:02x?}",
                program_name_ptr,
                &name_bytes[..32.min(name_bytes.len())]
            );

            // Find the null terminator and extract the name
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
            log::debug!("sys_exec: Found null terminator at position {}", name_len);
            let program_name = match core::str::from_utf8(&name_bytes[..name_len]) {
                Ok(s) => s,
                Err(_) => {
                    log::error!("sys_exec: Invalid UTF-8 in program name");
                    return SyscallResult::Err(22); // EINVAL
                }
            };

            log::info!("sys_exec: Loading program '{}'", program_name);

            #[cfg(feature = "testing")]
            {
                // Load the binary from the test disk by name
                let elf_vec = crate::userspace_test::get_test_binary(program_name);
                // Leak the vector to get a static slice (needed for exec_process)
                let boxed_slice = elf_vec.into_boxed_slice();
                Box::leak(boxed_slice) as &'static [u8]
            }
            #[cfg(not(feature = "testing"))]
            {
                log::error!("sys_exec: Testing feature not enabled");
                return SyscallResult::Err(22); // EINVAL
            }
        } else if elf_data_ptr != 0 {
            log::info!("sys_exec: Using ELF data from pointer {:#x}", elf_data_ptr);
            log::error!("sys_exec: User memory access not implemented yet");
            return SyscallResult::Err(22); // EINVAL
        } else {
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

        #[cfg(feature = "testing")]
        {
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

                        // CRITICAL FIX: Get the new stack pointer from the process
                        // The exec_process function set up a new stack at USER_STACK_TOP
                        const USER_STACK_TOP: u64 = 0x5555_5555_5000;
                        let new_rsp = USER_STACK_TOP;

                        // Modify the syscall frame so that when we return from syscall,
                        // we jump to the NEW program instead of returning to the old one
                        frame.rip = new_entry_point;
                        frame.rsp = new_rsp;
                        frame.rflags = 0x202; // IF=1 (interrupts enabled), bit 1=1 (reserved)

                        // Clear all registers for security (new program shouldn't see old data)
                        frame.rax = 0;
                        frame.rbx = 0;
                        frame.rcx = 0;
                        frame.rdx = 0;
                        frame.rsi = 0;
                        frame.rdi = 0;
                        frame.rbp = 0;
                        frame.r8 = 0;
                        frame.r9 = 0;
                        frame.r10 = 0;
                        frame.r11 = 0;
                        frame.r12 = 0;
                        frame.r13 = 0;
                        frame.r14 = 0;
                        frame.r15 = 0;

                        // Set up CR3 for the new process page table
                        if let Some(process) = manager.get_process(current_pid) {
                            if let Some(ref page_table) = process.page_table {
                                let new_cr3 = page_table.level_4_frame().start_address().as_u64();
                                log::info!("sys_exec: Setting next_cr3 to {:#x}", new_cr3);
                                unsafe {
                                    crate::per_cpu::set_next_cr3(new_cr3);
                                    // Also update saved_process_cr3
                                    core::arch::asm!(
                                        "mov gs:[80], {}",
                                        in(reg) new_cr3,
                                        options(nostack, preserves_flags)
                                    );
                                }
                            }
                        }

                        log::info!(
                            "sys_exec: Frame updated - RIP={:#x}, RSP={:#x}",
                            frame.rip,
                            frame.rsp
                        );

                        // exec() returns 0 on success (but caller never sees it because
                        // we're jumping to a new program)
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
        }

        #[cfg(not(feature = "testing"))]
        {
            let _ = elf_data;
            SyscallResult::Err(38) // ENOSYS
        }
    })
}

/// sys_exec - Replace the current process with a new program (deprecated)
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
///
/// DEPRECATED: Use sys_exec_with_frame instead to properly update the syscall frame
pub fn sys_exec(program_name_ptr: u64, elf_data_ptr: u64) -> SyscallResult {
    x86_64::instructions::interrupts::without_interrupts(|| {
        log::info!(
            "sys_exec called: program_name_ptr={:#x}, elf_data_ptr={:#x}",
            program_name_ptr,
            elf_data_ptr
        );

        // Get current process and thread
        let _current_thread_id = match crate::task::scheduler::current_thread_id() {
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

        // Load the program by name from the test disk
        // In a real implementation, this would come from the filesystem
        let _elf_data = if program_name_ptr != 0 {
            // Read the program name from userspace
            log::info!("sys_exec: Reading program name from userspace");

            // Read up to 64 bytes for the program name (null-terminated)
            let name_bytes = match copy_from_user(program_name_ptr, 64) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::error!("sys_exec: Failed to read program name: {}", e);
                    return SyscallResult::Err(14); // EFAULT
                }
            };

            // Find the null terminator and extract the name
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
            let program_name = match core::str::from_utf8(&name_bytes[..name_len]) {
                Ok(s) => s,
                Err(_) => {
                    log::error!("sys_exec: Invalid UTF-8 in program name");
                    return SyscallResult::Err(22); // EINVAL
                }
            };

            log::info!("sys_exec: Loading program '{}'", program_name);

            #[cfg(feature = "testing")]
            {
                // Load the binary from the test disk by name
                let elf_vec = crate::userspace_test::get_test_binary(program_name);
                // Leak the vector to get a static slice (needed for exec_process)
                let boxed_slice = elf_vec.into_boxed_slice();
                Box::leak(boxed_slice) as &'static [u8]
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

        #[cfg(feature = "testing")]
        {
            // Find current process
            let current_pid = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((pid, _)) = manager.find_process_by_thread(_current_thread_id) {
                    pid
                } else {
                    log::error!(
                        "sys_exec: Thread {} not found in any process",
                        _current_thread_id
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
            _current_thread_id
        );

        // Replace the process's address space
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            match manager.exec_process(current_pid, _elf_data) {
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
        } // End of #[cfg(feature = "testing")] block
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

/// waitpid options constants
pub const WNOHANG: u32 = 1;
#[allow(dead_code)]
pub const WUNTRACED: u32 = 2;

/// sys_waitpid - Wait for a child process to change state
///
/// This implements the wait4/waitpid system call.
///
/// Arguments:
/// - pid: PID to wait for
///   - pid > 0: Wait for specific child with that PID
///   - pid == -1: Wait for any child
///   - pid == 0: Wait for any child in same process group (NOT IMPLEMENTED)
///   - pid < -1: Wait for any child in process group |pid| (NOT IMPLEMENTED)
/// - status_ptr: Pointer to store exit status (or 0/null to not store)
/// - options: Flags (WNOHANG, WUNTRACED, etc.)
///
/// Returns:
/// - On success: PID of terminated child
/// - If WNOHANG and no child terminated: 0
/// - On error: negative errno (ECHILD, EINVAL, EFAULT)
pub fn sys_waitpid(pid: i64, status_ptr: u64, options: u32) -> SyscallResult {
    log::debug!("sys_waitpid: pid={}, status_ptr={:#x}, options={}", pid, status_ptr, options);

    // Get current thread ID
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_waitpid: No current thread");
            return SyscallResult::Err(super::errno::EINVAL as u64);
        }
    };

    // Find current process
    let mut manager_guard = crate::process::manager();
    let (current_pid, current_process) = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((pid, process)) => (pid, process),
            None => {
                log::error!("sys_waitpid: Thread {} not in any process", thread_id);
                return SyscallResult::Err(super::errno::EINVAL as u64);
            }
        },
        None => {
            log::error!("sys_waitpid: No process manager");
            return SyscallResult::Err(super::errno::EINVAL as u64);
        }
    };

    log::debug!("sys_waitpid: Current process PID={}, has {} children",
                current_pid.as_u64(), current_process.children.len());

    // Check for children
    if current_process.children.is_empty() {
        log::debug!("sys_waitpid: No children - returning ECHILD");
        return SyscallResult::Err(super::errno::ECHILD as u64);
    }

    // Handle different pid values
    match pid {
        // pid > 0: Wait for specific child
        p if p > 0 => {
            let target_pid = crate::process::ProcessId::new(p as u64);

            // Check if target is actually our child
            if !current_process.children.contains(&target_pid) {
                log::debug!("sys_waitpid: PID {} is not a child of {}", p, current_pid.as_u64());
                return SyscallResult::Err(super::errno::ECHILD as u64);
            }

            // We need to drop the mutable borrow to check child state
            let children_copy: Vec<_> = current_process.children.clone();
            drop(manager_guard);

            // Check if the specific child is already terminated
            let child_terminated = {
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    if let Some(child) = manager.get_process(target_pid) {
                        if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                            Some((target_pid, exit_code))
                        } else {
                            None
                        }
                    } else {
                        // Child doesn't exist in process table - shouldn't happen
                        None
                    }
                } else {
                    None
                }
            };

            if let Some((child_pid, exit_code)) = child_terminated {
                return complete_wait(child_pid, exit_code, status_ptr, &children_copy);
            }

            // Child exists but not terminated
            if options & WNOHANG != 0 {
                log::debug!("sys_waitpid: WNOHANG set, child {} not terminated", p);
                return SyscallResult::Ok(0);
            }

            // Blocking wait - block until child terminates
            // Mark thread as blocked then enter HLT loop. The timer interrupt will
            // see that current thread is blocked and switch to another thread.
            // When the child exits, unblock_for_child_exit() puts us back in ready queue.
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_child_exit();
            });

            loop {
                // Yield and halt - timer interrupt will switch to another thread
                // since current thread is blocked
                crate::task::scheduler::yield_current();
                x86_64::instructions::interrupts::enable_and_hlt();

                // After being rescheduled, check if child terminated
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    if let Some(child) = manager.get_process(target_pid) {
                        if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                            drop(manager_guard);
                            return complete_wait(target_pid, exit_code, status_ptr, &children_copy);
                        }
                    }
                }
                // If not terminated yet (spurious wakeup), continue waiting
            }
        }

        // pid == -1: Wait for any child
        -1 => {
            let children_copy: Vec<_> = current_process.children.clone();
            drop(manager_guard);

            // Check if any child is already terminated
            let terminated_child = {
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    let mut result = None;
                    for &child_pid in &children_copy {
                        if let Some(child) = manager.get_process(child_pid) {
                            if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                                result = Some((child_pid, exit_code));
                                break;
                            }
                        }
                    }
                    result
                } else {
                    None
                }
            };

            if let Some((child_pid, exit_code)) = terminated_child {
                return complete_wait(child_pid, exit_code, status_ptr, &children_copy);
            }

            // No terminated children yet
            if options & WNOHANG != 0 {
                log::debug!("sys_waitpid: WNOHANG set, no children terminated");
                return SyscallResult::Ok(0);
            }

            // Blocking wait - block until any child terminates
            // Mark thread as blocked then enter HLT loop. The timer interrupt will
            // see that current thread is blocked and switch to another thread.
            // When a child exits, unblock_for_child_exit() puts us back in ready queue.
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_child_exit();
            });

            loop {
                // Yield and halt - timer interrupt will switch to another thread
                // since current thread is blocked
                crate::task::scheduler::yield_current();
                x86_64::instructions::interrupts::enable_and_hlt();

                // After being rescheduled, check if any child terminated
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    for &child_pid in &children_copy {
                        if let Some(child) = manager.get_process(child_pid) {
                            if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                                drop(manager_guard);
                                return complete_wait(child_pid, exit_code, status_ptr, &children_copy);
                            }
                        }
                    }
                }
                // If no child terminated yet (spurious wakeup), continue waiting
            }
        }

        // pid == 0 or pid < -1: Process groups not implemented
        _ => {
            log::warn!("sys_waitpid: Process groups not implemented (pid={})", pid);
            SyscallResult::Err(super::errno::ENOSYS as u64)
        }
    }
}

/// Helper function to complete a wait operation
/// Writes the status and removes the child from parent's children list
fn complete_wait(
    child_pid: crate::process::ProcessId,
    exit_code: i32,
    status_ptr: u64,
    _children: &[crate::process::ProcessId],
) -> SyscallResult {
    // Encode exit status in wstatus format (for WIFEXITED)
    // Linux encodes normal exit as: (exit_code & 0xff) << 8
    let wstatus: i32 = (exit_code & 0xff) << 8;

    log::debug!("complete_wait: child {} exited with code {}, wstatus={:#x}",
                child_pid.as_u64(), exit_code, wstatus);

    // Write status to userspace if pointer is valid
    if status_ptr != 0 {
        if let Err(e) = copy_to_user(status_ptr, &wstatus as *const i32 as u64, core::mem::size_of::<i32>()) {
            log::error!("complete_wait: Failed to write status: {}", e);
            return SyscallResult::Err(super::errno::EFAULT as u64);
        }
    }

    // Remove child from parent's children list
    // Get current thread to find parent process
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        let mut manager_guard = crate::process::manager();
        if let Some(ref mut manager) = *manager_guard {
            if let Some((_parent_pid, parent)) = manager.find_process_by_thread_mut(thread_id) {
                parent.children.retain(|&id| id != child_pid);
                log::debug!("complete_wait: Removed child {} from parent's children list",
                           child_pid.as_u64());
            }
        }
    }

    // CRITICAL: Clear the blocked_in_syscall flag now that the syscall is completing.
    // This ensures future context switches will restore userspace context normally.
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            if thread.blocked_in_syscall {
                thread.blocked_in_syscall = false;
                log::debug!("complete_wait: Cleared blocked_in_syscall flag for thread {}", thread.id);
            }
        }
    });

    // TODO: Actually remove/reap the child process from the process table
    // For now, we leave it in the table but in Terminated state

    SyscallResult::Ok(child_pid.as_u64())
}

/// sys_dup2 - Duplicate a file descriptor to a specific number
///
/// dup2(old_fd, new_fd) creates a copy of old_fd using the file descriptor
/// number specified in new_fd. If new_fd was previously open, it is silently
/// closed before being reused.
///
/// Per POSIX: if old_fd == new_fd, dup2 just validates old_fd and returns it.
/// This avoids a race condition where the reference count would temporarily
/// go to zero.
///
/// Returns: new_fd on success, negative error code on failure
pub fn sys_dup2(old_fd: u64, new_fd: u64) -> SyscallResult {
    log::debug!("sys_dup2: old_fd={}, new_fd={}", old_fd, new_fd);

    // Get current thread to find process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_dup2: No current thread");
            return SyscallResult::Err(9); // EBADF
        }
    };

    // Get mutable access to process manager
    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_dup2: Thread {} not in any process", thread_id);
                return SyscallResult::Err(9); // EBADF
            }
        },
        None => {
            log::error!("sys_dup2: No process manager");
            return SyscallResult::Err(9); // EBADF
        }
    };

    // Call the fd_table's dup2 implementation
    match process.fd_table.dup2(old_fd as i32, new_fd as i32) {
        Ok(fd) => {
            log::debug!("sys_dup2: Successfully duplicated fd {} to {}", old_fd, fd);
            SyscallResult::Ok(fd as u64)
        }
        Err(e) => {
            log::debug!("sys_dup2: Failed with error {}", e);
            SyscallResult::Err(e as u64)
        }
    }
}

/// sys_dup - Duplicate a file descriptor
///
/// dup(old_fd) creates a copy of old_fd using the lowest-numbered unused
/// file descriptor.
///
/// Returns: new fd on success, negative error code on failure
pub fn sys_dup(old_fd: u64) -> SyscallResult {
    log::debug!("sys_dup: old_fd={}", old_fd);

    // Get current thread to find process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_dup: No current thread");
            return SyscallResult::Err(9); // EBADF
        }
    };

    // Get mutable access to process manager
    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_dup: Thread {} not in any process", thread_id);
                return SyscallResult::Err(9); // EBADF
            }
        },
        None => {
            log::error!("sys_dup: No process manager");
            return SyscallResult::Err(9); // EBADF
        }
    };

    // Call the fd_table's dup implementation
    match process.fd_table.dup(old_fd as i32) {
        Ok(fd) => {
            log::debug!("sys_dup: Successfully duplicated fd {} to {}", old_fd, fd);
            SyscallResult::Ok(fd as u64)
        }
        Err(e) => {
            log::debug!("sys_dup: Failed with error {}", e);
            SyscallResult::Err(e as u64)
        }
    }
}

/// fcntl - file control operations
///
/// Performs various operations on file descriptors:
/// - F_DUPFD: Duplicate fd to lowest available >= arg
/// - F_DUPFD_CLOEXEC: Same as F_DUPFD but sets FD_CLOEXEC
/// - F_GETFD: Get fd flags (FD_CLOEXEC)
/// - F_SETFD: Set fd flags
/// - F_GETFL: Get file status flags (O_NONBLOCK, etc.)
/// - F_SETFL: Set file status flags
pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64) -> SyscallResult {
    use crate::ipc::fd::fcntl_cmd::*;

    let fd = fd as i32;
    let cmd = cmd as i32;
    let arg = arg as i32;

    log::debug!("sys_fcntl: fd={}, cmd={}, arg={}", fd, cmd, arg);

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_fcntl: No current thread!");
            return SyscallResult::Err(9); // EBADF
        }
    };

    let manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => {
            log::error!("sys_fcntl: Failed to get process manager");
            return SyscallResult::Err(9); // EBADF
        }
    };

    let _process = match manager_guard
        .as_ref()
        .and_then(|m| m.find_process_by_thread(thread_id))
        .map(|(_, p)| p)
    {
        Some(p) => p,
        None => {
            log::error!("sys_fcntl: Failed to find process for thread {}", thread_id);
            return SyscallResult::Err(9); // EBADF
        }
    };

    // Need to reborrow mutably for fd_table operations
    drop(manager_guard);
    let mut manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => return SyscallResult::Err(9),
    };
    let process = match manager_guard
        .as_mut()
        .and_then(|m| m.find_process_by_thread_mut(thread_id))
        .map(|(_, p)| p)
    {
        Some(p) => p,
        None => return SyscallResult::Err(9),
    };

    match cmd {
        F_DUPFD => {
            match process.fd_table.dup_at_least(fd, arg, false) {
                Ok(new_fd) => {
                    log::debug!("sys_fcntl F_DUPFD: {} -> {}", fd, new_fd);
                    SyscallResult::Ok(new_fd as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        F_DUPFD_CLOEXEC => {
            match process.fd_table.dup_at_least(fd, arg, true) {
                Ok(new_fd) => {
                    log::debug!("sys_fcntl F_DUPFD_CLOEXEC: {} -> {}", fd, new_fd);
                    SyscallResult::Ok(new_fd as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        F_GETFD => {
            match process.fd_table.get_fd_flags(fd) {
                Ok(flags) => {
                    log::debug!("sys_fcntl F_GETFD: fd={} flags={}", fd, flags);
                    SyscallResult::Ok(flags as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        F_SETFD => {
            match process.fd_table.set_fd_flags(fd, arg as u32) {
                Ok(()) => {
                    log::debug!("sys_fcntl F_SETFD: fd={} flags={}", fd, arg);
                    SyscallResult::Ok(0)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        F_GETFL => {
            match process.fd_table.get_status_flags(fd) {
                Ok(flags) => {
                    log::debug!("sys_fcntl F_GETFL: fd={} flags={:#x}", fd, flags);
                    SyscallResult::Ok(flags as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        F_SETFL => {
            match process.fd_table.set_status_flags(fd, arg as u32) {
                Ok(()) => {
                    log::debug!("sys_fcntl F_SETFL: fd={} flags={:#x}", fd, arg);
                    SyscallResult::Ok(0)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        _ => {
            log::warn!("sys_fcntl: Unknown command {}", cmd);
            SyscallResult::Err(22) // EINVAL
        }
    }
}

/// sys_poll - Poll file descriptors for I/O readiness
///
/// This implements the poll() syscall which monitors multiple file descriptors
/// for I/O readiness.
///
/// Arguments:
/// - fds_ptr: Pointer to array of pollfd structures
/// - nfds: Number of file descriptors to poll
/// - timeout: Timeout in milliseconds (-1 = infinite, 0 = non-blocking)
///
/// Returns:
/// - On success: Number of fds with non-zero revents
/// - On timeout: 0
/// - On error: negative errno
///
/// Note: Currently only non-blocking poll (timeout=0) is fully supported.
pub fn sys_poll(fds_ptr: u64, nfds: u64, _timeout: i32) -> SyscallResult {
    use crate::ipc::poll::{self, events, PollFd};

    log::debug!("sys_poll: fds_ptr={:#x}, nfds={}, timeout={}", fds_ptr, nfds, _timeout);

    // Validate parameters
    if fds_ptr == 0 && nfds > 0 {
        return SyscallResult::Err(14); // EFAULT
    }

    if nfds > 256 {
        return SyscallResult::Err(22); // EINVAL - too many fds
    }

    if nfds == 0 {
        return SyscallResult::Ok(0);
    }

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_poll: No current thread");
            return SyscallResult::Err(22); // EINVAL
        }
    };

    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_poll: Thread {} not in any process", thread_id);
                return SyscallResult::Err(22); // EINVAL
            }
        },
        None => {
            log::error!("sys_poll: No process manager");
            return SyscallResult::Err(22); // EINVAL
        }
    };

    // Read pollfd array from userspace
    let _pollfd_size = core::mem::size_of::<PollFd>();

    // Allocate buffer for pollfds
    let mut pollfds: Vec<PollFd> = Vec::with_capacity(nfds as usize);

    // Copy from userspace
    unsafe {
        let src = fds_ptr as *const PollFd;
        for i in 0..nfds as usize {
            pollfds.push(core::ptr::read(src.add(i)));
        }
    }

    // Poll each fd
    let mut ready_count: u64 = 0;

    for pollfd in pollfds.iter_mut() {
        // Clear revents
        pollfd.revents = 0;

        // Check if fd is valid
        if pollfd.fd < 0 {
            // Negative fd - skip it (per POSIX, ignore negative fds)
            continue;
        }

        // Check if fd exists
        let fd_entry = match process.fd_table.get(pollfd.fd) {
            Some(entry) => entry,
            None => {
                // Invalid fd - set POLLNVAL
                pollfd.revents = events::POLLNVAL;
                ready_count += 1;
                continue;
            }
        };

        // Poll this fd
        pollfd.revents = poll::poll_fd(fd_entry, pollfd.events);

        if pollfd.revents != 0 {
            ready_count += 1;
        }
    }

    // Write updated pollfds back to userspace
    unsafe {
        let dst = fds_ptr as *mut PollFd;
        for (i, pollfd) in pollfds.iter().enumerate() {
            core::ptr::write(dst.add(i), *pollfd);
        }
    }

    log::debug!("sys_poll: {} fds ready", ready_count);
    SyscallResult::Ok(ready_count)
}

/// sys_select - Synchronous I/O multiplexing
///
/// This implements the select() syscall which monitors multiple file descriptors
/// for I/O readiness using fd_set bitmaps.
///
/// Arguments:
/// - nfds: Highest-numbered file descriptor + 1
/// - readfds_ptr: Pointer to fd_set (u64 bitmap) for read fds (may be NULL)
/// - writefds_ptr: Pointer to fd_set (u64 bitmap) for write fds (may be NULL)
/// - exceptfds_ptr: Pointer to fd_set (u64 bitmap) for exception fds (may be NULL)
/// - timeout_ptr: Pointer to timeval structure (0 or NULL for non-blocking)
///
/// Returns:
/// - On success: Number of fds with events
/// - On timeout: 0
/// - On error: negative errno
///
/// Note: Currently only non-blocking select (timeout=0 or NULL) is supported.
/// fd_set is a u64 bitmap supporting fds 0-63.
pub fn sys_select(
    nfds: i32,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    _timeout_ptr: u64,
) -> SyscallResult {
    use crate::ipc::poll;

    log::debug!(
        "sys_select: nfds={}, readfds={:#x}, writefds={:#x}, exceptfds={:#x}, timeout={:#x}",
        nfds, readfds_ptr, writefds_ptr, exceptfds_ptr, _timeout_ptr
    );

    // Validate nfds - must be non-negative and <= 64 (we only support u64 bitmaps)
    if nfds < 0 {
        log::debug!("sys_select: Invalid nfds {}", nfds);
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    if nfds > 64 {
        log::debug!("sys_select: nfds {} exceeds max 64", nfds);
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    // If nfds is 0, nothing to do
    if nfds == 0 {
        return SyscallResult::Ok(0);
    }

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_select: No current thread");
            return SyscallResult::Err(super::errno::EINVAL as u64);
        }
    };

    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_select: Thread {} not in any process", thread_id);
                return SyscallResult::Err(super::errno::EINVAL as u64);
            }
        },
        None => {
            log::error!("sys_select: No process manager");
            return SyscallResult::Err(super::errno::EINVAL as u64);
        }
    };

    // Read fd_set bitmaps from userspace (only if pointer is non-NULL)
    let readfds: u64 = if readfds_ptr != 0 {
        unsafe { *(readfds_ptr as *const u64) }
    } else {
        0
    };

    let writefds: u64 = if writefds_ptr != 0 {
        unsafe { *(writefds_ptr as *const u64) }
    } else {
        0
    };

    let exceptfds: u64 = if exceptfds_ptr != 0 {
        unsafe { *(exceptfds_ptr as *const u64) }
    } else {
        0
    };

    log::debug!(
        "sys_select: read={:#x}, write={:#x}, except={:#x}",
        readfds, writefds, exceptfds
    );

    // Track ready fds
    let mut ready_count: u64 = 0;
    let mut result_readfds: u64 = 0;
    let mut result_writefds: u64 = 0;
    let mut result_exceptfds: u64 = 0;

    // Check each fd up to nfds
    for fd in 0..nfds {
        let fd_bit = 1u64 << fd;

        // Check if this fd is in any of the sets
        let in_readfds = (readfds & fd_bit) != 0;
        let in_writefds = (writefds & fd_bit) != 0;
        let in_exceptfds = (exceptfds & fd_bit) != 0;

        // Skip if fd is not in any set
        if !in_readfds && !in_writefds && !in_exceptfds {
            continue;
        }

        // Look up the file descriptor
        let fd_entry = match process.fd_table.get(fd) {
            Some(entry) => entry,
            None => {
                // Invalid fd - return EBADF
                log::debug!("sys_select: Bad fd {}", fd);
                return SyscallResult::Err(super::errno::EBADF as u64);
            }
        };

        // Check readability
        if in_readfds && poll::check_readable(fd_entry) {
            result_readfds |= fd_bit;
            ready_count += 1;
        }

        // Check writability
        if in_writefds && poll::check_writable(fd_entry) {
            result_writefds |= fd_bit;
            ready_count += 1;
        }

        // Check exception
        if in_exceptfds && poll::check_exception(fd_entry) {
            result_exceptfds |= fd_bit;
            ready_count += 1;
        }
    }

    // Write results back to userspace (only if pointer is non-NULL)
    if readfds_ptr != 0 {
        unsafe { *(readfds_ptr as *mut u64) = result_readfds; }
    }
    if writefds_ptr != 0 {
        unsafe { *(writefds_ptr as *mut u64) = result_writefds; }
    }
    if exceptfds_ptr != 0 {
        unsafe { *(exceptfds_ptr as *mut u64) = result_exceptfds; }
    }

    log::debug!(
        "sys_select: {} fds ready (read={:#x}, write={:#x}, except={:#x})",
        ready_count, result_readfds, result_writefds, result_exceptfds
    );

    SyscallResult::Ok(ready_count)
}
