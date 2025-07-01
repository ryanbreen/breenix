//! System call handler implementations
//! 
//! This module contains the actual implementation of each system call.

use super::SyscallResult;
use core::slice;

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
        // Handle thread exit through ProcessScheduler
        crate::task::process_task::ProcessScheduler::handle_thread_exit(thread_id, exit_code);
        
        // Mark current thread as terminated
        crate::task::scheduler::with_scheduler(|scheduler| {
            if let Some(thread) = scheduler.current_thread_mut() {
                thread.set_terminated();
            }
        });
        
        // Yield to scheduler to pick next thread
        crate::task::scheduler::yield_current();
        
        // The scheduler should switch to another thread
        // If we get here, something went wrong
        log::error!("sys_exit: Failed to switch to next thread");
    } else {
        log::error!("sys_exit: No current thread in scheduler");
    }
    
    // If we get here, there are no more processes to run
    log::info!("No more processes to run, returning to kernel");
    
    // For now, we still panic to show the process exited
    if exit_code == 0 {
        panic!("Last process exited successfully");
    } else {
        panic!("Last process exited with code: {}", exit_code);
    }
}

/// Perform context switch after process exit
/// This should never return if there's another process to run
fn perform_process_exit_switch() {
    // Check if there's another process ready to run
    if let Some(ref mut manager) = *crate::process::manager() {
        if let Some(next_pid) = manager.schedule_next() {
            log::info!("Switching to next process (PID {})", next_pid.as_u64());
            
            // Get the process info
            if let Some(process) = manager.get_process(next_pid) {
                if let Some(ref thread) = process.main_thread {
                    // Prepare for context switch
                    unsafe {
                        // Get selectors
                        let user_cs = crate::gdt::USER_CODE_SELECTOR.0 | 3;
                        let user_ds = crate::gdt::USER_DATA_SELECTOR.0 | 3;
                        
                        // Note: In a real implementation, we'd restore the thread's saved context
                        // For now, we assume the process hasn't been run before
                        log::info!("Switching to process at {:#x}", process.entry_point);
                        
                        // This will switch to the new process and never return
                        crate::task::userspace_switch::switch_to_userspace(
                            process.entry_point,
                            thread.stack_top,
                            user_cs,
                            user_ds,
                        );
                    }
                }
            }
        } else {
            log::info!("No ready processes in queue");
        }
    }
}

/// sys_write - Write to a file descriptor
/// 
/// Currently only supports stdout/stderr writing to serial port.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
    log::debug!("sys_write: fd={}, buf_ptr={:#x}, count={}", fd, buf_ptr, count);
    
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
    log::trace!("sys_yield called");
    
    // Yield to the scheduler
    crate::task::scheduler::yield_current();
    
    // If we return here, either we're still the best thread to run
    // or the context switch will happen when we return from the syscall
    SyscallResult::Ok(0)
}

/// sys_get_time - Get current system time in ticks
pub fn sys_get_time() -> SyscallResult {
    let ticks = crate::time::get_ticks();
    log::info!("USERSPACE: sys_get_time called, returning {} ticks", ticks);
    SyscallResult::Ok(ticks)
}