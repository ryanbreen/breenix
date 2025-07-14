//! Syscall dispatch table infrastructure
//! 
//! This module provides a table-driven approach for dispatching system calls,
//! replacing the match-based approach with a more efficient and maintainable
//! function pointer table.

use super::handler::SyscallFrame;
use alloc::string::ToString;

/// Maximum number of system calls supported
/// Using 32 instead of 256 to save memory (2KB vs 128B for function pointers)
pub const SYS_MAX: usize = 32;

/// Standard Linux errno values (negative for ABI compatibility)
pub const ENOSYS: isize = -38;  // Function not implemented
pub const EBADF: isize = -9;    // Bad file descriptor
pub const EFAULT: isize = -14;  // Bad address
pub const EINVAL: isize = -22;  // Invalid argument
pub const EPERM: isize = -1;    // Operation not permitted
pub const EIO: isize = -5;      // I/O error
pub const ECHILD: isize = -10;  // No child processes
pub const EINTR: isize = -4;    // Interrupted system call

/// Syscall handler function signature
/// Takes a syscall frame and returns isize (negative for errors, positive for success)
pub type SyscallHandler = fn(&mut SyscallFrame) -> isize;

/// System call dispatch table
/// Using Option<SyscallHandler> to allow unimplemented syscalls to return -ENOSYS
static SYSCALL_TABLE: [Option<SyscallHandler>; SYS_MAX] = {
    let mut table: [Option<SyscallHandler>; SYS_MAX] = [None; SYS_MAX];
    
    // Populate syscall handlers
    table[0] = Some(sys_exit_wrapper as SyscallHandler);      // SYS_EXIT
    table[1] = Some(sys_write_wrapper as SyscallHandler);     // SYS_WRITE
    table[4] = Some(sys_get_time_wrapper as SyscallHandler);  // SYS_GET_TIME
    table[5] = Some(sys_fork_wrapper as SyscallHandler);      // SYS_FORK
    table[11] = Some(sys_exec_wrapper as SyscallHandler);     // SYS_EXEC
    
    table
};

/// Wrapper for sys_write syscall
/// 
/// # Arguments
/// * `frame` - Syscall frame containing arguments:
///   - rdi: file descriptor (1=stdout, 2=stderr)
///   - rsi: buffer pointer (user virtual address)
///   - rdx: buffer length
/// 
/// # Returns
/// * `isize` - Number of bytes written, or negative errno
fn sys_write_wrapper(frame: &mut SyscallFrame) -> isize {
    let fd = frame.rdi as i32;
    let buf_ptr = frame.rsi as usize;
    let len = frame.rdx as usize;
    
    // Validate file descriptor (only stdout and stderr for now)
    if fd != 1 && fd != 2 {
        return EBADF;  // Bad file descriptor
    }
    
    // Validate buffer length
    if len == 0 {
        return 0;  // Nothing to write
    }
    
    // For now, limit write size to prevent abuse
    const MAX_WRITE_SIZE: usize = 4096;
    if len > MAX_WRITE_SIZE {
        return EINVAL;  // Invalid argument
    }
    
    // Read from user buffer and write to serial
    match read_user_buffer_and_write(buf_ptr, len) {
        Ok(bytes_written) => bytes_written as isize,
        Err(errno) => errno,
    }
}

/// Read from user buffer and write to serial output
/// 
/// # Arguments
/// * `buf_ptr` - User virtual address of buffer
/// * `len` - Length of buffer to read
/// 
/// # Returns
/// * `Result<usize, isize>` - Bytes written or negative errno
fn read_user_buffer_and_write(buf_ptr: usize, len: usize) -> Result<usize, isize> {
    // For now, we'll do a simple implementation that reads byte by byte
    // In a production OS, we'd use copy_from_user() with page fault handling
    
    let mut bytes_written = 0;
    
    for i in 0..len {
        let byte_addr = buf_ptr + i;
        
        // Basic address validation - ensure it's in user space
        if byte_addr >= 0x800000000000 {
            return Err(EFAULT);  // Bad address - tried to access kernel space
        }
        
        // Read the byte from user memory
        // SAFETY: We validated the address is in user space
        let byte = unsafe {
            let ptr = byte_addr as *const u8;
            // In a real OS, this would use copy_from_user with page fault handling
            // For now, we'll try a simple read and hope it doesn't page fault
            core::ptr::read_volatile(ptr)
        };
        
        // Write the byte to serial
        crate::serial::write_byte(byte);
        bytes_written += 1;
    }
    
    Ok(bytes_written)
}

/// Wrapper for sys_exit syscall
/// 
/// # Arguments
/// * `frame` - Syscall frame containing arguments:
///   - rdi: exit code (0-255)
/// 
/// # Returns
/// * `isize` - This function never returns (process terminates)
fn sys_exit_wrapper(frame: &mut SyscallFrame) -> isize {
    let exit_code = frame.rdi as i32;
    
    // Validate exit code range (0-255 is standard for Unix exit codes)
    let exit_code = if exit_code < 0 || exit_code > 255 {
        1 // Default to error exit code for invalid values
    } else {
        exit_code
    };
    
    log::info!("Process {} exiting with code {}", 
        crate::task::scheduler::current_thread_id().unwrap_or(0), exit_code);
    
    // Terminate the current process
    terminate_current_process(exit_code);
}

/// Terminate the current process with the given exit code
/// 
/// # Arguments
/// * `exit_code` - Exit code (0-255)
fn terminate_current_process(exit_code: i32) -> ! {
    // Get current thread ID
    let thread_id = crate::task::scheduler::current_thread_id().unwrap_or(0);
    
    log::info!("Terminating process {} with exit code {}", thread_id, exit_code);
    
    // Mark the thread as terminated
    // For now, we'll do a simple implementation that just removes the thread from scheduling
    // In a full implementation, we would:
    // 1. Clean up process resources (memory, file handles, etc.)
    // 2. Notify parent process (if any)
    // 3. Handle zombie state management
    // 4. Wake up processes waiting on this one
    
    // Remove thread from scheduler
    crate::task::scheduler::terminate_current_thread();
}

/// Wrapper for sys_get_time syscall
/// 
/// Returns the current time in timer ticks since boot.
/// This is a simple syscall that takes no arguments and returns the current
/// timer tick count as a u64 value.
/// 
/// # Arguments
/// * `frame` - Syscall frame (no arguments needed)
/// 
/// # Returns
/// * `isize` - Current timer ticks (always positive)
fn sys_get_time_wrapper(_frame: &mut SyscallFrame) -> isize {
    // Get current timer ticks
    let ticks = crate::time::get_ticks();
    
    // Return as isize (timer ticks are always positive)
    ticks as isize
}

/// Wrapper for sys_fork syscall
/// 
/// Creates a new process by duplicating the current process.
/// The parent process receives the child's PID, and the child receives 0.
/// 
/// # Arguments
/// * `frame` - Syscall frame containing the current execution context
/// 
/// # Returns
/// * `isize` - Child PID for parent, 0 for child, negative errno on error
fn sys_fork_wrapper(frame: &mut SyscallFrame) -> isize {
    // Call the existing sys_fork_with_frame implementation
    match crate::syscall::handlers::sys_fork_with_frame(frame) {
        crate::syscall::SyscallResult::Ok(value) => value as isize,
        crate::syscall::SyscallResult::Err(errno) => -(errno as isize),
    }
}

/// Wrapper for sys_exec syscall
/// 
/// Replaces the current process with a new program.
/// This syscall never returns on success (the process is replaced).
/// 
/// # Arguments
/// * `frame` - Syscall frame containing arguments:
///   - rdi: program name pointer (user virtual address)
///   - rsi: arguments pointer (user virtual address)
/// 
/// # Returns
/// * `isize` - 0 on success (but control never returns to caller), negative errno on error
fn sys_exec_wrapper(frame: &mut SyscallFrame) -> isize {
    let _program_name_ptr = frame.rdi;
    let _args_ptr = frame.rsi;
    
    // For now, use a hardcoded program for testing
    // In a real implementation, we'd parse the program name and load from filesystem
    
    #[cfg(feature = "testing")]
    {
        // Use exec_target for testing
        log::info!("sys_exec_wrapper: Calling exec_replace with exec_target");
        // exec_replace returns 0 on success, but control never returns to the original caller
        // The interrupt return path will use the new context
        crate::syscall::exec::exec_replace(
            alloc::string::String::from("exec_target"),
            crate::userspace_test::EXEC_TARGET_ELF
        )
    }
    
    #[cfg(not(feature = "testing"))]
    {
        log::error!("sys_exec_wrapper: exec not implemented without testing feature");
        ENOSYS // Not implemented
    }
}

/// Dispatch a system call by number
/// 
/// # Arguments
/// * `nr` - System call number
/// * `frame` - Syscall frame with registers and arguments
/// 
/// # Returns
/// * `isize` - Return value (negative for errors, following Linux ABI)
pub fn dispatch(nr: usize, frame: &mut SyscallFrame) -> isize {
    // Bounds check
    if nr >= SYS_MAX {
        return ENOSYS;
    }
    
    // Handle test syscall for Phase 4A compatibility
    if nr == 0x1234 {
        log::warn!("SYSCALL_OK"); // Emit test marker
        log::info!("Test syscall 0x1234 received - Phase 4A syscall gate working!");
        return 0x5678; // Return test value
    }
    
    // Dispatch to handler
    match SYSCALL_TABLE[nr] {
        Some(handler) => handler(frame),
        None => ENOSYS,
    }
}

/// Register a system call handler
/// 
/// # Arguments
/// * `nr` - System call number
/// * `handler` - Handler function
/// 
/// # Returns
/// * `Result<(), &'static str>` - Success or error message
pub fn register_syscall(nr: usize, _handler: SyscallHandler) -> Result<(), &'static str> {
    if nr >= SYS_MAX {
        return Err("Syscall number exceeds SYS_MAX");
    }
    
    // For now, we use a static table so we can't modify it at runtime
    // In a future PR, we could use OnceLock<Vec<Option<SyscallHandler>>> for dynamic registration
    Err("Dynamic syscall registration not yet implemented - use static table")
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test_case]
    fn test_dispatch_bounds_check() {
        // Create a dummy frame
        let mut frame = SyscallFrame {
            rax: 0, rcx: 0, rdx: 0, rbx: 0, rbp: 0, rsi: 0, rdi: 0,
            r8: 0, r9: 0, r10: 0, r11: 0, r12: 0, r13: 0, r14: 0, r15: 0,
            rip: 0, cs: 0, rflags: 0, rsp: 0, ss: 0,
        };
        
        // Test out of bounds
        assert_eq!(dispatch(SYS_MAX, &mut frame), ENOSYS);
        assert_eq!(dispatch(9999, &mut frame), ENOSYS);
    }
    
    #[test_case]
    fn test_dispatch_unimplemented() {
        let mut frame = SyscallFrame {
            rax: 0, rcx: 0, rdx: 0, rbx: 0, rbp: 0, rsi: 0, rdi: 0,
            r8: 0, r9: 0, r10: 0, r11: 0, r12: 0, r13: 0, r14: 0, r15: 0,
            rip: 0, cs: 0, rflags: 0, rsp: 0, ss: 0,
        };
        
        // Test unimplemented syscall
        assert_eq!(dispatch(5, &mut frame), ENOSYS);
    }
    
    #[test_case]
    fn test_dispatch_test_syscall() {
        let mut frame = SyscallFrame {
            rax: 0, rcx: 0, rdx: 0, rbx: 0, rbp: 0, rsi: 0, rdi: 0,
            r8: 0, r9: 0, r10: 0, r11: 0, r12: 0, r13: 0, r14: 0, r15: 0,
            rip: 0, cs: 0, rflags: 0, rsp: 0, ss: 0,
        };
        
        // Test Phase 4A test syscall
        assert_eq!(dispatch(0x1234, &mut frame), 0x5678);
    }
}