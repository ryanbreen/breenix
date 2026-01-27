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
        Fifo { pipe_buffer: alloc::sync::Arc<spin::Mutex<crate::ipc::pipe::PipeBuffer>>, is_nonblocking: bool },
        UnixStream { socket: alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixStreamSocket>> },
        RegularFile { file: alloc::sync::Arc<spin::Mutex<crate::ipc::fd::RegularFile>> },
        TcpConnection { conn_id: crate::net::tcp::ConnectionId },
        Device { device_type: crate::fs::devfs::DeviceType },
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
                WriteOperation::Fifo { pipe_buffer: pipe_buffer.clone(), is_nonblocking: (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0 }
            }
            FdKind::FifoRead(_, _) => WriteOperation::Ebadf,
            FdKind::TcpSocket(_) => WriteOperation::Enotconn,
            FdKind::TcpListener(_) => WriteOperation::Enotconn,
            FdKind::TcpConnection(conn_id) => WriteOperation::TcpConnection { conn_id: *conn_id },
            FdKind::UdpSocket(_) => WriteOperation::Eopnotsupp,
            FdKind::UnixStream(socket) => WriteOperation::UnixStream { socket: socket.clone() },
            FdKind::UnixSocket(_) => WriteOperation::Enotconn,
            FdKind::UnixListener(_) => WriteOperation::Enotconn,
            FdKind::RegularFile(file) => WriteOperation::RegularFile { file: file.clone() },
            FdKind::Directory(_) => WriteOperation::Eisdir,
            FdKind::Device(device_type) => WriteOperation::Device { device_type: device_type.clone() },
            FdKind::DevfsDirectory { .. } => WriteOperation::Eisdir,
            FdKind::DevptsDirectory { .. } => WriteOperation::Eisdir,
            FdKind::PtyMaster(_) | FdKind::PtySlave(_) => WriteOperation::Eopnotsupp,
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
        WriteOperation::Fifo { pipe_buffer, is_nonblocking } => {
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
            match crate::net::tcp::tcp_send(&conn_id, &buffer) {
                Ok(n) => SyscallResult::Ok(n as u64),
                Err(e) => {
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
                DeviceType::Null | DeviceType::Zero => SyscallResult::Ok(buffer.len() as u64),
                DeviceType::Console | DeviceType::Tty => write_to_stdio(fd, &buffer),
            }
        }
        WriteOperation::RegularFile { file } => {
            let (inode_num, position, flags) = {
                let file_guard = file.lock();
                (file_guard.inode_num, file_guard.position, file_guard.flags)
            };

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

            {
                let mut file_guard = file.lock();
                file_guard.position = write_offset + bytes_written as u64;
            }

            SyscallResult::Ok(bytes_written as u64)
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
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.block_current_for_stdin_read();
                        });
                        loop {
                            if crate::ipc::stdin::has_data() {
                                break;
                            }
                            unsafe { core::arch::asm!("wfi"); }
                        }
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
            let mut buf = vec![0u8; count as usize];
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
        FdKind::FifoRead(_path, pipe_buffer) => {
            let mut pipe = pipe_buffer.lock();
            let mut buf = vec![0u8; count as usize];
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
        FdKind::PipeWrite(_) | FdKind::FifoWrite(_, _) => SyscallResult::Err(9),
        FdKind::UdpSocket(_) | FdKind::TcpSocket(_) | FdKind::UnixSocket(_) | FdKind::UnixListener(_) | FdKind::TcpListener(_) => {
            SyscallResult::Err(super::errno::ENOTCONN as u64)
        }
        FdKind::UnixStream(socket) => {
            let mut sock = socket.lock();
            let mut buf = vec![0u8; count as usize];
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
        FdKind::TcpConnection(conn_id) => {
            let mut buf = vec![0u8; count as usize];
            match crate::net::tcp::tcp_recv(conn_id, &mut buf) {
                Ok(n) => {
                    if copy_to_user_bytes(buf_ptr, &buf[..n]).is_err() {
                        return SyscallResult::Err(14);
                    }
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        FdKind::RegularFile(file) => {
            let (inode_num, position) = {
                let file_guard = file.lock();
                (file_guard.inode_num, file_guard.position)
            };

            let mut root_fs = crate::fs::ext2::root_fs();
            let fs = match root_fs.as_mut() {
                Some(fs) => fs,
                None => return SyscallResult::Err(super::errno::ENOSYS as u64),
            };

            let mut buf = vec![0u8; count as usize];
            let bytes_read = match fs.read_file_range(inode_num as u32, position, &mut buf) {
                Ok(n) => n,
                Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
            };

            drop(root_fs);

            {
                let mut file_guard = file.lock();
                file_guard.position = position + bytes_read as u64;
            }

            if copy_to_user_bytes(buf_ptr, &buf[..bytes_read]).is_err() {
                return SyscallResult::Err(14);
            }

            SyscallResult::Ok(bytes_read as u64)
        }
        FdKind::Directory(_) | FdKind::DevfsDirectory { .. } | FdKind::DevptsDirectory { .. } => {
            SyscallResult::Err(super::errno::EISDIR as u64)
        }
        FdKind::Device(device_type) => {
            use crate::fs::devfs::DeviceType;
            match device_type {
                DeviceType::Null => SyscallResult::Ok(0),
                DeviceType::Zero => {
                    let buf = vec![0u8; count as usize];
                    if copy_to_user_bytes(buf_ptr, &buf).is_err() {
                        return SyscallResult::Err(14);
                    }
                    SyscallResult::Ok(count)
                }
                DeviceType::Console | DeviceType::Tty => SyscallResult::Err(9),
            }
        }
        FdKind::PtyMaster(_) | FdKind::PtySlave(_) => SyscallResult::Err(95),
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
            match process.fd_table.dup_min(fd, arg) {
                Ok(new_fd) => SyscallResult::Ok(new_fd as u64),
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        F_DUPFD_CLOEXEC => {
            match process.fd_table.dup_min(fd, arg) {
                Ok(new_fd) => {
                    if let Some(entry) = process.fd_table.get_mut(new_fd) {
                        entry.flags |= crate::ipc::fd::flags::FD_CLOEXEC;
                    }
                    SyscallResult::Ok(new_fd as u64)
                }
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
