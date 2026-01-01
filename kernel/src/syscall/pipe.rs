//! Pipe syscall implementation
//!
//! Implements the pipe() and pipe2() syscalls for creating unidirectional communication channels.

use super::userptr::copy_to_user;
use super::SyscallResult;
use crate::ipc::fd::{flags, status_flags, FileDescriptor};
use crate::ipc::{create_pipe, FdKind};
use crate::process::manager;

/// sys_pipe - Create a pipe
///
/// Creates a unidirectional data channel that can be used for inter-process communication.
/// Two file descriptors are returned: pipefd[0] is the read end, pipefd[1] is the write end.
///
/// # Arguments
/// * `pipefd_ptr` - Pointer to an array of two i32s to receive the file descriptors
///
/// # Returns
/// * `Ok(0)` on success
/// * `Err(errno)` on failure:
///   - EFAULT (14): Invalid pointer
///   - EMFILE (24): Too many open files
///   - ESRCH (3): Process not found
pub fn sys_pipe(pipefd_ptr: u64) -> SyscallResult {
    log::debug!("sys_pipe: Creating pipe, pipefd_ptr={:#x}", pipefd_ptr);

    // Validate output pointer
    if pipefd_ptr == 0 {
        log::error!("sys_pipe: null pipefd pointer");
        return SyscallResult::Err(14); // EFAULT
    }

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_pipe: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let mut manager_guard = manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_pipe: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_pipe: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Create the pipe buffer (shared between read and write ends)
    let (read_buffer, write_buffer) = create_pipe();

    // Allocate file descriptors for both ends
    let read_fd = match process.fd_table.alloc(FdKind::PipeRead(read_buffer)) {
        Ok(fd) => fd,
        Err(e) => {
            log::error!("sys_pipe: Failed to allocate read fd: {}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    let write_fd = match process.fd_table.alloc(FdKind::PipeWrite(write_buffer)) {
        Ok(fd) => fd,
        Err(e) => {
            // Clean up read fd on failure
            let _ = process.fd_table.close(read_fd);
            log::error!("sys_pipe: Failed to allocate write fd: {}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    // Write the file descriptors to user space
    let pipefd: [i32; 2] = [read_fd, write_fd];
    if let Err(e) = copy_to_user(pipefd_ptr as *mut [i32; 2], &pipefd) {
        // Clean up on failure
        let _ = process.fd_table.close(read_fd);
        let _ = process.fd_table.close(write_fd);
        log::error!("sys_pipe: Failed to copy fds to user: {}", e);
        return SyscallResult::Err(14); // EFAULT
    }

    log::info!(
        "sys_pipe: Created pipe with read_fd={}, write_fd={}",
        read_fd,
        write_fd
    );

    SyscallResult::Ok(0)
}

/// sys_close - Close a file descriptor
///
/// # Arguments
/// * `fd` - The file descriptor to close
///
/// # Returns
/// * `Ok(0)` on success
/// * `Err(errno)` on failure:
///   - EBADF (9): Bad file descriptor
///   - ESRCH (3): Process not found
pub fn sys_close(fd: i32) -> SyscallResult {
    log::debug!("sys_close: Closing fd={}", fd);

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_close: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let mut manager_guard = manager();
    let (process_pid, process) = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((pid, p)) => (pid, p),
            None => {
                log::error!("sys_close: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_close: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    log::debug!(
        "sys_close: thread {} -> process {} '{}', closing fd={}",
        thread_id,
        process_pid.as_u64(),
        process.name,
        fd
    );

    // Close the file descriptor
    match process.fd_table.close(fd) {
        Ok(fd_entry) => {
            // Handle cleanup for specific fd types
            match fd_entry.kind {
                FdKind::PipeRead(buffer) => {
                    // Mark reader as closed
                    buffer.lock().close_read();
                    log::debug!("sys_close: Closed pipe read end fd={}", fd);
                }
                FdKind::PipeWrite(buffer) => {
                    // Mark writer as closed
                    buffer.lock().close_write();
                    log::debug!("sys_close: Closed pipe write end fd={}", fd);
                }
                FdKind::StdIo(_) => {
                    log::debug!("sys_close: Closed stdio fd={}", fd);
                }
                FdKind::UdpSocket(_) => {
                    // Socket unbinding is handled by UdpSocket::Drop when the last
                    // Arc reference is released, allowing shared sockets (via dup/fork)
                    // to remain bound until all references are closed.
                    log::debug!("sys_close: Closed UDP socket fd={}", fd);
                }
            }
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::error!(
                "sys_close: Failed to close fd={} for process {} '{}' (thread {}): error {}",
                fd,
                process_pid.as_u64(),
                process.name,
                thread_id,
                e
            );
            // Log what fds ARE present
            for i in 0..10 {
                if let Some(fd_entry) = process.fd_table.get(i) {
                    log::debug!("  fd_table[{}] = {:?}", i, fd_entry.kind);
                }
            }
            SyscallResult::Err(e as u64)
        }
    }
}

/// sys_pipe2 - Create a pipe with flags
///
/// Like pipe(), but allows setting additional flags on the pipe file descriptors.
///
/// # Arguments
/// * `pipefd_ptr` - Pointer to an array of two i32s to receive the file descriptors
/// * `flags` - Flags to apply to both file descriptors:
///   - O_CLOEXEC (0x80000): Set FD_CLOEXEC on both fds
///   - O_NONBLOCK (0x800): Set O_NONBLOCK status flag on both fds
///
/// # Returns
/// * `Ok(0)` on success
/// * `Err(errno)` on failure:
///   - EFAULT (14): Invalid pointer
///   - EINVAL (22): Invalid flags
///   - EMFILE (24): Too many open files
///   - ESRCH (3): Process not found
pub fn sys_pipe2(pipefd_ptr: u64, pipe_flags: u64) -> SyscallResult {
    log::debug!("sys_pipe2: Creating pipe with flags={:#x}, pipefd_ptr={:#x}", pipe_flags, pipefd_ptr);

    // Validate flags - only O_CLOEXEC and O_NONBLOCK are allowed
    let flags_u32 = pipe_flags as u32;
    let valid_flags = status_flags::O_CLOEXEC | status_flags::O_NONBLOCK;
    if (flags_u32 & !valid_flags) != 0 {
        log::error!("sys_pipe2: Invalid flags {:#x}", flags_u32);
        return SyscallResult::Err(22); // EINVAL
    }

    // Validate output pointer
    if pipefd_ptr == 0 {
        log::error!("sys_pipe2: null pipefd pointer");
        return SyscallResult::Err(14); // EFAULT
    }

    // Get current process
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_pipe2: No current thread");
            return SyscallResult::Err(3); // ESRCH
        }
    };
    let mut manager_guard = manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
            None => {
                log::error!("sys_pipe2: Process not found for thread {}", thread_id);
                return SyscallResult::Err(3); // ESRCH
            }
        },
        None => {
            log::error!("sys_pipe2: Process manager not initialized");
            return SyscallResult::Err(3); // ESRCH
        }
    };

    // Create the pipe buffer (shared between read and write ends)
    let (read_buffer, write_buffer) = create_pipe();

    // Determine fd_flags and status_flags based on pipe_flags argument
    let fd_flags = if (flags_u32 & status_flags::O_CLOEXEC) != 0 {
        flags::FD_CLOEXEC
    } else {
        0
    };
    let status_flags_val = if (flags_u32 & status_flags::O_NONBLOCK) != 0 {
        status_flags::O_NONBLOCK
    } else {
        0
    };

    // Create file descriptors with the appropriate flags
    let read_fd_entry = FileDescriptor::with_flags(
        FdKind::PipeRead(read_buffer),
        fd_flags,
        status_flags_val,
    );
    let write_fd_entry = FileDescriptor::with_flags(
        FdKind::PipeWrite(write_buffer),
        fd_flags,
        status_flags_val,
    );

    // Allocate file descriptors for both ends using alloc_with_entry
    let read_fd = match process.fd_table.alloc_with_entry(read_fd_entry.clone()) {
        Ok(fd) => fd,
        Err(e) => {
            log::error!("sys_pipe2: Failed to allocate read fd: {}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    let write_fd = match process.fd_table.alloc_with_entry(write_fd_entry.clone()) {
        Ok(fd) => fd,
        Err(e) => {
            // Clean up read fd on failure
            let _ = process.fd_table.close(read_fd);
            log::error!("sys_pipe2: Failed to allocate write fd: {}", e);
            return SyscallResult::Err(e as u64);
        }
    };

    // Write the file descriptors to user space
    let pipefd: [i32; 2] = [read_fd, write_fd];
    if let Err(e) = copy_to_user(pipefd_ptr as *mut [i32; 2], &pipefd) {
        // Clean up on failure
        let _ = process.fd_table.close(read_fd);
        let _ = process.fd_table.close(write_fd);
        log::error!("sys_pipe2: Failed to copy fds to user: {}", e);
        return SyscallResult::Err(14); // EFAULT
    }

    log::info!(
        "sys_pipe2: Created pipe with read_fd={}, write_fd={}, flags={:#x}",
        read_fd,
        write_fd,
        flags_u32
    );

    SyscallResult::Ok(0)
}
