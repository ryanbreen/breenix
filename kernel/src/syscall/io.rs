//! AArch64 syscall implementations for generic I/O and FD operations.
//!
//! These are duplicated from x86_64 handlers to avoid touching the hot-path
//! syscall handler while the ARM64 port is being brought up.

#![cfg(target_arch = "aarch64")]

use super::SyscallResult;
use alloc::vec::Vec;
use crate::syscall::userptr::validate_user_buffer;

/// Copy a byte buffer from userspace.
fn copy_from_user_bytes(ptr: u64, len: usize) -> Result<Vec<u8>, u64> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if ptr == 0 {
        return Err(14); // EFAULT
    }

    validate_user_buffer(ptr as *const u8, len)?;

    let mut buffer = Vec::with_capacity(len);
    unsafe {
        let slice = core::slice::from_raw_parts(ptr as *const u8, len);
        buffer.extend_from_slice(slice);
    }
    Ok(buffer)
}

/// Copy a byte buffer to userspace.
fn copy_to_user_bytes(ptr: u64, data: &[u8]) -> Result<(), u64> {
    if data.is_empty() {
        return Ok(());
    }
    if ptr == 0 {
        return Err(14); // EFAULT
    }

    validate_user_buffer(ptr as *const u8, data.len())?;

    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, data.len());
    }
    Ok(())
}

/// Helper function to write to stdio through TTY layer.
fn write_to_stdio(fd: u64, buffer: &[u8]) -> SyscallResult {
    let _ = fd;
    let bytes_written = crate::tty::write_output(buffer);
    SyscallResult::Ok(bytes_written as u64)
}

/// sys_write - Write to a file descriptor
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
    use crate::ipc::FdKind;

    if buf_ptr == 0 || count == 0 {
        return SyscallResult::Ok(0);
    }

    let buffer = match copy_from_user_bytes(buf_ptr, count as usize) {
        Ok(buf) => buf,
        Err(_) => return SyscallResult::Err(14), // EFAULT
    };

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            return write_to_stdio(fd, &buffer);
        }
    };

    enum WriteOperation {
        StdIo,
        Pipe { pipe_buffer: alloc::sync::Arc<spin::Mutex<crate::ipc::pipe::PipeBuffer>>, is_nonblocking: bool },
        UnixStream { socket: alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixStreamSocket>> },
        TcpConnection { conn_id: crate::net::tcp::ConnectionId },
        PtyMaster(u32),
        PtySlave(u32),
        Ebadf,
        Enotconn,
        Eisdir,
        Eopnotsupp,
    }

    let write_op = {
        let manager_guard = crate::process::manager();
        let process = match &*manager_guard {
            Some(manager) => match manager.find_process_by_thread(thread_id) {
                Some((_pid, p)) => p,
                None => return write_to_stdio(fd, &buffer),
            },
            None => return write_to_stdio(fd, &buffer),
        };

        let fd_entry = match process.fd_table.get(fd as i32) {
            Some(entry) => entry,
            None => {
                return SyscallResult::Err(9); // EBADF
            }
        };

        match &fd_entry.kind {
            FdKind::StdIo(n) if *n == 1 || *n == 2 => WriteOperation::StdIo,
            FdKind::StdIo(_) => WriteOperation::Ebadf,
            FdKind::PipeWrite(pipe_buffer) => {
                WriteOperation::Pipe { pipe_buffer: pipe_buffer.clone(), is_nonblocking: (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0 }
            }
            FdKind::PipeRead(_) => WriteOperation::Ebadf,
            FdKind::FifoWrite(_path, pipe_buffer) => {
                WriteOperation::Pipe { pipe_buffer: pipe_buffer.clone(), is_nonblocking: (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0 }
            }
            FdKind::FifoRead(_, _) => WriteOperation::Ebadf,
            FdKind::UdpSocket(_) => WriteOperation::Eopnotsupp,
            FdKind::RegularFile(_) => WriteOperation::Eopnotsupp,
            FdKind::Device(_) => WriteOperation::Eopnotsupp,
            FdKind::Directory(_) | FdKind::DevfsDirectory { .. } | FdKind::DevptsDirectory { .. } => {
                WriteOperation::Eisdir
            }
            FdKind::UnixStream(socket) => WriteOperation::UnixStream { socket: socket.clone() },
            FdKind::UnixSocket(_) => WriteOperation::Enotconn,
            FdKind::UnixListener(_) => WriteOperation::Enotconn,
            FdKind::PtyMaster(pty_num) => WriteOperation::PtyMaster(*pty_num),
            FdKind::PtySlave(pty_num) => WriteOperation::PtySlave(*pty_num),
            // TCP sockets
            FdKind::TcpSocket(_) | FdKind::TcpListener(_) => WriteOperation::Enotconn,
            FdKind::TcpConnection(conn_id) => WriteOperation::TcpConnection { conn_id: *conn_id },
        }
    };

    match write_op {
        WriteOperation::StdIo => write_to_stdio(fd, &buffer),
        WriteOperation::Ebadf => SyscallResult::Err(9),
        WriteOperation::Enotconn => SyscallResult::Err(super::errno::ENOTCONN as u64),
        WriteOperation::Eisdir => SyscallResult::Err(super::errno::EISDIR as u64),
        WriteOperation::Eopnotsupp => SyscallResult::Err(95),
        WriteOperation::Pipe { pipe_buffer, is_nonblocking } => {
            let mut pipe = pipe_buffer.lock();
            match pipe.write(&buffer) {
                Ok(n) => SyscallResult::Ok(n as u64),
                Err(11) if !is_nonblocking => SyscallResult::Err(11),
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        WriteOperation::UnixStream { socket } => {
            let sock = socket.lock();
            match sock.write(&buffer) {
                Ok(n) => SyscallResult::Ok(n as u64),
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        WriteOperation::TcpConnection { conn_id } => {
            // Write to established TCP connection
            match crate::net::tcp::tcp_send(&conn_id, &buffer) {
                Ok(n) => SyscallResult::Ok(n as u64),
                Err(e) => {
                    // Return EPIPE if the connection was shutdown for writing
                    if e.contains("shutdown") {
                        SyscallResult::Err(super::errno::EPIPE as u64)
                    } else {
                        SyscallResult::Err(super::errno::EIO as u64)
                    }
                }
            }
        }
        WriteOperation::PtyMaster(pty_num) => {
            // Write to PTY master - data goes to slave through line discipline
            if let Some(pair) = crate::tty::pty::get(pty_num) {
                match pair.master_write(&buffer) {
                    Ok(n) => SyscallResult::Ok(n as u64),
                    Err(e) => SyscallResult::Err(e as u64),
                }
            } else {
                SyscallResult::Err(5) // EIO
            }
        }
        WriteOperation::PtySlave(pty_num) => {
            // Write to PTY slave - data goes to master
            if let Some(pair) = crate::tty::pty::get(pty_num) {
                match pair.slave_write(&buffer) {
                    Ok(n) => SyscallResult::Ok(n as u64),
                    Err(e) => SyscallResult::Err(e as u64),
                }
            } else {
                SyscallResult::Err(5) // EIO
            }
        }
    }
}

/// sys_read - Read from a file descriptor
pub fn sys_read(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
    use crate::ipc::FdKind;

    if buf_ptr == 0 || count == 0 {
        return SyscallResult::Ok(0);
    }

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            return SyscallResult::Ok(0);
        }
    };
    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => return SyscallResult::Ok(0),
        },
        None => return SyscallResult::Ok(0),
    };

    let fd_entry = match process.fd_table.get(fd as i32) {
        Some(entry) => entry,
        None => return SyscallResult::Err(9),
    };

    match &fd_entry.kind {
        FdKind::StdIo(0) => {
            drop(manager_guard);

            let mut user_buf = alloc::vec![0u8; count as usize];

            loop {
                crate::ipc::stdin::register_blocked_reader(thread_id);

                let read_result = crate::ipc::stdin::read_bytes(&mut user_buf);

                match read_result {
                    Ok(n) => {
                        crate::ipc::stdin::unregister_blocked_reader(thread_id);
                        if n > 0 {
                            if copy_to_user_bytes(buf_ptr, &user_buf[..n]).is_err() {
                                return SyscallResult::Err(14);
                            }
                        }
                        return SyscallResult::Ok(n as u64);
                    }
                    Err(11) => {
                        // EAGAIN - no data available, need to block
                        // Block the current thread AND set blocked_in_syscall flag.
                        // CRITICAL: Setting blocked_in_syscall is essential because:
                        // 1. The thread will enter a kernel-mode WFI loop below
                        // 2. If a context switch happens while in WFI, the scheduler sees
                        //    from_userspace=false (kernel mode) but blocked_in_syscall tells
                        //    it to save/restore kernel context, not userspace context
                        // 3. Without this flag, no context is saved when switching away,
                        //    and stale userspace context is restored when switching back,
                        //    causing ELR_EL1 corruption (kernel address in userspace context)
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.block_current();
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = true;
                            }
                        });

                        // CRITICAL: Re-enable preemption before entering blocking loop!
                        // The syscall handler called preempt_disable() at entry, but we need
                        // to allow timer interrupts to schedule other threads while we're blocked.
                        crate::per_cpu::preempt_enable();

                        // WFI loop - wait for interrupt which will either:
                        // 1. Wake us via wake_blocked_readers_try() when keyboard data arrives
                        // 2. Context switch to another thread via timer interrupt
                        loop {
                            // Check for pending signals that should interrupt this syscall
                            if let Some(e) = crate::syscall::check_signals_for_eintr() {
                                // Signal pending - unblock and return EINTR
                                crate::ipc::stdin::unregister_blocked_reader(thread_id);
                                crate::task::scheduler::with_scheduler(|sched| {
                                    if let Some(thread) = sched.current_thread_mut() {
                                        thread.blocked_in_syscall = false;
                                        thread.set_ready();
                                    }
                                });
                                crate::per_cpu::preempt_disable();
                                return SyscallResult::Err(e as u64);
                            }

                            crate::task::scheduler::yield_current();
                            // CRITICAL: Enable interrupts before WFI!
                            // preempt_enable() only modifies preempt counter, not DAIF.
                            // Without enabling interrupts, WFI wakes immediately but
                            // the timer interrupt handler never runs, causing the
                            // thread to spin in a tight loop without ever yielding.
                            #[cfg(target_arch = "aarch64")]
                            unsafe { core::arch::asm!("msr daifclr, #0xf", options(nomem, nostack)); }
                            unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }

                            // Check if we've been unblocked (thread state changed from Blocked)
                            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                                sched.current_thread()
                                    .map(|t| t.state == crate::task::thread::ThreadState::Blocked)
                                    .unwrap_or(false)
                            }).unwrap_or(false);

                            if !still_blocked {
                                // CRITICAL: Check for signals AFTER waking up!
                                // We might have been unblocked to deliver a signal.
                                if let Some(e) = crate::syscall::check_signals_for_eintr() {
                                    crate::ipc::stdin::unregister_blocked_reader(thread_id);
                                    crate::task::scheduler::with_scheduler(|sched| {
                                        if let Some(thread) = sched.current_thread_mut() {
                                            thread.blocked_in_syscall = false;
                                            thread.set_ready();
                                        }
                                    });
                                    crate::per_cpu::preempt_disable();
                                    return SyscallResult::Err(e as u64);
                                }
                                break; // We've been woken for data, try reading again
                            }
                        }

                        // Re-disable preemption before continuing to balance syscall's preempt_disable
                        crate::per_cpu::preempt_disable();

                        // Clear blocked_in_syscall now that we're resuming normal syscall execution
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                            }
                        });
                    }
                    Err(e) => {
                        crate::ipc::stdin::unregister_blocked_reader(thread_id);
                        return SyscallResult::Err(e as u64);
                    }
                }
            }
        }
        FdKind::StdIo(_) => SyscallResult::Err(9),
        FdKind::PipeRead(pipe_buffer) => {
            let mut pipe = pipe_buffer.lock();
            let mut buf = alloc::vec![0u8; count as usize];
            match pipe.read(&mut buf) {
                Ok(n) => {
                    if copy_to_user_bytes(buf_ptr, &buf[..n]).is_err() {
                        return SyscallResult::Err(14);
                    }
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        FdKind::PipeWrite(_) => SyscallResult::Err(9),
        FdKind::FifoRead(_path, pipe_buffer) => {
            let mut pipe = pipe_buffer.lock();
            let mut buf = alloc::vec![0u8; count as usize];
            match pipe.read(&mut buf) {
                Ok(n) => {
                    if copy_to_user_bytes(buf_ptr, &buf[..n]).is_err() {
                        return SyscallResult::Err(14);
                    }
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        FdKind::FifoWrite(_, _) => SyscallResult::Err(9),
        FdKind::UdpSocket(_) | FdKind::UnixSocket(_) | FdKind::UnixListener(_) => {
            SyscallResult::Err(super::errno::ENOTCONN as u64)
        }
        FdKind::RegularFile(file_ref) => {
            // Read from ext2 regular file
            //
            // Get file info under the lock, then drop lock before filesystem operations
            let (inode_num, position) = {
                let file = file_ref.lock();
                (file.inode_num, file.position)
            };

            // Drop the process manager lock before filesystem operations
            drop(manager_guard);

            // Access the mounted ext2 filesystem
            let root_fs = crate::fs::ext2::root_fs();
            let fs = match root_fs.as_ref() {
                Some(fs) => fs,
                None => {
                    return SyscallResult::Err(super::errno::ENOSYS as u64);
                }
            };

            // Read the inode
            let inode = match fs.read_inode(inode_num as u32) {
                Ok(inode) => inode,
                Err(_) => {
                    return SyscallResult::Err(super::errno::EIO as u64);
                }
            };

            // Read the file data
            let data = match fs.read_file_range(&inode, position, count as usize) {
                Ok(data) => data,
                Err(_) => {
                    return SyscallResult::Err(super::errno::EIO as u64);
                }
            };

            // Drop the filesystem lock before copying to userspace
            drop(root_fs);

            let bytes_read = data.len();

            // Copy data to userspace
            if bytes_read > 0 {
                if copy_to_user_bytes(buf_ptr, &data).is_err() {
                    return SyscallResult::Err(14); // EFAULT
                }
            }

            // Update file position - need to re-acquire process manager lock
            {
                let manager_guard = crate::process::manager();
                if let Some(manager) = &*manager_guard {
                    if let Some((_, process)) = manager.find_process_by_thread(thread_id) {
                        if let Some(fd_entry) = process.fd_table.get(fd as i32) {
                            if let FdKind::RegularFile(file_ref) = &fd_entry.kind {
                                let mut file = file_ref.lock();
                                file.position += bytes_read as u64;
                            }
                        }
                    }
                }
            }

            SyscallResult::Ok(bytes_read as u64)
        }
        FdKind::Device(device_type) => {
            // Read from devfs device (/dev/null, /dev/zero, /dev/console, /dev/tty)
            let mut user_buf = alloc::vec![0u8; count as usize];
            match crate::fs::devfs::device_read(*device_type, &mut user_buf) {
                Ok(n) => {
                    if n > 0 {
                        // Copy to userspace
                        if copy_to_user_bytes(buf_ptr, &user_buf[..n]).is_err() {
                            return SyscallResult::Err(14); // EFAULT
                        }
                    }
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => {
                    SyscallResult::Err((-e) as u64)
                }
            }
        }
        FdKind::Directory(_) | FdKind::DevfsDirectory { .. } | FdKind::DevptsDirectory { .. } => {
            SyscallResult::Err(super::errno::EISDIR as u64)
        }
        FdKind::UnixStream(socket) => {
            let sock = socket.lock();
            let mut buf = alloc::vec![0u8; count as usize];
            match sock.read(&mut buf) {
                Ok(n) => {
                    if copy_to_user_bytes(buf_ptr, &buf[..n]).is_err() {
                        return SyscallResult::Err(14);
                    }
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        FdKind::PtyMaster(pty_num) => {
            // Read from PTY master - read data written by slave
            if let Some(pair) = crate::tty::pty::get(*pty_num) {
                let mut buf = alloc::vec![0u8; count as usize];
                match pair.master_read(&mut buf) {
                    Ok(n) => {
                        if n > 0 {
                            if copy_to_user_bytes(buf_ptr, &buf[..n]).is_err() {
                                return SyscallResult::Err(14);
                            }
                        }
                        SyscallResult::Ok(n as u64)
                    }
                    Err(e) => SyscallResult::Err(e as u64),
                }
            } else {
                SyscallResult::Err(5) // EIO
            }
        }
        FdKind::PtySlave(pty_num) => {
            // Read from PTY slave - read processed data from line discipline
            if let Some(pair) = crate::tty::pty::get(*pty_num) {
                let mut buf = alloc::vec![0u8; count as usize];
                match pair.slave_read(&mut buf) {
                    Ok(n) => {
                        if n > 0 {
                            if copy_to_user_bytes(buf_ptr, &buf[..n]).is_err() {
                                return SyscallResult::Err(14);
                            }
                        }
                        SyscallResult::Ok(n as u64)
                    }
                    Err(e) => SyscallResult::Err(e as u64),
                }
            } else {
                SyscallResult::Err(5) // EIO
            }
        }
        // TCP sockets
        FdKind::TcpSocket(_) | FdKind::TcpListener(_) => {
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
                        if copy_to_user_bytes(buf_ptr, &user_buf[..n]).is_err() {
                            return SyscallResult::Err(14); // EFAULT
                        }
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
                            return SyscallResult::Err(super::errno::EAGAIN as u64);
                        }
                        // Will block below
                    }
                    _ => unreachable!(),
                }

                // No data - block the thread
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.block_current();
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.blocked_in_syscall = true;
                    }
                });

                // Double-check for data after setting Blocked state
                if crate::net::tcp::tcp_has_data(&conn_id) {
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                            thread.set_ready();
                        }
                    });
                    crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
                    continue;
                }

                // Re-enable preemption before WFI loop
                crate::per_cpu::preempt_enable();

                // WFI loop - wait for data to arrive
                loop {
                    // Check for pending signals that should interrupt this syscall
                    if let Some(e) = crate::syscall::check_signals_for_eintr() {
                        // Signal pending - unblock and return EINTR
                        crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                thread.set_ready();
                            }
                        });
                        crate::per_cpu::preempt_disable();
                        return SyscallResult::Err(e as u64);
                    }

                    // CRITICAL: Drain loopback queue - ARM64 has no NIC interrupts,
                    // so we must poll for localhost packets during blocking waits
                    crate::net::drain_loopback_queue();

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

                    crate::task::scheduler::yield_current();
                    unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
                }

                // Reset quantum after blocking wait to prevent immediate preemption
                crate::arch_impl::aarch64::timer_interrupt::reset_quantum();

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
    }
}

/// sys_dup - Duplicate a file descriptor
pub fn sys_dup(old_fd: u64) -> SyscallResult {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(9),
    };

    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
            None => return SyscallResult::Err(9),
        },
        None => return SyscallResult::Err(9),
    };

    match process.fd_table.dup(old_fd as i32) {
        Ok(fd) => SyscallResult::Ok(fd as u64),
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// sys_dup2 - Duplicate a file descriptor to a specific number
pub fn sys_dup2(old_fd: u64, new_fd: u64) -> SyscallResult {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(9),
    };

    let mut manager_guard = crate::process::manager();
    let process = match &mut *manager_guard {
        Some(manager) => match manager.find_process_by_thread_mut(thread_id) {
            Some((_pid, p)) => p,
            None => return SyscallResult::Err(9),
        },
        None => return SyscallResult::Err(9),
    };

    match process.fd_table.dup2(old_fd as i32, new_fd as i32) {
        Ok(fd) => SyscallResult::Ok(fd as u64),
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// sys_fcntl - file control operations
pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64) -> SyscallResult {
    use crate::ipc::fd::fcntl_cmd::*;

    let fd = fd as i32;
    let cmd = cmd as i32;
    let arg = arg as i32;

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(9),
    };

    let manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => return SyscallResult::Err(9),
    };

    let _process = match manager_guard
        .as_ref()
        .and_then(|m| m.find_process_by_thread(thread_id))
        .map(|(_, p)| p)
    {
        Some(p) => p,
        None => return SyscallResult::Err(9),
    };

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
                Ok(new_fd) => SyscallResult::Ok(new_fd as u64),
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        F_DUPFD_CLOEXEC => {
            match process.fd_table.dup_at_least(fd, arg, true) {
                Ok(new_fd) => SyscallResult::Ok(new_fd as u64),
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        F_GETFD => {
            match process.fd_table.get(fd) {
                Some(entry) => SyscallResult::Ok(entry.flags as u64),
                None => SyscallResult::Err(9),
            }
        }
        F_SETFD => {
            match process.fd_table.get_mut(fd) {
                Some(entry) => {
                    entry.flags = arg as u32;
                    SyscallResult::Ok(0)
                }
                None => SyscallResult::Err(9),
            }
        }
        F_GETFL => {
            match process.fd_table.get(fd) {
                Some(entry) => SyscallResult::Ok(entry.status_flags as u64),
                None => SyscallResult::Err(9),
            }
        }
        F_SETFL => {
            match process.fd_table.get_mut(fd) {
                Some(entry) => {
                    entry.status_flags = arg as u32;
                    SyscallResult::Ok(0)
                }
                None => SyscallResult::Err(9),
            }
        }
        _ => SyscallResult::Err(22),
    }
}

/// sys_poll - Poll file descriptors for I/O readiness
pub fn sys_poll(fds_ptr: u64, nfds: u64, _timeout: i32) -> SyscallResult {
    use crate::ipc::poll::{self, events, PollFd};

    crate::net::drain_loopback_queue();

    if fds_ptr == 0 && nfds > 0 {
        return SyscallResult::Err(14);
    }

    if nfds > 256 {
        return SyscallResult::Err(22);
    }

    if nfds == 0 {
        return SyscallResult::Ok(0);
    }

    let byte_len = (core::mem::size_of::<PollFd>()) * (nfds as usize);
    if validate_user_buffer(fds_ptr as *const u8, byte_len).is_err() {
        return SyscallResult::Err(14);
    }

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(22),
    };

    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => return SyscallResult::Err(22),
        },
        None => return SyscallResult::Err(22),
    };

    let mut pollfds: Vec<PollFd> = Vec::with_capacity(nfds as usize);
    unsafe {
        let src = fds_ptr as *const PollFd;
        for i in 0..nfds as usize {
            pollfds.push(core::ptr::read(src.add(i)));
        }
    }

    let mut ready_count: u64 = 0;

    for pollfd in pollfds.iter_mut() {
        pollfd.revents = 0;

        if pollfd.fd < 0 {
            continue;
        }

        let fd_entry = match process.fd_table.get(pollfd.fd) {
            Some(entry) => entry,
            None => {
                pollfd.revents = events::POLLNVAL;
                ready_count += 1;
                continue;
            }
        };

        pollfd.revents = poll::poll_fd(fd_entry, pollfd.events);
        if pollfd.revents != 0 {
            ready_count += 1;
        }
    }

    unsafe {
        let dst = fds_ptr as *mut PollFd;
        for (i, pollfd) in pollfds.iter().enumerate() {
            core::ptr::write(dst.add(i), *pollfd);
        }
    }

    SyscallResult::Ok(ready_count)
}

/// sys_select - Synchronous I/O multiplexing
pub fn sys_select(
    nfds: i32,
    readfds_ptr: u64,
    writefds_ptr: u64,
    exceptfds_ptr: u64,
    _timeout_ptr: u64,
) -> SyscallResult {
    use crate::ipc::poll;

    crate::net::drain_loopback_queue();

    if nfds < 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    if nfds > 64 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    if readfds_ptr != 0 {
        if validate_user_buffer(readfds_ptr as *const u8, core::mem::size_of::<u64>()).is_err() {
            return SyscallResult::Err(14);
        }
    }
    if writefds_ptr != 0 {
        if validate_user_buffer(writefds_ptr as *const u8, core::mem::size_of::<u64>()).is_err() {
            return SyscallResult::Err(14);
        }
    }
    if exceptfds_ptr != 0 {
        if validate_user_buffer(exceptfds_ptr as *const u8, core::mem::size_of::<u64>()).is_err() {
            return SyscallResult::Err(14);
        }
    }

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::EINVAL as u64),
    };

    let manager_guard = crate::process::manager();
    let process = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((_pid, p)) => p,
            None => return SyscallResult::Err(super::errno::EINVAL as u64),
        },
        None => return SyscallResult::Err(super::errno::EINVAL as u64),
    };

    let readfds = if readfds_ptr != 0 { unsafe { *(readfds_ptr as *const u64) } } else { 0 };
    let writefds = if writefds_ptr != 0 { unsafe { *(writefds_ptr as *const u64) } } else { 0 };
    let exceptfds = if exceptfds_ptr != 0 { unsafe { *(exceptfds_ptr as *const u64) } } else { 0 };

    let mut ready_read: u64 = 0;
    let mut ready_write: u64 = 0;
    let mut ready_except: u64 = 0;
    let mut ready_count = 0;

    for fd in 0..nfds {
        let fd_mask = 1u64 << fd;
        let fd_entry = match process.fd_table.get(fd as i32) {
            Some(entry) => entry,
            None => continue,
        };

        let mut fd_ready = false;

        if (readfds & fd_mask) != 0 {
            if poll::poll_fd(fd_entry, poll::events::POLLIN) != 0 {
                ready_read |= fd_mask;
                fd_ready = true;
            }
        }

        if (writefds & fd_mask) != 0 {
            if poll::poll_fd(fd_entry, poll::events::POLLOUT) != 0 {
                ready_write |= fd_mask;
                fd_ready = true;
            }
        }

        if (exceptfds & fd_mask) != 0 {
            if poll::poll_fd(fd_entry, poll::events::POLLERR) != 0 {
                ready_except |= fd_mask;
                fd_ready = true;
            }
        }

        if fd_ready {
            ready_count += 1;
        }
    }

    if readfds_ptr != 0 {
        unsafe { *(readfds_ptr as *mut u64) = ready_read; }
    }
    if writefds_ptr != 0 {
        unsafe { *(writefds_ptr as *mut u64) = ready_write; }
    }
    if exceptfds_ptr != 0 {
        unsafe { *(exceptfds_ptr as *mut u64) = ready_except; }
    }

    SyscallResult::Ok(ready_count)
}
