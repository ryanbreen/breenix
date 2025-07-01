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
    
    // Exit the current process
    crate::process::exit_current(exit_code);
    
    // TODO: Once scheduler integration is complete:
    // - This should trigger a context switch to the next process
    // - Never return to userspace
    
    // Log that we're about to panic (for debugging)
    log::info!("Process will now terminate (panic for demo purposes)");
    
    // For now, we still panic to show the process exited
    // In a real implementation, we'd switch to the next process
    if exit_code == 0 {
        panic!("Process exited successfully");
    } else {
        panic!("Process exited with code: {}", exit_code);
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
/// 
/// Currently just returns since we don't have a scheduler yet.
pub fn sys_yield() -> SyscallResult {
    log::trace!("sys_yield called");
    
    // TODO: Once we have a scheduler:
    // 1. Mark current task as ready
    // 2. Call scheduler to pick next task
    // 3. Context switch if different task selected
    
    // For now, just return success
    SyscallResult::Ok(0)
}

/// sys_get_time - Get current system time in ticks
pub fn sys_get_time() -> SyscallResult {
    let ticks = crate::time::get_ticks();
    log::info!("USERSPACE: sys_get_time called, returning {} ticks", ticks);
    SyscallResult::Ok(ticks)
}