//! System call handler implementations
//! 
//! This module contains the actual implementation of each system call.

use super::{SyscallResult, SyscallError};
use core::slice;

/// File descriptors
#[allow(dead_code)]
const FD_STDIN: u64 = 0;
const FD_STDOUT: u64 = 1;
const FD_STDERR: u64 = 2;

/// sys_exit - Terminate the current task
/// 
/// For now, this just halts the system since we don't have proper task management yet.
#[allow(dead_code)]
pub fn sys_exit(exit_code: u64) -> SyscallResult {
    log::info!("sys_exit called with code: {}", exit_code);
    
    // TODO: Once we have proper task management:
    // 1. Mark current task as terminated
    // 2. Clean up task resources
    // 3. Schedule next task
    // 4. Never return
    
    // For now, just halt
    log::info!("System halting (no task management yet)");
    loop {
        x86_64::instructions::hlt();
    }
}

/// sys_write - Write to a file descriptor
/// 
/// Currently only supports stdout/stderr writing to serial port.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
    // Validate file descriptor
    if fd != FD_STDOUT && fd != FD_STDERR {
        return Err(SyscallError::InvalidArgument);
    }
    
    // Validate buffer pointer and count
    if buf_ptr == 0 || count == 0 {
        return Ok(0);
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
    
    Ok(bytes_written)
}

/// sys_read - Read from a file descriptor
/// 
/// Currently returns 0 (no data available) as keyboard is async-only.
#[allow(dead_code)]
pub fn sys_read(fd: u64, _buf_ptr: u64, _count: u64) -> SyscallResult {
    // Validate file descriptor
    if fd != FD_STDIN {
        return Err(SyscallError::InvalidArgument);
    }
    
    // TODO: Implement synchronous keyboard reading
    // For now, always return 0 (no data available)
    Ok(0)
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
    Ok(0)
}

/// sys_get_time - Get current system time in ticks
pub fn sys_get_time() -> SyscallResult {
    let ticks = crate::time::get_ticks();
    Ok(ticks)
}