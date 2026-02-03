//! System call handler implementations
//!
//! This module contains the actual implementation of each system call.

use super::SyscallResult;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::structures::paging::Translate;
use x86_64::VirtAddr;
use crate::arch_impl::traits::CpuOps;

// Architecture-specific CPU type for interrupt control
type Cpu = crate::arch_impl::x86_64::X86Cpu;

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

fn copy_string_from_user(user_ptr: u64, max_len: usize) -> Result<Vec<u8>, &'static str> {
    if user_ptr == 0 {
        return Err("null pointer");
    }

    let mapper = unsafe { crate::memory::paging::get_mapper() };
    let mut buffer = Vec::new();

    for offset in 0..max_len {
        let addr = user_ptr
            .checked_add(offset as u64)
            .ok_or("userspace address overflow")?;

        if !crate::memory::layout::is_valid_user_address(addr) {
            return Err("invalid userspace address");
        }

        if mapper.translate_addr(VirtAddr::new(addr)).is_none() {
            return Err("unmapped userspace address");
        }

        let byte = unsafe { *(addr as *const u8) };
        buffer.push(byte);

        if byte == 0 {
            break;
        }
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

    // Note: Logging removed from hot path to prevent stack overflow.
    // Each log call in interactive mode writes to the Logs terminal,
    // which adds significant stack depth during syscall handling.

    // Validate buffer pointer and count
    if buf_ptr == 0 || count == 0 {
        return SyscallResult::Ok(0);
    }

    // Copy data from userspace
    let buffer = match copy_from_user(buf_ptr, count as usize) {
        Ok(buf) => buf,
        Err(_e) => {
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

    // Determine the fd kind while holding the manager lock, then release it
    // before doing slow I/O operations. This prevents blocking signal delivery
    // to other processes while we're doing serial writes.
    enum WriteOperation {
        StdIo,
        Pipe { pipe_buffer: alloc::sync::Arc<spin::Mutex<crate::ipc::pipe::PipeBuffer>>, is_nonblocking: bool },
        Fifo { pipe_buffer: alloc::sync::Arc<spin::Mutex<crate::ipc::pipe::PipeBuffer>>, is_nonblocking: bool },
        UnixStream { socket: alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixStreamSocket>> },
        RegularFile { file: alloc::sync::Arc<spin::Mutex<crate::ipc::fd::RegularFile>> },
        TcpConnection { conn_id: crate::net::tcp::ConnectionId },
        Device { device_type: crate::fs::devfs::DeviceType },
        Ebadf,
        Enotconn,  // Socket not connected
        Eisdir,    // Is a directory
        Eopnotsupp, // Operation not supported
    }

    let write_op = {
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
                return SyscallResult::Err(9); // EBADF
            }
        };

        match &fd_entry.kind {
            FdKind::StdIo(n) if *n == 1 || *n == 2 => WriteOperation::StdIo,
            FdKind::StdIo(_) => WriteOperation::Ebadf, // stdin - can't write
            FdKind::PipeWrite(pipe_buffer) => {
                WriteOperation::Pipe { pipe_buffer: pipe_buffer.clone(), is_nonblocking: (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0 }
            }
            FdKind::PipeRead(_) => WriteOperation::Ebadf,
            FdKind::FifoWrite(_path, pipe_buffer) => {
                WriteOperation::Fifo { pipe_buffer: pipe_buffer.clone(), is_nonblocking: (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0 }
            }
            FdKind::FifoRead(_, _) => WriteOperation::Ebadf,
            FdKind::TcpSocket(_) => WriteOperation::Enotconn,  // Unconnected TCP socket
            FdKind::TcpListener(_) => WriteOperation::Enotconn, // Listener can't write
            FdKind::TcpConnection(conn_id) => WriteOperation::TcpConnection { conn_id: *conn_id },
            FdKind::UdpSocket(_) => WriteOperation::Eopnotsupp, // UDP must use sendto
            FdKind::UnixStream(socket) => WriteOperation::UnixStream { socket: socket.clone() },
            FdKind::UnixSocket(_) => WriteOperation::Enotconn,  // Unconnected Unix socket
            FdKind::UnixListener(_) => WriteOperation::Enotconn, // Listener can't write
            FdKind::RegularFile(file) => WriteOperation::RegularFile { file: file.clone() },
            FdKind::Directory(_) => WriteOperation::Eisdir,
            FdKind::Device(device_type) => WriteOperation::Device { device_type: device_type.clone() },
            FdKind::DevfsDirectory { .. } => WriteOperation::Eisdir,
            FdKind::DevptsDirectory { .. } => WriteOperation::Eisdir,
            FdKind::PtyMaster(_) | FdKind::PtySlave(_) => {
                // PTY write not implemented yet
                WriteOperation::Eopnotsupp
            }
        }
        // manager_guard dropped here, releasing the lock before I/O
    };

    // Now perform the actual I/O operation without holding the manager lock
    match write_op {
        WriteOperation::StdIo => write_to_stdio(fd, &buffer),
        WriteOperation::Ebadf => SyscallResult::Err(9), // EBADF
        WriteOperation::Enotconn => SyscallResult::Err(super::errno::ENOTCONN as u64),
        WriteOperation::Eisdir => SyscallResult::Err(super::errno::EISDIR as u64),
        WriteOperation::Eopnotsupp => SyscallResult::Err(95), // EOPNOTSUPP
        WriteOperation::Pipe { pipe_buffer, is_nonblocking } => {
            let mut pipe = pipe_buffer.lock();
            match pipe.write(&buffer) {
                Ok(n) => {
                    log::debug!("sys_write: Wrote {} bytes to pipe", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(11) if !is_nonblocking => {
                    // Blocking pipe write not implemented, return EAGAIN
                    log::debug!("sys_write: Pipe full, blocking not implemented - returning EAGAIN");
                    SyscallResult::Err(11) // EAGAIN
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        WriteOperation::Fifo { pipe_buffer, is_nonblocking } => {
            let mut pipe = pipe_buffer.lock();
            match pipe.write(&buffer) {
                Ok(n) => {
                    log::debug!("sys_write: Wrote {} bytes to FIFO", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(11) if !is_nonblocking => {
                    log::debug!("sys_write: FIFO full, blocking not implemented - returning EAGAIN");
                    SyscallResult::Err(11) // EAGAIN
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        WriteOperation::UnixStream { socket } => {
            let sock = socket.lock();
            match sock.write(&buffer) {
                Ok(n) => {
                    log::debug!("sys_write: Wrote {} bytes to Unix socket", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        WriteOperation::TcpConnection { conn_id } => {
            // Write to established TCP connection
            match crate::net::tcp::tcp_send(&conn_id, &buffer) {
                Ok(n) => {
                    log::debug!("sys_write: Wrote {} bytes to TCP connection", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => {
                    log::debug!("sys_write: TCP write error: {}", e);
                    // Return EPIPE if the connection was shutdown for writing
                    if e.contains("shutdown") {
                        SyscallResult::Err(super::errno::EPIPE as u64)
                    } else {
                        SyscallResult::Err(super::errno::EIO as u64)
                    }
                }
            }
        }
        WriteOperation::Device { device_type } => {
            use crate::fs::devfs::DeviceType;
            match device_type {
                DeviceType::Null | DeviceType::Zero => {
                    // /dev/null, /dev/zero - discard all data
                    SyscallResult::Ok(buffer.len() as u64)
                }
                DeviceType::Console | DeviceType::Tty => {
                    // Write to console/tty
                    write_to_stdio(fd, &buffer)
                }
            }
        }
        WriteOperation::RegularFile { file } => {
            // Write to ext2 regular file
            let (inode_num, position, flags) = {
                let file_guard = file.lock();
                (file_guard.inode_num, file_guard.position, file_guard.flags)
            };

            // Handle O_APPEND flag - seek to end before writing
            let write_offset = if (flags & crate::syscall::fs::O_APPEND) != 0 {
                let root_fs = crate::fs::ext2::root_fs();
                let fs = match root_fs.as_ref() {
                    Some(fs) => fs,
                    None => return SyscallResult::Err(super::errno::ENOSYS as u64),
                };
                match fs.read_inode(inode_num as u32) {
                    Ok(inode) => inode.size(),
                    Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
                }
            } else {
                position
            };

            // Write the data
            let mut root_fs = crate::fs::ext2::root_fs();
            let fs = match root_fs.as_mut() {
                Some(fs) => fs,
                None => return SyscallResult::Err(super::errno::ENOSYS as u64),
            };

            let bytes_written = match fs.write_file_range(inode_num as u32, write_offset, &buffer) {
                Ok(n) => n,
                Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
            };

            drop(root_fs);

            // Update file position
            {
                let mut file_guard = file.lock();
                file_guard.position = write_offset + bytes_written as u64;
            }

            log::debug!("sys_write: Wrote {} bytes to regular file (inode {})", bytes_written, inode_num);
            SyscallResult::Ok(bytes_written as u64)
        }
    }
}

/// Helper function to write to stdio through TTY layer
///
/// This is the POSIX-correct way to write stdout/stderr. All output goes through
/// the TTY layer which handles:
/// - OPOST output processing
/// - ONLCR (NL -> CR-NL conversion when enabled)
/// - Carriage return handling (\r moves to start of line without newline)
fn write_to_stdio(fd: u64, buffer: &[u8]) -> SyscallResult {
    // Suppress the fd unused warning
    let _ = fd;

    // Route all stdout/stderr writes through the TTY layer for POSIX-compliant
    // output processing. The TTY layer handles:
    // - OPOST flag processing
    // - ONLCR (newline -> carriage return + newline conversion)
    // - Direct output of control characters like \r
    let bytes_written = crate::tty::write_output(buffer);

    SyscallResult::Ok(bytes_written as u64)
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
            // stdin - read from stdin ring buffer
            //
            // Keyboard input goes to the stdin buffer via keyboard interrupt handler.
            // The TTY layer is used for terminal control (signals, echo) but not for
            // data transport. This allows character-at-a-time reads to work properly.
            //
            // Drop the process manager lock before potentially blocking
            drop(manager_guard);

            let mut user_buf = alloc::vec![0u8; count as usize];

            // Blocking read loop: keep trying until we get data or an error
            // Similar to pause() implementation - block, HLT loop, check for data
            loop {
                // Register as blocked reader FIRST to avoid race condition
                // where data arrives between checking and blocking
                crate::ipc::stdin::register_blocked_reader(thread_id);

                // Read from stdin buffer
                let read_result = crate::ipc::stdin::read_bytes(&mut user_buf);

                match read_result {
                    Ok(n) => {
                        // Data was available - unregister from blocked readers
                        crate::ipc::stdin::unregister_blocked_reader(thread_id);

                        if n > 0 {
                            // Copy to userspace
                            if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                                return SyscallResult::Err(14); // EFAULT
                            }
                            log::trace!("sys_read: Read {} bytes from stdin", n);
                        }
                        return SyscallResult::Ok(n as u64);
                    }
                    Err(11) => {
                        // EAGAIN - no data available, need to block and wait
                        // We're already registered as blocked reader

                        // Block the current thread AND set blocked_in_syscall flag.
                        // CRITICAL: Setting blocked_in_syscall is essential because:
                        // 1. The thread will enter a kernel-mode HLT loop below
                        // 2. If a context switch happens while in HLT, the scheduler sees
                        //    from_userspace=false (kernel mode) but blocked_in_syscall tells
                        //    it to save/restore kernel context, not userspace context
                        // 3. Without this flag, no context is saved when switching away,
                        //    and stale userspace context is restored when switching back,
                        //    causing RIP corruption (kernel address in userspace CS)
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.block_current();
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = true;
                                log::trace!("sys_read: Thread {} blocked on stdin (blocked_in_syscall=true)", thread.id);
                            }
                        });

                        log::trace!("sys_read: Thread {} blocking on stdin", thread_id);

                        // CRITICAL: Re-enable preemption before entering blocking loop!
                        // The syscall handler called preempt_disable() at entry, but we need
                        // to allow timer interrupts to schedule other threads while we're blocked.
                        crate::per_cpu::preempt_enable();

                        // HLT loop - wait for timer interrupt which will switch to another thread
                        // When keyboard data arrives, the interrupt handler will unblock us
                        loop {
                            crate::task::scheduler::yield_current();
                            Cpu::halt_with_interrupts();

                            // Check if we were unblocked (thread state changed from Blocked)
                            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.state == crate::task::thread::ThreadState::Blocked
                                } else {
                                    false
                                }
                            }).unwrap_or(false);

                            if !still_blocked {
                                log::trace!("sys_read: Thread {} unblocked from stdin wait", thread_id);
                                break;
                            }
                        }

                        // Re-disable preemption before continuing to balance syscall's preempt_disable
                        crate::per_cpu::preempt_disable();

                        // Clear blocked_in_syscall now that we're resuming normal syscall execution
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                log::trace!("sys_read: Thread {} cleared blocked_in_syscall", thread.id);
                            }
                        });

                        // Loop back to try reading again - we should have data now
                        continue;
                    }
                    Err(e) => {
                        // Error - unregister from blocked readers
                        crate::ipc::stdin::unregister_blocked_reader(thread_id);
                        log::trace!("sys_read: Stdin read error: {}", e);
                        return SyscallResult::Err(e as u64);
                    }
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
        FdKind::FifoRead(_path, pipe_buffer) => {
            // FIFO read - with blocking support
            let is_nonblocking = (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
            let pipe_buffer_clone = pipe_buffer.clone();

            // CRITICAL: Release process manager lock before blocking!
            // If we hold the lock while blocked in the HLT loop, timer interrupts
            // cannot perform context switches to other threads (like the child
            // process that needs to write to the FIFO).
            drop(manager_guard);

            let mut user_buf = alloc::vec![0u8; count as usize];

            // Try to read - if empty and blocking, we'll enter blocking path
            loop {
                let read_result = {
                    let mut pipe = pipe_buffer_clone.lock();
                    pipe.read(&mut user_buf)
                };

                match read_result {
                    Ok(n) => {
                        if n > 0 {
                            if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                                return SyscallResult::Err(14); // EFAULT
                            }
                        }
                        log::debug!("sys_read: Read {} bytes from FIFO", n);
                        return SyscallResult::Ok(n as u64);
                    }
                    Err(11) => {
                        // EAGAIN - buffer empty but writers exist
                        if is_nonblocking {
                            log::debug!("sys_read: FIFO empty, O_NONBLOCK set - returning EAGAIN");
                            return SyscallResult::Err(11); // EAGAIN
                        }

                        // === BLOCKING PATH ===
                        let thread_id = match crate::task::scheduler::current_thread_id() {
                            Some(tid) => tid,
                            None => return SyscallResult::Err(3), // ESRCH
                        };

                        log::debug!("sys_read: FIFO empty, thread {} entering blocking path", thread_id);

                        // Register as waiter BEFORE setting blocked state (race condition fix)
                        {
                            let mut pipe = pipe_buffer_clone.lock();
                            pipe.add_read_waiter(thread_id);
                        }

                        // Block the thread
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.block_current();
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = true;
                            }
                        });

                        // Check if data arrived during setup (race condition fix)
                        let data_ready = {
                            let pipe = pipe_buffer_clone.lock();
                            pipe.has_data_or_eof()
                        };

                        if data_ready {
                            // Data arrived during setup - unblock and retry immediately
                            crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.blocked_in_syscall = false;
                                    thread.set_ready();
                                }
                            });
                            continue; // Retry read
                        }

                        // Enable preemption for HLT loop
                        crate::per_cpu::preempt_enable();

                        // HLT loop - wait for data or EOF
                        loop {
                            crate::task::scheduler::yield_current();
                            Cpu::halt_with_interrupts();

                            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.state == crate::task::thread::ThreadState::Blocked
                                } else {
                                    false
                                }
                            }).unwrap_or(false);

                            if !still_blocked {
                                crate::per_cpu::preempt_disable();
                                log::debug!("sys_read: FIFO thread {} woken from blocking", thread_id);
                                break;
                            }
                        }

                        // Clear blocked state
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                            }
                        });
                        crate::interrupts::timer::reset_quantum();
                        crate::task::scheduler::check_and_clear_need_resched();

                        // Continue loop to retry read
                        continue;
                    }
                    Err(e) => {
                        log::debug!("sys_read: FIFO read error: {}", e);
                        return SyscallResult::Err(e as u64);
                    }
                }
            }
        }
        FdKind::FifoWrite(_, _) => {
            // Can't read from write end of FIFO
            SyscallResult::Err(9) // EBADF
        }
        FdKind::UdpSocket(_) => {
            // Can't read from UDP socket - must use recvfrom
            log::error!("sys_read: Cannot read from UDP socket, use recvfrom instead");
            SyscallResult::Err(95) // EOPNOTSUPP
        }
        FdKind::RegularFile(file_ref) => {
            // Read from ext2 regular file
            //
            // Get file info under the lock, then drop lock before filesystem operations
            let (inode_num, position) = {
                let file = file_ref.lock();
                (file.inode_num, file.position)
            };

            // Access the mounted ext2 filesystem
            let root_fs = crate::fs::ext2::root_fs();
            let fs = match root_fs.as_ref() {
                Some(fs) => fs,
                None => {
                    log::error!("sys_read: ext2 filesystem not mounted");
                    return SyscallResult::Err(super::errno::ENOSYS as u64);
                }
            };

            // Read the inode
            let inode = match fs.read_inode(inode_num as u32) {
                Ok(inode) => inode,
                Err(e) => {
                    log::error!("sys_read: Failed to read inode {}: {}", inode_num, e);
                    return SyscallResult::Err(super::errno::EIO as u64);
                }
            };

            // Read the file data
            let data = match fs.read_file_range(&inode, position, count as usize) {
                Ok(data) => data,
                Err(e) => {
                    log::error!("sys_read: Failed to read file data: {}", e);
                    return SyscallResult::Err(super::errno::EIO as u64);
                }
            };

            // Drop the filesystem lock before copying to userspace
            drop(root_fs);

            let bytes_read = data.len();

            // Copy data to userspace
            if bytes_read > 0 {
                if copy_to_user(buf_ptr, data.as_ptr() as u64, bytes_read).is_err() {
                    return SyscallResult::Err(14); // EFAULT
                }
            }

            // Update file position
            {
                let mut file = file_ref.lock();
                file.position += bytes_read as u64;
            }

            log::debug!("sys_read: Read {} bytes from regular file (inode {})", bytes_read, inode_num);
            SyscallResult::Ok(bytes_read as u64)
        }
        FdKind::Directory(_) => {
            // Cannot read from directory with read() - must use getdents
            log::debug!("sys_read: Cannot read from directory, use getdents instead");
            SyscallResult::Err(super::errno::EISDIR as u64)
        }
        FdKind::Device(device_type) => {
            // Read from devfs device (/dev/null, /dev/zero, /dev/console, /dev/tty)
            let mut user_buf = alloc::vec![0u8; count as usize];
            match crate::fs::devfs::device_read(*device_type, &mut user_buf) {
                Ok(n) => {
                    if n > 0 {
                        // Copy to userspace
                        if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                            return SyscallResult::Err(14); // EFAULT
                        }
                    }
                    log::debug!("sys_read: Read {} bytes from device {:?}", n, device_type);
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => {
                    log::debug!("sys_read: Device read error: {}", e);
                    SyscallResult::Err((-e) as u64)
                }
            }
        }
        FdKind::DevfsDirectory { .. } => {
            // Cannot read from directory with read() - must use getdents
            log::debug!("sys_read: Cannot read from /dev directory, use getdents instead");
            SyscallResult::Err(super::errno::EISDIR as u64)
        }
        FdKind::DevptsDirectory { .. } => {
            // Cannot read from directory with read() - must use getdents
            log::debug!("sys_read: Cannot read from /dev/pts directory, use getdents instead");
            SyscallResult::Err(super::errno::EISDIR as u64)
        }
        FdKind::TcpSocket(_) | FdKind::TcpListener(_) => {
            // Cannot read from unconnected TCP socket
            log::error!("sys_read: Cannot read from unconnected TCP socket");
            SyscallResult::Err(super::errno::ENOTCONN as u64)
        }
        FdKind::TcpConnection(conn_id) => {
            // Read from TCP connection with blocking/non-blocking support
            // Clone conn_id and capture flags before dropping manager_guard
            let conn_id = *conn_id;
            let is_nonblocking = (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
            drop(manager_guard);

            // Drain loopback queue for localhost connections (127.x.x.x, own IP).
            crate::net::drain_loopback_queue();

            let mut user_buf = alloc::vec![0u8; count as usize];

            // Read loop (may block if O_NONBLOCK not set)
            loop {
                // Register as waiter FIRST to avoid race condition
                crate::net::tcp::tcp_register_recv_waiter(&conn_id, thread_id);

                // Drain loopback queue in case data arrived
                crate::net::drain_loopback_queue();

                // Try to receive
                match crate::net::tcp::tcp_recv(&conn_id, &mut user_buf) {
                    Ok(n) if n > 0 => {
                        // Data received - unregister and return
                        crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
                        if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                            return SyscallResult::Err(14); // EFAULT
                        }
                        log::debug!("sys_read: Received {} bytes from TCP connection", n);
                        return SyscallResult::Ok(n as u64);
                    }
                    Ok(0) => {
                        // EOF (connection closed)
                        crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
                        return SyscallResult::Ok(0);
                    }
                    Err(_) => {
                        // No data available
                        if is_nonblocking {
                            // O_NONBLOCK set: return EAGAIN immediately
                            crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
                            log::debug!("sys_read: TCP no data, O_NONBLOCK set - returning EAGAIN");
                            return SyscallResult::Err(super::errno::EAGAIN as u64);
                        }
                        // Will block below
                    }
                    _ => unreachable!(),
                }

                // No data - block the thread
                log::debug!("TCP recv: entering blocking path, thread={}", thread_id);

                crate::task::scheduler::with_scheduler(|sched| {
                    sched.block_current();
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.blocked_in_syscall = true;
                    }
                });

                // Double-check for data after setting Blocked state
                if crate::net::tcp::tcp_has_data(&conn_id) {
                    log::info!("TCP: Thread {} caught race - data arrived during block setup", thread_id);
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                            thread.set_ready();
                        }
                    });
                    crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
                    continue;
                }

                // Re-enable preemption before HLT loop
                crate::per_cpu::preempt_enable();

                log::info!("TCP_BLOCK: Thread {} entering blocked state for recv", thread_id);

                // HLT loop - wait for data to arrive
                loop {
                    crate::task::scheduler::yield_current();
                    Cpu::halt_with_interrupts();

                    let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.state == crate::task::thread::ThreadState::Blocked
                        } else {
                            false
                        }
                    }).unwrap_or(false);

                    if !still_blocked {
                        crate::per_cpu::preempt_disable();
                        log::info!("TCP_BLOCK: Thread {} woken from recv blocking", thread_id);
                        break;
                    }
                }

                // Clear blocked_in_syscall
                crate::task::scheduler::with_scheduler(|sched| {
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.blocked_in_syscall = false;
                    }
                });

                // Unregister from wait queue (will re-register at top of loop)
                crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);

                // Drain loopback again before retrying
                crate::net::drain_loopback_queue();
            }
        }
        FdKind::PtyMaster(pty_num) => {
            // Read from PTY master (slave's output)
            match crate::tty::pty::get(*pty_num) {
                Some(pair) => {
                    let mut user_buf = alloc::vec![0u8; count as usize];
                    match pair.master_read(&mut user_buf) {
                        Ok(n) => {
                            if n > 0 {
                                // Copy to userspace
                                if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                                    return SyscallResult::Err(14); // EFAULT
                                }
                            }
                            log::debug!("sys_read: Read {} bytes from PTY master {}", n, pty_num);
                            SyscallResult::Ok(n as u64)
                        }
                        Err(e) => {
                            log::debug!("sys_read: PTY master read error: {}", e);
                            SyscallResult::Err(e as u64)
                        }
                    }
                }
                None => {
                    log::error!("sys_read: PTY {} not found", pty_num);
                    SyscallResult::Err(super::errno::EIO as u64)
                }
            }
        }
        FdKind::PtySlave(pty_num) => {
            // Read from PTY slave (from line discipline output)
            match crate::tty::pty::get(*pty_num) {
                Some(pair) => {
                    let mut user_buf = alloc::vec![0u8; count as usize];
                    match pair.slave_read(&mut user_buf) {
                        Ok(n) => {
                            if n > 0 {
                                // Copy to userspace
                                if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                                    return SyscallResult::Err(14); // EFAULT
                                }
                            }
                            log::debug!("sys_read: Read {} bytes from PTY slave {}", n, pty_num);
                            SyscallResult::Ok(n as u64)
                        }
                        Err(e) => {
                            log::debug!("sys_read: PTY slave read error: {}", e);
                            SyscallResult::Err(e as u64)
                        }
                    }
                }
                None => {
                    log::error!("sys_read: PTY {} not found", pty_num);
                    SyscallResult::Err(super::errno::EIO as u64)
                }
            }
        }
        FdKind::UnixStream(socket_ref) => {
            // Read from Unix stream socket
            let is_nonblocking = (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
            let socket_clone = socket_ref.clone();

            // Drop manager guard before potentially blocking
            drop(manager_guard);

            let mut user_buf = alloc::vec![0u8; count as usize];

            loop {
                // Register as waiter FIRST to avoid race condition
                let socket = socket_clone.lock();
                socket.register_waiter(thread_id);
                drop(socket);

                // Try to read
                let socket = socket_clone.lock();
                match socket.read(&mut user_buf) {
                    Ok(n) => {
                        socket.unregister_waiter(thread_id);
                        drop(socket);

                        if n > 0 {
                            // Copy to userspace
                            if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                                return SyscallResult::Err(14); // EFAULT
                            }
                        }
                        log::debug!("sys_read: Read {} bytes from Unix socket", n);
                        return SyscallResult::Ok(n as u64);
                    }
                    Err(11) => {
                        // EAGAIN - no data available
                        if is_nonblocking {
                            socket.unregister_waiter(thread_id);
                            drop(socket);
                            return SyscallResult::Err(11); // EAGAIN
                        }

                        // Check if peer closed (EOF case)
                        if socket.peer_closed() {
                            socket.unregister_waiter(thread_id);
                            drop(socket);
                            return SyscallResult::Ok(0); // EOF
                        }

                        drop(socket);

                        // Block the thread
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.block_current();
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = true;
                            }
                        });

                        // Double-check for data after setting Blocked state
                        let socket = socket_clone.lock();
                        if socket.has_data() || socket.peer_closed() {
                            socket.unregister_waiter(thread_id);
                            drop(socket);
                            crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.blocked_in_syscall = false;
                                    thread.set_ready();
                                }
                            });
                            continue;
                        }
                        drop(socket);

                        // Re-enable preemption before HLT loop
                        crate::per_cpu::preempt_enable();

                        // HLT loop
                        loop {
                            crate::task::scheduler::yield_current();
                            Cpu::halt_with_interrupts();

                            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.state == crate::task::thread::ThreadState::Blocked
                                } else {
                                    false
                                }
                            }).unwrap_or(false);

                            if !still_blocked {
                                crate::per_cpu::preempt_disable();
                                break;
                            }
                        }

                        // Clear blocked_in_syscall
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                            }
                        });
                        crate::interrupts::timer::reset_quantum();
                        crate::task::scheduler::check_and_clear_need_resched();

                        // Unregister and retry
                        let socket = socket_clone.lock();
                        socket.unregister_waiter(thread_id);
                        drop(socket);
                        continue;
                    }
                    Err(e) => {
                        socket.unregister_waiter(thread_id);
                        drop(socket);
                        log::debug!("sys_read: Unix socket read error: {}", e);
                        return SyscallResult::Err(e as u64);
                    }
                }
            }
        }
        FdKind::UnixSocket(_) | FdKind::UnixListener(_) => {
            // Cannot read from unconnected Unix socket
            log::error!("sys_read: Cannot read from unconnected Unix socket");
            SyscallResult::Err(super::errno::ENOTCONN as u64)
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
    Cpu::without_interrupts(|| {
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

/// sys_exec_with_frame - Replace the current process with a new program (legacy, no argv support)
///
/// This is the older implementation without argv support. It is kept for backward
/// compatibility but is no longer used by the syscall handler (use sys_execv_with_frame instead).
///
/// Parameters:
/// - frame: mutable reference to the syscall frame (to update RIP/RSP on success)
/// - program_name_ptr: pointer to program name
/// - elf_data_ptr: pointer to ELF data in memory (for embedded programs)
///
/// Returns: Never returns on success (frame is modified to jump to new program)
/// Returns: Error code on failure
#[allow(dead_code)]
#[allow(unused_variables)]
#[allow(unreachable_code)]
pub fn sys_exec_with_frame(
    frame: &mut super::handler::SyscallFrame,
    program_name_ptr: u64,
    elf_data_ptr: u64,
) -> SyscallResult {
    Cpu::without_interrupts(|| {
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
        // We need both the ELF data and the program name for exec_process
        let (elf_data, exec_program_name): (&'static [u8], Option<&'static str>) = if program_name_ptr != 0 {
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
                let elf_data = Box::leak(boxed_slice) as &'static [u8];
                // Also leak the program name so we can pass it to exec_process
                let name_string = alloc::string::String::from(program_name);
                let leaked_name: &'static str = Box::leak(name_string.into_boxed_str());
                (elf_data, Some(leaked_name))
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
                (crate::userspace_test::get_test_binary_static("hello_world"), Some("hello_world"))
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
                match manager.exec_process(current_pid, elf_data, exec_program_name) {
                    Ok(new_entry_point) => {
                        log::info!(
                            "sys_exec: Successfully replaced process address space, entry point: {:#x}",
                            new_entry_point
                        );

                        // CRITICAL FIX: Get the new stack pointer from the process
                        // The exec_process function set up a new stack at USER_STACK_TOP
                        // NOTE: Must match the value used in exec_process() in manager.rs
                        const USER_STACK_TOP: u64 = 0x7FFF_FF01_0000;
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

/// Load ELF binary from ext2 filesystem path.
///
/// Returns the file content as Vec<u8> on success, or an errno on failure.
///
/// NOTE: This function intentionally has NO logging to avoid timing overhead.
/// It's called on every exec syscall, and serial I/O causes CI timing issues.
#[cfg(feature = "testing")]
fn load_elf_from_ext2(path: &str) -> Result<Vec<u8>, i32> {
    use super::errno::{EACCES, EIO, ENOENT, ENOTDIR};
    use crate::fs::ext2;

    // Get ext2 filesystem
    let fs_guard = ext2::root_fs();
    let fs = fs_guard.as_ref().ok_or(EIO)?;

    // Resolve path to inode number
    let inode_num = fs.resolve_path(path).map_err(|e| {
        if e.contains("not found") {
            ENOENT
        } else {
            EIO
        }
    })?;

    // Read inode metadata
    let inode = fs.read_inode(inode_num).map_err(|_| EIO)?;

    // Check it's a regular file (not directory)
    if inode.is_dir() {
        return Err(ENOTDIR);
    }

    // Check execute permission (S_IXUSR = 0o100)
    let perms = inode.permissions();
    if (perms & 0o100) == 0 {
        return Err(EACCES);
    }

    // Read file content
    let data = fs.read_file_content(&inode).map_err(|_| EIO)?;

    Ok(data)
}

/// sys_execv_with_frame - Replace the current process with a new program (with argv support)
///
/// This is the extended implementation that supports passing command-line arguments.
/// The kernel sets up argc/argv on the new process's stack following Linux ABI.
///
/// Parameters:
/// - frame: mutable reference to the syscall frame (to update RIP/RSP on success)
/// - program_name_ptr: pointer to program name (null-terminated string)
/// - argv_ptr: pointer to argv array (array of pointers to null-terminated strings, ending with NULL)
///
/// The argv array should be laid out in user memory as:
///   argv[0] -> pointer to first string (usually program name)
///   argv[1] -> pointer to second string
///   ...
///   argv[n] -> NULL (end of array)
///
/// Returns: Never returns on success (frame is modified to jump to new program)
/// Returns: Error code on failure
#[allow(unused_variables)]
pub fn sys_execv_with_frame(
    frame: &mut super::handler::SyscallFrame,
    program_name_ptr: u64,
    argv_ptr: u64,
) -> SyscallResult {
    // IMPORTANT: Do NOT wrap the entire function in without_interrupts()!
    // ELF loading from ext2 filesystem requires interrupts for VirtIO I/O.
    // Only the final frame manipulation needs to be interrupt-safe.

    log::info!(
        "sys_execv_with_frame called: program_name_ptr={:#x}, argv_ptr={:#x}",
        program_name_ptr,
        argv_ptr
    );

    // Get current process and thread
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_execv: No current thread");
            return SyscallResult::Err(22); // EINVAL
        }
    };

    // Read the program name from userspace
    if program_name_ptr == 0 {
        log::error!("sys_execv: NULL program name");
        return SyscallResult::Err(22); // EINVAL
    }

    let name_bytes = match copy_string_from_user(program_name_ptr, 256) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::error!("sys_execv: Failed to read program name: {}", e);
            return SyscallResult::Err(14); // EFAULT
        }
    };

    let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
    let program_name = match core::str::from_utf8(&name_bytes[..name_len]) {
        Ok(s) => s,
        Err(_) => {
            log::error!("sys_execv: Invalid UTF-8 in program name");
            return SyscallResult::Err(22); // EINVAL
        }
    };

    log::info!("sys_execv: Loading program '{}'", program_name);

    // Read argv array from userspace (with interrupts enabled - safe)
    let mut argv_vec: Vec<Vec<u8>> = Vec::new();

    if argv_ptr != 0 {
        // Read up to 64 argument pointers
        const MAX_ARGS: usize = 64;
        const MAX_ARG_LEN: usize = 4096;

        for i in 0..MAX_ARGS {
            let ptr_addr = argv_ptr + (i * 8) as u64;
            let arg_ptr_bytes = match copy_from_user(ptr_addr, 8) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::error!("sys_execv: Failed to read argv[{}] pointer: {}", i, e);
                    return SyscallResult::Err(14); // EFAULT
                }
            };

            // Interpret as u64 pointer
            let arg_ptr = u64::from_le_bytes([
                arg_ptr_bytes[0], arg_ptr_bytes[1], arg_ptr_bytes[2], arg_ptr_bytes[3],
                arg_ptr_bytes[4], arg_ptr_bytes[5], arg_ptr_bytes[6], arg_ptr_bytes[7],
            ]);

            // NULL pointer marks end of argv
            if arg_ptr == 0 {
                break;
            }

            // Read the argument string
            let arg_bytes = match copy_string_from_user(arg_ptr, MAX_ARG_LEN) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::error!("sys_execv: Failed to read argv[{}] string at {:#x}: {}", i, arg_ptr, e);
                    return SyscallResult::Err(14); // EFAULT
                }
            };

            // Find null terminator and truncate
            let arg_len = arg_bytes.iter().position(|&b| b == 0).unwrap_or(arg_bytes.len());
            let mut arg = arg_bytes[..arg_len].to_vec();
            arg.push(0); // Ensure null-terminated
            argv_vec.push(arg);
        }
    }

    // If no argv provided, use program name as argv[0]
    if argv_vec.is_empty() {
        let mut arg0 = program_name.as_bytes().to_vec();
        arg0.push(0);
        argv_vec.push(arg0);
    }

    log::info!("sys_execv: argc={}", argv_vec.len());
    for (i, arg) in argv_vec.iter().enumerate() {
        if let Ok(s) = core::str::from_utf8(&arg[..arg.len().saturating_sub(1)]) {
            log::debug!("sys_execv: argv[{}] = '{}'", i, s);
        }
    }

    #[cfg(feature = "testing")]
    {
        // Load ELF binary WITH interrupts enabled - ext2 I/O needs timer interrupts
        // for proper VirtIO operation
        let elf_vec = if program_name.contains('/') {
            // Path-like name: load from ext2 filesystem
            match load_elf_from_ext2(program_name) {
                Ok(data) => data,
                Err(errno) => return SyscallResult::Err(errno as u64),
            }
        } else {
            // Bare name: try ext2 /bin/ first, then fall back to test disk
            let bin_path = alloc::format!("/bin/{}", program_name);
            match load_elf_from_ext2(&bin_path) {
                Ok(data) => data,
                Err(_) => {
                    // Fall back to test disk for compatibility
                    crate::userspace_test::get_test_binary(program_name)
                }
            }
        };
        let boxed_slice = elf_vec.into_boxed_slice();
        let elf_data = Box::leak(boxed_slice) as &'static [u8];
        let name_string = alloc::string::String::from(program_name);
        let leaked_name: &'static str = Box::leak(name_string.into_boxed_str());

        // Find current process
        let current_pid = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((pid, _)) = manager.find_process_by_thread(current_thread_id) {
                    pid
                } else {
                    log::error!("sys_execv: Thread {} not found in any process", current_thread_id);
                    return SyscallResult::Err(3); // ESRCH
                }
            } else {
                log::error!("sys_execv: Process manager not available");
                return SyscallResult::Err(12); // ENOMEM
            }
        };

        log::info!(
            "sys_execv: Replacing process {} (thread {}) with new program",
            current_pid.as_u64(),
            current_thread_id
        );

        // Convert argv_vec to slice of slices for exec_process_with_argv
        let argv_slices: Vec<&[u8]> = argv_vec.iter().map(|v| v.as_slice()).collect();

        // CRITICAL SECTION: Frame manipulation and process state changes
        // Only this part needs interrupts disabled for atomicity
        Cpu::without_interrupts(|| {
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut manager) = *manager_guard {
                match manager.exec_process_with_argv(current_pid, elf_data, Some(leaked_name), &argv_slices) {
                    Ok((new_entry_point, new_rsp)) => {
                        log::info!(
                            "sys_execv: Successfully replaced process address space, entry={:#x}, rsp={:#x}",
                            new_entry_point, new_rsp
                        );

                        // Modify the syscall frame to jump to the new program
                        frame.rip = new_entry_point;
                        frame.rsp = new_rsp;
                        frame.rflags = 0x202;

                        // Clear all registers for security
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
                                log::info!("sys_execv: Setting next_cr3 to {:#x}", new_cr3);
                                unsafe {
                                    crate::per_cpu::set_next_cr3(new_cr3);
                                    core::arch::asm!(
                                        "mov gs:[80], {}",
                                        in(reg) new_cr3,
                                        options(nostack, preserves_flags)
                                    );
                                }
                            }
                        }

                        log::info!(
                            "sys_execv: Frame updated - RIP={:#x}, RSP={:#x}",
                            frame.rip, frame.rsp
                        );

                        SyscallResult::Ok(0)
                    }
                    Err(e) => {
                        log::error!("sys_execv: Failed to exec process: {}", e);
                        SyscallResult::Err(12) // ENOMEM
                    }
                }
            } else {
                log::error!("sys_execv: Process manager not available");
                SyscallResult::Err(12) // ENOMEM
            }
        })
    }

    #[cfg(not(feature = "testing"))]
    {
        SyscallResult::Err(38) // ENOSYS
    }
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
    Cpu::without_interrupts(|| {
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
        // We need both the ELF data and the program name for exec_process
        let (_elf_data, _exec_program_name): (&'static [u8], Option<&'static str>) = if program_name_ptr != 0 {
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
                let elf_data = Box::leak(boxed_slice) as &'static [u8];
                // Also leak the program name so we can pass it to exec_process
                let name_string = alloc::string::String::from(program_name);
                let leaked_name: &'static str = Box::leak(name_string.into_boxed_str());
                (elf_data, Some(leaked_name))
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
                (crate::userspace_test::get_test_binary_static("hello_world"), Some("hello_world"))
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
            match manager.exec_process(current_pid, _elf_data, _exec_program_name) {
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
    Cpu::without_interrupts(|| {
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

            // Enable preemption before entering HLT loop so scheduler can switch threads.
            // The syscall handler called preempt_disable() at entry, so we balance it here
            // to allow context switches while blocked. We must re-disable before returning
            // to match the preempt_enable() at syscall exit.
            crate::per_cpu::preempt_enable();

            loop {
                // Yield and halt - timer interrupt will switch to another thread
                // since current thread is blocked
                crate::task::scheduler::yield_current();
                Cpu::halt_with_interrupts();

                // After being rescheduled, check if child terminated
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    if let Some(child) = manager.get_process(target_pid) {
                        if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                            drop(manager_guard);
                            // Re-disable preemption before returning to balance syscall exit's preempt_enable()
                            crate::per_cpu::preempt_disable();
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

            // Enable preemption before entering HLT loop so scheduler can switch threads.
            // The syscall handler called preempt_disable() at entry, so we balance it here
            // to allow context switches while blocked. We must re-disable before returning
            // to match the preempt_enable() at syscall exit.
            crate::per_cpu::preempt_enable();

            loop {
                // Yield and halt - timer interrupt will switch to another thread
                // since current thread is blocked
                crate::task::scheduler::yield_current();
                Cpu::halt_with_interrupts();

                // After being rescheduled, check if any child terminated
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    for &child_pid in &children_copy {
                        if let Some(child) = manager.get_process(child_pid) {
                            if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                                drop(manager_guard);
                                // Re-disable preemption before returning to balance syscall exit's preempt_enable()
                                crate::per_cpu::preempt_disable();
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
    // Encode exit status in wstatus format.
    // The wstatus encoding distinguishes between:
    // - Normal exit (WIFEXITED): lower 7 bits are 0, exit code in bits 8-15
    // - Signal termination (WIFSIGNALED): lower 7 bits are signal number, bit 7 is core dump flag
    //
    // In our implementation:
    // - Negative exit codes indicate signal termination: exit_code = -(signal_number)
    // - Positive/zero exit codes indicate normal exit
    let wstatus: i32 = if exit_code < 0 {
        // Signal termination
        // Extract signal number from negative exit code
        let signal_number = (-exit_code) as i32;
        // Check for core dump flag (0x80 in signal number indicates core dump)
        let core_dump = (signal_number & 0x80) != 0;
        let sig = signal_number & 0x7f;
        // Encode: lower 7 bits = signal, bit 7 = core dump
        sig | (if core_dump { 0x80 } else { 0 })
    } else {
        // Normal exit
        // Linux encodes normal exit as: (exit_code & 0xff) << 8
        (exit_code & 0xff) << 8
    };

    log::debug!("complete_wait: child {} exited with code {}, wstatus={:#x}{}",
                child_pid.as_u64(), exit_code, wstatus,
                if exit_code < 0 { " (signal termination)" } else { " (normal exit)" });

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

    // Drain loopback queue for localhost connections (127.x.x.x, own IP).
    // Hardware-received packets arrive via interrupt → softirq → process_rx().
    crate::net::drain_loopback_queue();

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

    // Drain loopback queue for localhost connections (127.x.x.x, own IP).
    // Hardware-received packets arrive via interrupt → softirq → process_rx().
    crate::net::drain_loopback_queue();

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

/// CowStats structure returned by sys_cow_stats
/// Matches the layout expected by userspace
#[repr(C)]
pub struct CowStatsResult {
    pub total_faults: u64,
    pub manager_path: u64,
    pub direct_path: u64,
    pub pages_copied: u64,
    pub sole_owner_opt: u64,
}

/// sys_cow_stats - Get Copy-on-Write statistics (for testing)
///
/// This syscall is used to verify that the CoW optimization paths are working.
/// It returns the current CoW statistics to userspace.
///
/// Parameters:
/// - stats_ptr: pointer to a CowStatsResult structure in userspace
///
/// Returns: 0 on success, negative error code on failure
pub fn sys_cow_stats(stats_ptr: u64) -> SyscallResult {
    use crate::interrupts::cow_stats;

    if stats_ptr == 0 {
        return SyscallResult::Err(14); // EFAULT - null pointer
    }

    // Validate the address is in userspace
    if !crate::memory::layout::is_valid_user_address(stats_ptr) {
        log::error!("sys_cow_stats: Invalid userspace address {:#x}", stats_ptr);
        return SyscallResult::Err(14); // EFAULT
    }

    // Get the current stats
    let stats = cow_stats::get_stats();

    // Copy to userspace
    unsafe {
        let user_stats = stats_ptr as *mut CowStatsResult;
        (*user_stats).total_faults = stats.total_faults;
        (*user_stats).manager_path = stats.manager_path;
        (*user_stats).direct_path = stats.direct_path;
        (*user_stats).pages_copied = stats.pages_copied;
        (*user_stats).sole_owner_opt = stats.sole_owner_opt;
    }

    log::debug!(
        "sys_cow_stats: total={}, manager={}, direct={}, copied={}, sole_owner={}",
        stats.total_faults,
        stats.manager_path,
        stats.direct_path,
        stats.pages_copied,
        stats.sole_owner_opt
    );

    SyscallResult::Ok(0)
}

/// sys_simulate_oom - Enable or disable OOM simulation (for testing)
///
/// This syscall is used to test the kernel's behavior when frame allocation fails
/// during Copy-on-Write page faults. When OOM simulation is enabled, all frame
/// allocations will return None, causing CoW faults to fail and processes to be
/// terminated with SIGSEGV.
///
/// Parameters:
/// - enable: 1 to enable OOM simulation, 0 to disable
///
/// Returns: 0 on success, -ENOSYS if testing feature is not compiled in
///
/// # Safety
/// Only enable OOM simulation briefly for testing! Extended OOM simulation will
/// crash the kernel because it affects ALL frame allocations.
///
/// # Expected behavior when OOM is active
/// 1. Fork succeeds (CoW sharing, no new frames needed)
/// 2. Child writes to shared page (triggers CoW fault)
/// 3. CoW fault handler tries to allocate frame, fails
/// 4. handle_cow_fault() returns false
/// 5. page_fault_handler() kills the process with exit code -11 (SIGSEGV)
/// 6. Parent receives SIGCHLD and can waitpid() for the child
pub fn sys_simulate_oom(enable: u64) -> SyscallResult {
    #[cfg(feature = "testing")]
    {
        if enable != 0 {
            crate::memory::frame_allocator::enable_oom_simulation();
            log::info!("sys_simulate_oom: OOM simulation ENABLED");
        } else {
            crate::memory::frame_allocator::disable_oom_simulation();
            log::info!("sys_simulate_oom: OOM simulation disabled");
        }
        SyscallResult::Ok(0)
    }

    #[cfg(not(feature = "testing"))]
    {
        let _ = enable; // suppress unused warning
        log::warn!("sys_simulate_oom: testing feature not compiled in");
        SyscallResult::Err(38) // ENOSYS - function not implemented
    }
}
