//! Pipe syscall implementation
//!
//! Implements the pipe() syscall for creating unidirectional communication channels.

use super::userptr::copy_to_user;
use super::SyscallResult;
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
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
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
                FdKind::UdpSocket(socket_ref) => {
                    // Unbind the socket if it was bound
                    let socket = socket_ref.lock();
                    if let Some(port) = socket.local_port {
                        crate::socket::SOCKET_REGISTRY.unbind_udp(port);
                        log::debug!("sys_close: Closed UDP socket fd={}, unbound port {}", fd, port);
                    } else {
                        log::debug!("sys_close: Closed unbound UDP socket fd={}", fd);
                    }
                }
            }
            SyscallResult::Ok(0)
        }
        Err(e) => {
            log::error!("sys_close: Failed to close fd={}: error {}", fd, e);
            SyscallResult::Err(e as u64)
        }
    }
}
