//! System call handler implementations
//!
//! This module contains the actual implementation of each system call.

use super::SyscallResult;
#[cfg(target_arch = "x86_64")]
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::Translate;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

/// Architecture-conditional reset_quantum helper.
/// Same pattern as syscall/socket.rs.
#[inline]
fn reset_quantum() {
    #[cfg(target_arch = "x86_64")]
    crate::interrupts::timer::reset_quantum();
    #[cfg(target_arch = "aarch64")]
    crate::arch_impl::aarch64::timer_interrupt::reset_quantum();
}

/// Global flag to signal that userspace testing is complete and kernel should exit
pub static USERSPACE_TEST_COMPLETE: AtomicBool = AtomicBool::new(false);

/// P6a PR-2, review finding B2. Latches the one post-userspace tombstone census
/// sample so the x86 gate can pin it by exact count: the block below is entered
/// whenever the last userspace thread exits, and a second entry would emit a
/// second line with different values.
#[cfg(all(target_arch = "x86_64", feature = "boot_tests"))]
static TOMBSTONE_CENSUS_AFTER_USERSPACE: AtomicBool = AtomicBool::new(false);

/// File descriptors (legacy constants, now using FdKind-based routing)
#[allow(dead_code)]
const FD_STDIN: u64 = 0;
#[allow(dead_code)]
const FD_STDOUT: u64 = 1;
#[allow(dead_code)]
const FD_STDERR: u64 = 2;

/// Copy data from userspace memory
///
/// CRITICAL: This function works WITHOUT switching page tables.
/// The kernel mappings MUST be present in all process page tables for this to work.
/// We rely on the fact that userspace memory is mapped in the current page table.
fn copy_from_user(user_ptr: u64, len: usize) -> Result<Vec<u8>, &'static str> {
    if user_ptr == 0 {
        return Err("null pointer");
    }

    // Validate the whole [user_ptr, user_ptr+len) range against the closed
    // allow-list of legitimate userspace regions (code/data, mmap, stack) --
    // see memory::layout::is_valid_user_range's doc comment for the full
    // rationale. We deliberately do NOT use
    // userptr::validate_user_buffer's broad canonical-half bound check here:
    // on x86_64 that bound also contains the kernel's own mapped PIE image
    // and heap, which ProcessPageTable::new copies (without USER_ACCESSIBLE)
    // into every process's page table -- a userspace pointer into either
    // region still translates and is still readable by this kernel-mode
    // code, turning this function into a kernel-memory read primitive
    // (#729 review finding B4). The earlier comment here justified the
    // broad check as covering heap-allocated addresses is_valid_user_address
    // misses; that premise did not hold (#729 review finding M4): a
    // process's brk-extended heap stays under USERSPACE_CODE_DATA_END and
    // was already covered by the code/data region.
    if !crate::memory::layout::is_valid_user_range(user_ptr, len) {
        return Err("invalid userspace address");
    }

    let mut buffer = Vec::with_capacity(len);

    unsafe {
        let slice = core::slice::from_raw_parts(user_ptr as *const u8, len);
        buffer.extend_from_slice(slice);
    }

    Ok(buffer)
}

#[cfg(target_arch = "x86_64")]
fn copy_string_from_user(user_ptr: u64, max_len: usize) -> Result<Vec<u8>, &'static str> {
    if user_ptr == 0 {
        return Err("null pointer");
    }

    // Validate the worst-case [user_ptr, user_ptr + max_len) range up front
    // against the closed allow-list of legitimate userspace regions
    // (code/data, mmap, stack) -- see copy_from_user's comment above and
    // memory::layout::is_valid_user_range's doc comment for the full
    // rationale. This function used to make this same check with
    // userptr::validate_user_buffer's broad canonical-half bound, which also
    // admits the kernel's own mapped PIE image and heap on x86_64 --
    // #729 review finding B4 confirmed this was a live, userspace-reachable
    // kernel-memory disclosure through sys_spawn's argv, not merely a
    // theoretical widening. #729's original heap-address concern does not
    // require a separate arm here: a process's brk-extended heap stays under
    // USERSPACE_CODE_DATA_END and is already covered by the code/data
    // region (#729 review finding M4). The actual string may be shorter
    // than max_len (we stop at the first NUL byte below); validating the
    // full worst-case length up front is still correct because a shorter
    // valid string is always a subset of an accepted range.
    if !crate::memory::layout::is_valid_user_range(user_ptr, max_len) {
        return Err("invalid userspace address");
    }

    let mapper = unsafe { crate::memory::paging::get_mapper() };
    let mut buffer = Vec::new();

    for offset in 0..max_len {
        let addr = user_ptr
            .checked_add(offset as u64)
            .ok_or("userspace address overflow")?;

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

    // Validate the whole [user_ptr, user_ptr+len) range against the closed
    // allow-list of legitimate userspace regions -- see copy_from_user's
    // comment above and memory::layout::is_valid_user_range's doc comment
    // for the full rationale. copy_to_user WRITES kernel-supplied bytes to
    // user_ptr, so the broad canonical-half bound this used to check with
    // (userptr::validate_user_buffer) was not just a kernel-memory read
    // primitive like copy_from_user's (#729 review finding B4) but a kernel-
    // memory WRITE / corruption primitive: any syscall that copies a result
    // buffer back to a caller-supplied pointer (e.g. read()) would happily
    // write into the kernel's own mapped PIE image or heap if the caller
    // named an address there.
    if !crate::memory::layout::is_valid_user_range(user_ptr, len) {
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
        crate::tracing::providers::process::trace_thread_exit(thread_id as u16, exit_code as u16);

        // Handle clear_child_tid for clone threads (CLONE_CHILD_CLEARTID).
        // Snapshot under PROCESS_MANAGER, then write to userspace after the
        // lock is dropped because the tid address may reference a CoW page.
        let clear_child_tid = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                    if let Some(tid_addr) = process.clear_child_tid {
                        let tg_id = process.thread_group_id.unwrap_or(_pid.as_u64());
                        Some((tg_id, tid_addr))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some((tg_id, tid_addr)) = clear_child_tid {
            let zero = 0u32;
            let _ = super::userptr::copy_to_user(tid_addr as *mut u32, &zero);
            super::futex::futex_wake_for_thread_group(tg_id, tid_addr, u32::MAX);
        }

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
            #[cfg(target_arch = "x86_64")]
            {
                crate::keyboard::stream::wake_keyboard_task();
                log::info!("Woke keyboard task to ensure input processing continues");
            }

            // Signal that userspace testing is complete with clear markers
            log::info!("🎯 USERSPACE TEST COMPLETE - All processes finished successfully");
            let (exited, nonzero) = crate::task::exit_tally::totals();
            let (failures, failure_count) = crate::task::exit_tally::snapshot_failures();
            let failures = &failures[..failure_count];
            log::info!(
                "TEST_TALLY: exited={} nonzero={} failed=[{}]",
                exited,
                nonzero,
                crate::task::exit_tally::FailureList::new(failures, nonzero)
            );

            if nonzero == 0 {
                log::info!("=====================================");
                log::info!("✅ USERSPACE EXECUTION SUCCESSFUL ✅");
                log::info!("✅ Ring 3 execution confirmed       ✅");
                log::info!("✅ System calls working correctly   ✅");
                log::info!("✅ Process lifecycle complete       ✅");
                log::info!("=====================================");
                log::info!("🏁 TEST RUNNER: All tests passed - you can exit QEMU now 🏁");
            } else {
                log::error!(
                    "🚨 Failing userspace processes: {} 🚨",
                    crate::task::exit_tally::FailureList::new(failures, nonzero)
                );
                log::error!(
                    "🚨 TEST RUNNER: FAILED - {} of {} userspace processes exited nonzero 🚨",
                    nonzero,
                    exited
                );
            }

            // Set flag for automated systems that want to detect completion
            USERSPACE_TEST_COMPLETE.store(true, Ordering::SeqCst);

            // #775: follow the periodic heartbeat with a final snapshot after
            // the last userspace exit has been recorded. The accounting itself
            // is lock-free and allocation-free; no formatting occurs in the
            // interrupt or context-switch path.
            #[cfg(target_arch = "x86_64")]
            crate::task::dispatch_strand_census::report_snapshot();

            // P6a PR-2, review finding B2: sample the tombstone census AFTER a
            // live reap. x86's other two census sites both fire before any user
            // process exists, so `removed` never left the join oracle's own two
            // rows and whether the rows the four live `complete_wait` reaps
            // claimed completed their join was unmeasured — the x86 half of this
            // phase's central retention claim had no evidence. This point is the
            // end of the userspace phase: no userspace thread remains, so no
            // further reap can occur, and `resident` here is retention at
            // quiesce. Boot-test profile only, on the exit path, once per boot.
            #[cfg(all(target_arch = "x86_64", feature = "boot_tests"))]
            if !TOMBSTONE_CENSUS_AFTER_USERSPACE.swap(true, Ordering::SeqCst) {
                crate::tracing::providers::teardown::emit_tombstone_census();
            }

            // Fallback BTRT finalization: if all userspace threads are gone,
            // finalize regardless of whether every registered PID called on_process_exit.
            // This handles forked children, hanging tests, etc.
            #[cfg(feature = "btrt")]
            crate::test_framework::btrt::finalize();
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

/// Validate `fd` before a degenerate transfer answers `Ok(0)`.
///
/// `read`, `write`, `pread64` and `pwrite64` all answer a zero-length (or
/// null-buffer) request with `Ok(0)`. Linux looks the descriptor up first, so
/// such a request against a closed, negative or never-opened descriptor fails
/// with `EBADF`; ours returned success and told the caller nothing (#670).
///
/// Returns `Err(EBADF)` only when the caller has a process context whose
/// descriptor table has no such entry. Kernel threads have no descriptor table
/// at all, so they keep whatever fallback their handler already applied.
///
/// This runs only on the degenerate path. The ordinary path already performs
/// the same lookup, so no non-degenerate call gains work.
fn validate_fd_for_degenerate_transfer(fd: i32) -> Result<(), u64> {
    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return Ok(());
    };
    crate::arch_without_interrupts(|| {
        let manager_guard = crate::process::manager();
        let Some(manager) = manager_guard.as_ref() else {
            return Ok(());
        };
        let Some((_pid, process)) = manager.find_process_by_thread(thread_id) else {
            return Ok(());
        };
        if process.fd_table.get(fd).is_some() {
            Ok(())
        } else {
            Err(super::errno::EBADF as u64)
        }
    })
}

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
        // Linux checks the descriptor before honouring a degenerate transfer
        // (#670): a zero-length operation on a bad descriptor is EBADF, not 0.
        if let Err(e) = validate_fd_for_degenerate_transfer(fd as i32) {
            return SyscallResult::Err(e);
        }
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
        Pipe {
            pipe_buffer: alloc::sync::Arc<spin::Mutex<crate::ipc::pipe::PipeBuffer>>,
            is_nonblocking: bool,
        },
        Fifo {
            pipe_buffer: alloc::sync::Arc<spin::Mutex<crate::ipc::pipe::PipeBuffer>>,
            is_nonblocking: bool,
        },
        UnixStream {
            socket: alloc::sync::Arc<spin::Mutex<crate::socket::unix::UnixStreamSocket>>,
        },
        RegularFile {
            file: alloc::sync::Arc<spin::Mutex<crate::ipc::fd::RegularFile>>,
        },
        TcpConnection {
            conn_id: crate::net::tcp::ConnectionId,
        },
        Device {
            device_type: crate::fs::devfs::DeviceType,
        },
        PtyMaster(u32),
        PtySlave(u32),
        Ebadf,
        Enotconn,   // Socket not connected
        Eisdir,     // Is a directory
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
            FdKind::PipeWrite(pipe_buffer) => WriteOperation::Pipe {
                pipe_buffer: pipe_buffer.clone(),
                is_nonblocking: (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK)
                    != 0,
            },
            FdKind::PipeRead(_) => WriteOperation::Ebadf,
            FdKind::FifoWrite(_path, pipe_buffer) => WriteOperation::Fifo {
                pipe_buffer: pipe_buffer.clone(),
                is_nonblocking: (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK)
                    != 0,
            },
            FdKind::FifoRead(_, _) => WriteOperation::Ebadf,
            FdKind::TcpSocket(_) => WriteOperation::Enotconn,
            FdKind::TcpListener(_) => WriteOperation::Enotconn,
            FdKind::TcpConnection(conn_id) => WriteOperation::TcpConnection { conn_id: *conn_id },
            FdKind::UdpSocket(_) => WriteOperation::Eopnotsupp, // UDP must use sendto
            FdKind::UnixStream(socket) => WriteOperation::UnixStream {
                socket: socket.clone(),
            },
            FdKind::UnixSocket(_) => WriteOperation::Enotconn, // Unconnected Unix socket
            FdKind::UnixListener(_) => WriteOperation::Enotconn, // Listener can't write
            FdKind::RegularFile(file) => WriteOperation::RegularFile { file: file.clone() },
            FdKind::Directory(_) => WriteOperation::Eisdir,
            FdKind::Device(device_type) => WriteOperation::Device {
                device_type: device_type.clone(),
            },
            FdKind::DevfsDirectory { .. } => WriteOperation::Eisdir,
            FdKind::DevptsDirectory { .. } => WriteOperation::Eisdir,
            FdKind::PtyMaster(pty_num) => WriteOperation::PtyMaster(*pty_num),
            FdKind::PtySlave(pty_num) => WriteOperation::PtySlave(*pty_num),
            FdKind::ProcfsFile { .. } => WriteOperation::Ebadf,
            FdKind::ProcfsDirectory { .. } => WriteOperation::Eisdir,
            FdKind::Epoll(_) => WriteOperation::Ebadf,
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
        WriteOperation::PtyMaster(pty_num) => {
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
            if let Some(pair) = crate::tty::pty::get(pty_num) {
                match pair.slave_write(&buffer) {
                    Ok(n) => SyscallResult::Ok(n as u64),
                    Err(e) => SyscallResult::Err(e as u64),
                }
            } else {
                SyscallResult::Err(5) // EIO
            }
        }
        WriteOperation::Pipe {
            pipe_buffer,
            is_nonblocking,
        } => {
            let mut pipe = pipe_buffer.lock();
            match pipe.write(&buffer) {
                Ok(n) => {
                    log::debug!("sys_write: Wrote {} bytes to pipe", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(11) if !is_nonblocking => {
                    // Blocking pipe write not implemented, return EAGAIN
                    log::debug!(
                        "sys_write: Pipe full, blocking not implemented - returning EAGAIN"
                    );
                    SyscallResult::Err(11) // EAGAIN
                }
                Err(e) => SyscallResult::Err(e as u64),
            }
        }
        WriteOperation::Fifo {
            pipe_buffer,
            is_nonblocking,
        } => {
            let mut pipe = pipe_buffer.lock();
            match pipe.write(&buffer) {
                Ok(n) => {
                    log::debug!("sys_write: Wrote {} bytes to FIFO", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(11) if !is_nonblocking => {
                    log::debug!(
                        "sys_write: FIFO full, blocking not implemented - returning EAGAIN"
                    );
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
                    crate::net::drain_loopback_queue();
                    log::debug!("sys_write: Wrote {} bytes to TCP connection", n);
                    SyscallResult::Ok(n as u64)
                }
                Err(e) => {
                    log::warn!("sys_write: TCP write error: {}", e);
                    // Map error string to specific errno
                    if e.contains("shutdown") {
                        SyscallResult::Err(super::errno::EPIPE as u64)
                    } else if e.contains("not found") {
                        SyscallResult::Err(super::errno::EBADF as u64)
                    } else if e.contains("not established") {
                        // Connection exists but state is not Established
                        // (RST received -> Closed, or FIN received -> CloseWait)
                        SyscallResult::Err(super::errno::ENOTCONN as u64)
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
            let (inode_num, position, flags, file_mount_id) = {
                let file_guard = file.lock();
                (
                    file_guard.inode_num,
                    file_guard.position,
                    file_guard.flags,
                    file_guard.mount_id,
                )
            };

            // Dispatch to correct filesystem based on mount_id
            let is_home = crate::fs::ext2::home_mount_id().map_or(false, |id| id == file_mount_id);

            let (write_offset, bytes_written) = if is_home {
                let mut fs_guard = crate::fs::ext2::home_fs_write();
                let fs = match fs_guard.as_mut() {
                    Some(fs) => fs,
                    None => return SyscallResult::Err(super::errno::ENOSYS as u64),
                };
                let wo = if (flags & crate::syscall::fs::O_APPEND) != 0 {
                    match fs.read_inode(inode_num as u32) {
                        Ok(inode) => inode.size(),
                        Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
                    }
                } else {
                    position
                };
                let bw = match fs.write_file_range(inode_num as u32, wo, &buffer) {
                    Ok(n) => n,
                    Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
                };
                (wo, bw)
            } else {
                let mut fs_guard = crate::fs::ext2::root_fs_write();
                let fs = match fs_guard.as_mut() {
                    Some(fs) => fs,
                    None => return SyscallResult::Err(super::errno::ENOSYS as u64),
                };
                let wo = if (flags & crate::syscall::fs::O_APPEND) != 0 {
                    match fs.read_inode(inode_num as u32) {
                        Ok(inode) => inode.size(),
                        Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
                    }
                } else {
                    position
                };
                let bw = match fs.write_file_range(inode_num as u32, wo, &buffer) {
                    Ok(n) => n,
                    Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
                };
                (wo, bw)
            };

            // Update file position
            {
                let mut file_guard = file.lock();
                file_guard.position = write_offset + bytes_written as u64;
            }

            log::debug!(
                "sys_write: Wrote {} bytes to regular file (inode {})",
                bytes_written,
                inode_num
            );
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
        log::debug!(
            "sys_read: fd={}, buf_ptr={:#x}, count={}",
            fd,
            buf_ptr,
            count
        );
    }

    // Validate buffer pointer and count
    if buf_ptr == 0 || count == 0 {
        // Linux checks the descriptor before honouring a degenerate transfer
        // (#670): a zero-length operation on a bad descriptor is EBADF, not 0.
        if let Err(e) = validate_fd_for_degenerate_transfer(fd as i32) {
            return SyscallResult::Err(e);
        }
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
                            sched.block_current_in_syscall();
                        });

                        log::trace!("sys_read: Thread {} blocking on stdin", thread_id);

                        // CRITICAL: Re-enable preemption before entering blocking loop!
                        // The syscall handler called preempt_disable() at entry, but we need
                        // to allow timer interrupts to schedule other threads while we're blocked.
                        crate::per_cpu::preempt_enable();

                        // HLT loop - wait for timer interrupt which will switch to another thread
                        // When keyboard data arrives, the interrupt handler will unblock us
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
                                log::debug!(
                                    "sys_read: Thread {} interrupted by signal (EINTR)",
                                    thread_id
                                );
                                return SyscallResult::Err(e as u64);
                            }

                            crate::task::scheduler::yield_current();
                            crate::arch_halt_with_interrupts();

                            // Check if we were unblocked (thread state changed from Blocked)
                            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.state == crate::task::thread::ThreadState::Blocked
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);

                            if !still_blocked {
                                log::trace!(
                                    "sys_read: Thread {} unblocked from stdin wait",
                                    thread_id
                                );
                                break;
                            }
                        }

                        // Re-disable preemption before continuing to balance syscall's preempt_disable
                        crate::per_cpu::preempt_disable();

                        // Clear blocked_in_syscall now that we're resuming normal syscall execution
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                log::trace!(
                                    "sys_read: Thread {} cleared blocked_in_syscall",
                                    thread.id
                                );
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
            let is_nonblocking =
                (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
            let pipe_buffer_clone = pipe_buffer.clone();

            // CRITICAL: Release process manager lock before potentially blocking!
            // If we hold the lock while blocked in the HLT loop, timer interrupts
            // cannot perform context switches to other threads (like the child
            // process that needs to write to the pipe).
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
                        log::debug!("sys_read: Read {} bytes from pipe", n);
                        return SyscallResult::Ok(n as u64);
                    }
                    Err(11) => {
                        // EAGAIN - buffer empty but writers exist
                        if is_nonblocking {
                            log::debug!("sys_read: Pipe empty, O_NONBLOCK set - returning EAGAIN");
                            return SyscallResult::Err(11); // EAGAIN
                        }

                        // === BLOCKING PATH ===
                        let thread_id = match crate::task::scheduler::current_thread_id() {
                            Some(tid) => tid,
                            None => return SyscallResult::Err(3), // ESRCH
                        };

                        log::debug!(
                            "sys_read: Pipe empty, thread {} entering blocking path",
                            thread_id
                        );

                        // Register as waiter BEFORE setting blocked state (race condition fix)
                        {
                            let mut pipe = pipe_buffer_clone.lock();
                            pipe.add_read_waiter(thread_id);
                        }

                        // Block the thread
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.block_current_in_syscall();
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
                            // Check for pending signals that should interrupt this syscall
                            if let Some(e) = crate::syscall::check_signals_for_eintr() {
                                // Signal pending - clean up and return EINTR
                                {
                                    let mut pipe = pipe_buffer_clone.lock();
                                    pipe.remove_read_waiter(thread_id);
                                }
                                crate::task::scheduler::with_scheduler(|sched| {
                                    if let Some(thread) = sched.current_thread_mut() {
                                        thread.blocked_in_syscall = false;
                                        thread.set_ready();
                                    }
                                });
                                crate::per_cpu::preempt_disable();
                                log::debug!(
                                    "sys_read: Pipe thread {} interrupted by signal (EINTR)",
                                    thread_id
                                );
                                return SyscallResult::Err(e as u64);
                            }

                            crate::task::scheduler::yield_current();
                            crate::arch_halt_with_interrupts();

                            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.state == crate::task::thread::ThreadState::Blocked
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);

                            if !still_blocked {
                                crate::per_cpu::preempt_disable();
                                log::debug!(
                                    "sys_read: Pipe thread {} woken from blocking",
                                    thread_id
                                );
                                break;
                            }
                        }

                        // Clear blocked state
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                            }
                        });
                        reset_quantum();
                        crate::task::scheduler::check_and_clear_need_resched();

                        // Continue loop to retry read
                        continue;
                    }
                    Err(e) => {
                        log::debug!("sys_read: Pipe read error: {}", e);
                        return SyscallResult::Err(e as u64);
                    }
                }
            }
        }
        FdKind::PipeWrite(_) => {
            // Can't read from write end of pipe
            SyscallResult::Err(9) // EBADF
        }
        FdKind::FifoRead(_path, pipe_buffer) => {
            // FIFO read - with blocking support
            let is_nonblocking =
                (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
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

                        log::debug!(
                            "sys_read: FIFO empty, thread {} entering blocking path",
                            thread_id
                        );

                        // Register as waiter BEFORE setting blocked state (race condition fix)
                        {
                            let mut pipe = pipe_buffer_clone.lock();
                            pipe.add_read_waiter(thread_id);
                        }

                        // Block the thread
                        crate::task::scheduler::with_scheduler(|sched| {
                            sched.block_current_in_syscall();
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
                            // Check for pending signals that should interrupt this syscall
                            if let Some(e) = crate::syscall::check_signals_for_eintr() {
                                // Signal pending - clean up and return EINTR
                                {
                                    let mut pipe = pipe_buffer_clone.lock();
                                    pipe.remove_read_waiter(thread_id);
                                }
                                crate::task::scheduler::with_scheduler(|sched| {
                                    if let Some(thread) = sched.current_thread_mut() {
                                        thread.blocked_in_syscall = false;
                                        thread.set_ready();
                                    }
                                });
                                crate::per_cpu::preempt_disable();
                                log::debug!(
                                    "sys_read: FIFO thread {} interrupted by signal (EINTR)",
                                    thread_id
                                );
                                return SyscallResult::Err(e as u64);
                            }

                            crate::task::scheduler::yield_current();
                            crate::arch_halt_with_interrupts();

                            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.state == crate::task::thread::ThreadState::Blocked
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);

                            if !still_blocked {
                                crate::per_cpu::preempt_disable();
                                log::debug!(
                                    "sys_read: FIFO thread {} woken from blocking",
                                    thread_id
                                );
                                break;
                            }
                        }

                        // Clear blocked state
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                            }
                        });
                        reset_quantum();
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
            // Read from ext2 regular file.
            //
            // CRITICAL: clone the Arc and extract values while PM lock held, then
            // drop PM lock BEFORE doing disk I/O.  On ARM64 the PM lock disables ALL
            // IRQs, and AHCI completions arrive as interrupts — holding the lock
            // during disk I/O deadlocks the system.
            let file_ref_owned = file_ref.clone();
            let (inode_num, position, file_mount_id) = {
                let file = file_ref.lock();
                (file.inode_num, file.position, file.mount_id)
            };
            // Release PM lock now — disk I/O below needs IRQs enabled.
            drop(manager_guard);

            // Dispatch to correct filesystem based on mount_id
            let is_home = crate::fs::ext2::home_mount_id().map_or(false, |id| id == file_mount_id);
            let data = if is_home {
                let fs_guard = crate::fs::ext2::home_fs_read();
                let fs = match fs_guard.as_ref() {
                    Some(fs) => fs,
                    None => {
                        log::error!("sys_read: ext2 home filesystem not mounted");
                        return SyscallResult::Err(super::errno::ENOSYS as u64);
                    }
                };
                let inode = match fs.read_inode(inode_num as u32) {
                    Ok(inode) => inode,
                    Err(e) => {
                        log::error!("sys_read: Failed to read inode {}: {}", inode_num, e);
                        return SyscallResult::Err(super::errno::EIO as u64);
                    }
                };
                match fs.read_file_range(&inode, position, count as usize) {
                    Ok(data) => data,
                    Err(e) => {
                        log::error!("sys_read: Failed to read file data: {}", e);
                        return SyscallResult::Err(super::errno::EIO as u64);
                    }
                }
            } else {
                let fs_guard = crate::fs::ext2::root_fs_read();
                let fs = match fs_guard.as_ref() {
                    Some(fs) => fs,
                    None => {
                        log::error!("sys_read: ext2 filesystem not mounted");
                        return SyscallResult::Err(super::errno::ENOSYS as u64);
                    }
                };
                let inode = match fs.read_inode(inode_num as u32) {
                    Ok(inode) => inode,
                    Err(e) => {
                        log::error!("sys_read: Failed to read inode {}: {}", inode_num, e);
                        return SyscallResult::Err(super::errno::EIO as u64);
                    }
                };
                match fs.read_file_range(&inode, position, count as usize) {
                    Ok(data) => data,
                    Err(e) => {
                        log::error!("sys_read: Failed to read file data: {}", e);
                        return SyscallResult::Err(super::errno::EIO as u64);
                    }
                }
            };

            let bytes_read = data.len();

            // Copy data to userspace
            if bytes_read > 0 {
                if copy_to_user(buf_ptr, data.as_ptr() as u64, bytes_read).is_err() {
                    return SyscallResult::Err(14); // EFAULT
                }
            }

            // Update file position (use the owned Arc we cloned before dropping PM lock)
            {
                let mut file = file_ref_owned.lock();
                file.position += bytes_read as u64;
            }

            log::debug!(
                "sys_read: Read {} bytes from regular file (inode {})",
                bytes_read,
                inode_num
            );
            SyscallResult::Ok(bytes_read as u64)
        }
        FdKind::Directory(_) => {
            // Cannot read from directory with read() - must use getdents
            log::debug!("sys_read: Cannot read from directory, use getdents instead");
            SyscallResult::Err(super::errno::EISDIR as u64)
        }
        FdKind::Device(device_type) => {
            // Read from devfs device (/dev/null, /dev/zero, /dev/console, /dev/tty)
            let device_type = *device_type;
            drop(manager_guard);
            let mut user_buf = alloc::vec![0u8; count as usize];
            match crate::fs::devfs::device_read(device_type, &mut user_buf) {
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
            let is_nonblocking =
                (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
            drop(manager_guard);

            let mut user_buf = alloc::vec![0u8; count as usize];

            // Read loop (may block if O_NONBLOCK not set)
            loop {
                // Register as waiter FIRST to avoid race condition
                crate::net::tcp::tcp_register_recv_waiter(&conn_id, thread_id);

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
                    sched.block_current_in_syscall();
                });

                // Double-check for data after setting Blocked state
                if crate::net::tcp::tcp_has_data(&conn_id) {
                    log::debug!(
                        "TCP: Thread {} caught race - data arrived during block setup",
                        thread_id
                    );
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

                log::debug!(
                    "TCP_BLOCK: Thread {} entering blocked state for recv",
                    thread_id
                );

                // HLT loop - wait for data to arrive
                loop {
                    // Check for pending signals that should interrupt this syscall
                    if let Some(e) = crate::syscall::check_signals_for_eintr() {
                        // Signal pending - clean up and return EINTR
                        crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                thread.set_ready();
                            }
                        });
                        crate::per_cpu::preempt_disable();
                        log::debug!(
                            "sys_read: TCP thread {} interrupted by signal (EINTR)",
                            thread_id
                        );
                        return SyscallResult::Err(e as u64);
                    }

                    crate::task::scheduler::yield_current();
                    crate::arch_halt_with_interrupts();

                    let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.state == crate::task::thread::ThreadState::Blocked
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);

                    // #772 instrumentation (experiment lane, counters only): distinguish
                    // a wasted turn (loop re-observes Blocked and sleeps again) from a
                    // turn that actually consumed the wake. See
                    // kernel/src/tracing/providers/counters.rs for the counter docs and
                    // docs/planning/green-program/sockets/764-RCA-2026-09-03.md for the
                    // RCA context these are meant to distinguish.
                    if still_blocked {
                        crate::trace_count!(
                            crate::tracing::providers::counters::RECV_WAIT_STILL_BLOCKED_TRUE
                        );
                    } else {
                        crate::trace_count!(
                            crate::tracing::providers::counters::RECV_WAIT_STILL_BLOCKED_FALSE
                        );
                    }

                    if !still_blocked {
                        crate::per_cpu::preempt_disable();
                        log::debug!("TCP_BLOCK: Thread {} woken from recv blocking", thread_id);
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
            }
        }
        FdKind::PtyMaster(pty_num) => {
            // Read from PTY master (slave's output) with blocking support
            let pty_num = *pty_num;
            let is_nonblocking =
                (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
            let pair = match crate::tty::pty::get(pty_num) {
                Some(p) => p,
                None => {
                    log::error!("sys_read: PTY {} not found", pty_num);
                    return SyscallResult::Err(super::errno::EIO as u64);
                }
            };
            drop(manager_guard);

            let mut user_buf = alloc::vec![0u8; count as usize];

            loop {
                pair.register_master_waiter(thread_id);

                match pair.master_read(&mut user_buf) {
                    Ok(n) => {
                        pair.unregister_master_waiter(thread_id);
                        if n > 0 {
                            if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                                return SyscallResult::Err(14); // EFAULT
                            }
                        }
                        return SyscallResult::Ok(n as u64);
                    }
                    Err(_) => {
                        if is_nonblocking {
                            pair.unregister_master_waiter(thread_id);
                            return SyscallResult::Err(super::errno::EAGAIN as u64);
                        }
                    }
                }

                // Block the thread
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.block_current_in_syscall();
                });

                // Double-check for data or hangup after setting Blocked state
                if pair.should_wake_master() {
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                            thread.set_ready();
                        }
                    });
                    pair.unregister_master_waiter(thread_id);
                    continue;
                }

                crate::per_cpu::preempt_enable();

                // HLT loop - wait for data to arrive
                loop {
                    if let Some(e) = crate::syscall::check_signals_for_eintr() {
                        pair.unregister_master_waiter(thread_id);
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
                    crate::arch_halt_with_interrupts();

                    let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.state == crate::task::thread::ThreadState::Blocked
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);

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

                pair.unregister_master_waiter(thread_id);
            }
        }
        FdKind::PtySlave(pty_num) => {
            // Read from PTY slave (from line discipline output) with blocking support
            let pty_num = *pty_num;
            let is_nonblocking =
                (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
            let pair = match crate::tty::pty::get(pty_num) {
                Some(p) => p,
                None => {
                    log::error!("sys_read: PTY {} not found", pty_num);
                    return SyscallResult::Err(super::errno::EIO as u64);
                }
            };
            drop(manager_guard);

            let mut user_buf = alloc::vec![0u8; count as usize];

            loop {
                pair.register_slave_waiter(thread_id);

                match pair.slave_read(&mut user_buf) {
                    Ok(n) => {
                        pair.unregister_slave_waiter(thread_id);
                        if n > 0 {
                            if copy_to_user(buf_ptr, user_buf.as_ptr() as u64, n).is_err() {
                                return SyscallResult::Err(14); // EFAULT
                            }
                        }
                        return SyscallResult::Ok(n as u64);
                    }
                    Err(_) => {
                        if is_nonblocking {
                            pair.unregister_slave_waiter(thread_id);
                            return SyscallResult::Err(super::errno::EAGAIN as u64);
                        }
                    }
                }

                // Block the thread
                crate::task::scheduler::with_scheduler(|sched| {
                    sched.block_current_in_syscall();
                });

                // Double-check for data after setting Blocked state
                if pair.has_slave_data() {
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                            thread.set_ready();
                        }
                    });
                    pair.unregister_slave_waiter(thread_id);
                    continue;
                }

                crate::per_cpu::preempt_enable();

                // HLT loop - wait for data to arrive
                loop {
                    if let Some(e) = crate::syscall::check_signals_for_eintr() {
                        pair.unregister_slave_waiter(thread_id);
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
                    crate::arch_halt_with_interrupts();

                    let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.state == crate::task::thread::ThreadState::Blocked
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);

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

                pair.unregister_slave_waiter(thread_id);
            }
        }
        FdKind::UnixStream(socket_ref) => {
            // Read from Unix stream socket
            let is_nonblocking =
                (fd_entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
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
                            sched.block_current_in_syscall();
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
                            // Check for pending signals that should interrupt this syscall
                            if let Some(e) = crate::syscall::check_signals_for_eintr() {
                                // Signal pending - clean up and return EINTR
                                let socket = socket_clone.lock();
                                socket.unregister_waiter(thread_id);
                                drop(socket);
                                crate::task::scheduler::with_scheduler(|sched| {
                                    if let Some(thread) = sched.current_thread_mut() {
                                        thread.blocked_in_syscall = false;
                                        thread.set_ready();
                                    }
                                });
                                crate::per_cpu::preempt_disable();
                                log::debug!(
                                    "sys_read: Unix socket thread {} interrupted by signal (EINTR)",
                                    thread_id
                                );
                                return SyscallResult::Err(e as u64);
                            }

                            crate::task::scheduler::yield_current();
                            crate::arch_halt_with_interrupts();

                            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.state == crate::task::thread::ThreadState::Blocked
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);

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
                        reset_quantum();
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
        FdKind::ProcfsFile {
            ref content,
            position,
        } => {
            // Read from procfs virtual file
            let content = content.clone();
            let pos = *position;
            drop(manager_guard);
            let bytes = content.as_bytes();
            if pos >= bytes.len() {
                return SyscallResult::Ok(0);
            }
            let remaining = &bytes[pos..];
            let to_copy = remaining.len().min(count as usize);
            if to_copy > 0 {
                if copy_to_user(buf_ptr, remaining.as_ptr() as u64, to_copy).is_err() {
                    return SyscallResult::Err(14); // EFAULT
                }
            }
            // Update position - re-acquire the manager lock
            let mut mg = crate::process::manager();
            if let Some(manager) = &mut *mg {
                if let Some((_pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                    if let Some(fd_entry) = process.fd_table.get_mut(fd as i32) {
                        if let FdKind::ProcfsFile { position, .. } = &mut fd_entry.kind {
                            *position += to_copy;
                        }
                    }
                }
            }
            SyscallResult::Ok(to_copy as u64)
        }
        FdKind::ProcfsDirectory { .. } => {
            // Cannot read from directory with read() - must use getdents
            log::debug!("sys_read: Cannot read from /proc directory, use getdents instead");
            SyscallResult::Err(super::errno::EISDIR as u64)
        }
        FdKind::Epoll(_) => {
            // Cannot read from epoll fd directly
            SyscallResult::Err(super::errno::EINVAL as u64)
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
#[cfg(target_arch = "x86_64")]
pub fn sys_fork_with_frame(frame: &super::handler::SyscallFrame) -> SyscallResult {
    // Create a CpuContext from the syscall frame - this captures the ACTUAL register
    // values at the time of the syscall, not the stale values from the last context switch
    let parent_context = crate::task::thread::CpuContext::from_syscall_frame(frame);

    // Call fork with the complete parent context
    sys_fork_with_parent_context(parent_context)
}

/// sys_fork with full parent context - captures all registers from syscall frame
///
/// NOTE: No `arch_without_interrupts`/`without_interrupts` wrapper around
/// this function's body (#745 precheck C1). x86's PROCESS_MANAGER lock is a
/// bare spinlock with no interrupt masking of its own
/// (`process/mod.rs`'s `#[cfg(not(target_arch = "aarch64"))] manager()` arm).
///
/// Why holding it unmasked is safe here, re-derived at these bytes rather
/// than copied from the precheck (which is what C1(b) asked for, and what
/// #745 review round 2 M1 caught round 1 not doing). The census is
/// `grep -n 'crate::process::manager()\|crate::process::try_manager()\|with_process_manager'`
/// over `kernel/src/interrupts.rs`, `kernel/src/interrupts/context_switch.rs`
/// and `kernel/src/interrupts/timer.rs`: nine x86 interrupt-context PM
/// accesses. SEVEN are non-blocking `try_manager()` (`interrupts.rs:726`,
/// `:965`; `context_switch.rs:277`, `:601`, `:728`, `:1199`, `:1543`), so a
/// thread holding PM here never blocks a timer ISR --
/// `check_need_resched_and_switch` refuses the dispatch and re-arms
/// `need_resched` instead. The remaining TWO are blocking
/// `crate::process::with_process_manager` calls (`interrupts.rs:1421` in the
/// page-fault handler, `:1708` in the GPF handler), so the flat claim "every
/// x86 interrupt-context PM access is non-blocking" is FALSE. Both sit
/// inside `if from_userspace` process-kill arms, and a CPU executing this
/// kernel-mode fork is not taking a userspace fault, so neither is reachable
/// while this window is held on that CPU; on `-smp 1` (what every x86 gate
/// boots) that closes it outright, and on SMP the fork holder is runnable
/// and releases. That is 7 of 9 non-blocking and 2 of 9 blocking-but-
/// unreachable-from-here, re-derived at these bytes.
///
/// This is the same unmasked shape `sys_spawn`'s Window 2 has run in
/// production since #713. Wrapping the whole operation in a hardware
/// interrupt mask would be a STRICTLY LARGER change than anything aarch64's
/// fork ever needed (aarch64 keeps every PM window IRQ-off already) and
/// would make the ENTIRE fork non-preemptible; what masks inside this window
/// today is only what masks everywhere in the kernel -- the heap allocator's
/// own `arch_without_interrupts` bracket around each allocation
/// (`memory/heap.rs:34`-`57`; 1 of the 6 `without_interrupts` occurrences under
/// `kernel/src/memory` and `kernel/src/process`, and the only one this window
/// reaches now that TLS registration is hoisted out). It would also reproduce
/// the interrupt-masking
/// anti-pattern aarch64's own fork history already proved causes a
/// single-CPU deadlock (see
/// `arch_impl/aarch64/syscall_entry.rs::sys_fork_aarch64`'s postmortem
/// comment) -- just with a different lock inventory. See
/// `docs/planning/745-x86-fork/` for the full analysis.
/// claim-lint:ok: "aarch64 keeps every PM window IRQ-off" is the
/// `#[cfg(target_arch = "aarch64")]` arm of `manager()` in
/// kernel/src/process/mod.rs -- one arm, not a survey.
#[cfg(target_arch = "x86_64")]
fn sys_fork_with_parent_context(parent_context: crate::task::thread::CpuContext) -> SyscallResult {
    use super::errno::{EINVAL, ENOMEM, ESRCH};

    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) if id != 0 => id,
        _ => {
            log::error!("sys_fork: No current thread in scheduler (or idle thread)");
            return SyscallResult::Err(EINVAL as u64);
        }
    };

    // Window 1: look up the caller's PID under the PM lock, then drop it --
    // no I/O and no scheduler call inside this lock (mirrors sys_spawn's
    // own Window 1, #713 precheck C6).
    let parent_pid = {
        let manager_guard = crate::process::manager();
        match *manager_guard {
            Some(ref manager) => match manager.find_process_by_thread(current_thread_id) {
                Some((pid, _)) => pid,
                None => {
                    log::error!(
                        "sys_fork: Current thread {} not found in any process",
                        current_thread_id
                    );
                    return SyscallResult::Err(ESRCH as u64);
                }
            },
            None => {
                log::error!("sys_fork: Process manager not available");
                return SyscallResult::Err(ENOMEM as u64);
            }
        }
    };

    // Reclaim quiesced process resources AND scheduler-owned kernel stacks
    // before consuming another finite kernel-stack-pool slot -- mirrors
    // aarch64 fork's ordering and sys_spawn's own (#713 C8; #745 precheck
    // section 3.2 -- the process-resource reclaim call was missing here
    // entirely). No PM guard is live across either call (#745 precheck C4).
    // claim-lint:ok: guard-liveness across both calls is ratchet-pinned, with
    // its own delete mutations, in tests/fork_lock_order_structure.rs.
    crate::task::process_task::reclaim_deferred_process_resources();
    crate::task::scheduler::reclaim_terminated_threads();

    // Create the child page table OUTSIDE the PM lock -- heap/frame
    // allocation must not run with PM held (creation.rs's documented
    // MEMORY_INFO lock-order rationale, same as sys_spawn).
    let child_page_table = match crate::memory::process_memory::ProcessPageTable::new() {
        Ok(pt) => Box::new(pt),
        Err(e) => {
            log::error!("sys_fork: Failed to create child page table: {}", e);
            return SyscallResult::Err(ENOMEM as u64);
        }
    };

    // Window 2: fork under the PM lock. `fork_process_with_parent_context`
    // and `complete_fork` contain no logging of their own -- they run
    // entirely inside this lock, and x86's PM lock is a bare spinlock that
    // blocks all dispatch while held (#745 precheck C9); see their own doc
    // comments in manager.rs, which also name the one callee inside this
    // window that still does log (#756).
    // claim-lint:ok: "entirely inside this lock" is a statement about these two
    // functions' own call sites, both of which are the two lines below; the one
    // callee that escapes the no-logging property is #756.
    let mut manager_guard = crate::process::manager();
    let fork_result = match *manager_guard {
        Some(ref mut manager) => {
            manager.fork_process_with_parent_context(parent_pid, parent_context, child_page_table)
        }
        None => Err("Process manager not available"),
    };

    match fork_result {
        Ok(child_pid) => {
            // Extract the child's thread info while STILL under the PM
            // lock (no logging here either -- see above).
            let child_info = match *manager_guard {
                Some(ref mut manager) => manager.get_process_mut(child_pid).and_then(|process| {
                    process.main_thread.as_mut().map(|thread| {
                        let thread_id = thread.id;
                        let tls_block = thread.tls_block;
                        (thread_id, tls_block, Box::new(thread.publish_to_scheduler()))
                    })
                }),
                None => None,
            };

            let Some((child_thread_id, child_tls_block, child_thread)) = child_info else {
                // Defensive teardown, believed unreachable in practice:
                // `complete_fork`'s own invariant guarantees `main_thread`
                // is `Some` whenever `fork_process_with_parent_context`
                // returns `Ok` (it is set immediately before the row
                // insert this same call performed). Mirrors sys_spawn's
                // own Window-3 undo (#713 precheck C2) for defense in
                // depth rather than leaving a half-published row behind.
                // claim-lint:ok: "guarantees" here is the invariant
                // `complete_fork` establishes two statements before its own
                // `Ok` (set_main_thread, then the row insert) -- read it there;
                // this arm is defense in depth, not a proof obligation, and is
                // itself census-pinned in tests/teardown_structure.rs.
                if let Some(ref mut manager) = *manager_guard {
                    if let Some(parent) = manager.get_process_mut(parent_pid) {
                        parent.children.retain(|&pid| pid != child_pid);
                    }
                    manager.remove_from_ready_queue(child_pid);
                    manager.remove_process(child_pid);
                }
                drop(manager_guard);
                log::error!(
                    "sys_fork: Child process {} has no main thread after a successful fork",
                    child_pid.as_u64()
                );
                return SyscallResult::Err(ENOMEM as u64);
            };

            // Drop the PM lock BEFORE any logging or scheduler operations
            // (mirrors aarch64 fork's own ordering, and is required by the
            // creation-publication lock-order census, #745 precheck C5).
            drop(manager_guard);

            // TLS registration for the child, hoisted OUT of `complete_fork`
            // (#745 precheck C9/C10, review round 2 B2). `register_thread_tls`
            // masks interrupts, takes the global TLS_MANAGER lock and logs at
            // debug level; running it under the PM lock was an x86-only
            // divergence -- `complete_fork_aarch64` registers no TLS at all.
            // Here is both correct and the latest safe point: the child cannot
            // be dispatched until `spawn_front` below puts it on the ready
            // queue, and `context_switch`'s `switch_tls(thread_id)` (the only
            // consumer of this registration) runs on that dispatch, aborting
            // it if the thread is unregistered.
            if let Err(e) = crate::tls::register_thread_tls(child_thread_id, child_tls_block) {
                log::warn!(
                    "sys_fork: Failed to register TLS for child thread {}: {}",
                    child_thread_id,
                    e
                );
            }

            crate::tracing::providers::process::trace_spawn_front(
                current_thread_id as u16,
                child_thread_id as u16,
            );
            crate::task::scheduler::spawn_front(child_thread);

            log::info!(
                "sys_fork: Fork successful - parent {} gets child PID {}, thread {}",
                parent_pid.as_u64(),
                child_pid.as_u64(),
                child_thread_id
            );

            SyscallResult::Ok(child_pid.as_u64())
        }
        Err(e) => {
            drop(manager_guard);
            log::error!("sys_fork: Failed to fork process: {}", e);
            SyscallResult::Err(ENOMEM as u64)
        }
    }
}

#[cfg(target_arch = "x86_64")]
pub fn sys_fork() -> SyscallResult {
    // DEPRECATED: This function should not be used - use sys_fork_with_frame instead
    // to get the actual register values at syscall time.
    log::error!("sys_fork() called without frame - this path is deprecated and broken!");
    log::error!(
        "The syscall handler should use sys_fork_with_frame() to capture registers correctly."
    );
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
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
#[allow(unused_variables)]
#[allow(unreachable_code)]
pub fn sys_exec_with_frame(
    frame: &mut super::handler::SyscallFrame,
    program_name_ptr: u64,
    elf_data_ptr: u64,
) -> SyscallResult {
    crate::arch_without_interrupts(|| {
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
        // Owned data must live long enough for exec_process to borrow
        let mut _elf_vec_storage: Option<alloc::vec::Vec<u8>> = None;
        let mut _name_storage: Option<alloc::string::String> = None;
        let (elf_data, exec_program_name): (&[u8], Option<&str>) = if program_name_ptr != 0 {
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
            let name_len = name_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_bytes.len());
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
                _elf_vec_storage = Some(elf_vec);
                _name_storage = Some(alloc::string::String::from(program_name));
                (
                    _elf_vec_storage.as_ref().unwrap().as_slice(),
                    Some(_name_storage.as_ref().unwrap().as_str()),
                )
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
                (
                    crate::userspace_test::get_test_binary_static("hello_world"),
                    Some("hello_world"),
                )
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
#[cfg(all(target_arch = "x86_64", feature = "testing"))]
fn load_elf_from_ext2(path: &str) -> Result<Vec<u8>, i32> {
    use super::errno::EIO;
    use crate::fs::ext2;

    // Determine which filesystem to use based on path
    let is_home = ext2::is_home_path(path);
    let fs_path = if is_home {
        ext2::strip_home_prefix(path)
    } else {
        path
    };

    if is_home {
        let fs_guard = ext2::home_fs_read();
        let fs = fs_guard.as_ref().ok_or(EIO)?;
        load_elf_from_ext2_fs(fs, fs_path)
    } else {
        let fs_guard = ext2::root_fs_read();
        let fs = fs_guard.as_ref().ok_or(EIO)?;
        load_elf_from_ext2_fs(fs, fs_path)
    }
}

/// Inner helper for loading ELF from any ext2 filesystem instance.
#[cfg(all(target_arch = "x86_64", feature = "testing"))]
fn load_elf_from_ext2_fs(fs: &crate::fs::ext2::Ext2Fs, path: &str) -> Result<Vec<u8>, i32> {
    use super::errno::{EACCES, EIO, ENOTDIR};

    let inode_num = fs.resolve_path(path).map_err(|e| {
        if e.contains("not found") {
            super::errno::ENOENT
        } else {
            EIO
        }
    })?;

    let inode = fs.read_inode(inode_num).map_err(|_| EIO)?;

    if inode.is_dir() {
        return Err(ENOTDIR);
    }

    let perms = inode.permissions();
    if (perms & 0o100) == 0 {
        return Err(EACCES);
    }

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
#[cfg(target_arch = "x86_64")]
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

    let name_len = name_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(name_bytes.len());
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
                arg_ptr_bytes[0],
                arg_ptr_bytes[1],
                arg_ptr_bytes[2],
                arg_ptr_bytes[3],
                arg_ptr_bytes[4],
                arg_ptr_bytes[5],
                arg_ptr_bytes[6],
                arg_ptr_bytes[7],
            ]);

            // NULL pointer marks end of argv
            if arg_ptr == 0 {
                break;
            }

            // Read the argument string
            let arg_bytes = match copy_string_from_user(arg_ptr, MAX_ARG_LEN) {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::error!(
                        "sys_execv: Failed to read argv[{}] string at {:#x}: {}",
                        i,
                        arg_ptr,
                        e
                    );
                    return SyscallResult::Err(14); // EFAULT
                }
            };

            // Find null terminator and truncate
            let arg_len = arg_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(arg_bytes.len());
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
        let elf_data = elf_vec.as_slice();

        // Find current process
        let current_pid = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((pid, _)) = manager.find_process_by_thread(current_thread_id) {
                    pid
                } else {
                    log::error!(
                        "sys_execv: Thread {} not found in any process",
                        current_thread_id
                    );
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

        // CRITICAL SECTION: manager call, scheduler commit, CR3 install and frame patch —
        // masked together, same shape as the production arm below (#721 K3/K10).
        crate::arch_without_interrupts(|| {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                log::error!("sys_execv: Process manager not available");
                return SyscallResult::Err(12); // ENOMEM
            };

            let (new_entry_point, new_rsp, commit) = match manager.exec_process_with_argv(
                current_pid,
                elf_data,
                Some(program_name),
                &argv_slices,
            ) {
                Ok(value) => value,
                Err("exec blocked while CLONE_VM sibling shares old address space") => {
                    return SyscallResult::Err(11); // EAGAIN
                }
                Err(e) => {
                    log::error!("sys_execv: Failed to exec process: {}", e);
                    return SyscallResult::Err(12); // ENOMEM
                }
            };

            let new_cr3 = commit.new_page_table_root();

            // #721 K3/X1: read everything needed off the receipt above, then release PM
            // before taking the SCHEDULER lock inside commit.apply() — never read the
            // process manager again after this drop.
            drop(manager_guard);

            commit.apply();

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

            // Set up CR3 for the new process page table, off the commit receipt.
            log::info!("sys_execv: Setting next_cr3 to {:#x}", new_cr3);
            unsafe {
                crate::per_cpu::set_next_cr3(new_cr3);
                core::arch::asm!(
                    "mov gs:[80], {}",
                    in(reg) new_cr3,
                    options(nostack, preserves_flags)
                );
            }

            log::info!(
                "sys_execv: Frame updated - RIP={:#x}, RSP={:#x}",
                frame.rip,
                frame.rsp
            );

            SyscallResult::Ok(0)
        })
    }

    #[cfg(not(feature = "testing"))]
    {
        // #721: production exec. Resolve the /bin/ prefix inline and load via the
        // production-safe, zero-feature ext2 reader — mirrors sys_spawn's already-landed
        // pattern (#713); load_elf_from_ext2 above stays #[cfg(feature = "testing")]-only,
        // so calling it here would leave this arm silently ENOSYS-shaped again (#721 spec
        // section 2.1, #713 anti-vacuity / precheck section 4.6). This read happens with
        // interrupts enabled, before any lock is taken — see this function's own
        // top-of-file comment; do not move it inside the masked section below (X2).
        let resolved_path = if program_name.contains('/') {
            alloc::string::String::from(program_name)
        } else {
            alloc::format!("/bin/{}", program_name)
        };

        let elf_vec = match crate::boot::init_image::read_init_from_ext2(&resolved_path) {
            Ok(data) => data,
            Err(msg) => {
                let errno = match msg {
                    "init not found" => super::errno::ENOENT,
                    "init is a directory" => super::errno::EISDIR,
                    _ => super::errno::EIO,
                };
                return SyscallResult::Err(errno as u64);
            }
        };
        let elf_data = elf_vec.as_slice();

        // #721 K12: reclaim scheduler-owned kernel stacks and deferred process resources
        // before exec runs, mirroring sys_spawn's identical ordering (#713 precheck C8).
        // This mirrors, but does not fix, two separately-filed pools exec_process_with_argv
        // touches on every call: it allocates a fresh GuardedStack for its manually-mapped
        // user stack (manager.rs, "Create a dummy stack object since we manually mapped the
        // stack") from #720's never-reclaiming NEXT_USER_STACK_ADDR bump allocator, and the
        // previous GuardedStack it replaces is dropped into #583's no-op Drop (the frames
        // are never returned). reclaim_deferred_process_resources()/reclaim_terminated_threads()
        // reclaim kernel stacks and dead-process resources — neither touches either pool.
        // Before this PR, production x86 exec consumed nothing (ENOSYS); it is now a new
        // per-call consumer of #720's finite VA budget and #583's frame leak. The PM lock is
        // not held across either reclaim call.
        crate::task::process_task::reclaim_deferred_process_resources();
        crate::task::scheduler::reclaim_terminated_threads();

        // Find current process (unmasked PM window, matches the testing arm above)
        let current_pid = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((pid, _)) = manager.find_process_by_thread(current_thread_id) {
                    pid
                } else {
                    log::error!(
                        "sys_execv: Thread {} not found in any process",
                        current_thread_id
                    );
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

        let argv_slices: Vec<&[u8]> = argv_vec.iter().map(|v| v.as_slice()).collect();

        // CRITICAL SECTION: manager call, scheduler commit, CR3 install and frame patch —
        // masked together (#721 K10: mirrors aarch64's sys_exec_aarch64, which masks this
        // exact window for exec specifically). #713's sys_spawn deliberately does NOT mask
        // its own creation window, citing creation.rs's documented worry: disabling
        // interrupts while holding PROCESS_MANAGER and then acquiring MEMORY_INFO could
        // deadlock against a concurrent thread doing the reverse. That worry is stale by
        // the time exec ever runs: MEMORY_INFO (frame_allocator.rs) is a `spin::Once`
        // populated once at boot and read everywhere thereafter via `.get()` — a non-blocking
        // load, not a lock acquisition — and `allocate_frame()` itself is CAS-based, so there
        // is no second lock for a masked exec to block on while holding PROCESS_MANAGER.
        // aarch64 already masks this identical operation in production with no such hazard
        // materializing, so the same regime is followed here. (creation.rs's and sys_spawn's
        // own comments are stale in the same way; not rewritten here to keep this diff
        // scoped to exec.) The ext2 read and the argv/name parsing above both stay outside
        // this section (X2).
        crate::arch_without_interrupts(|| {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                log::error!("sys_execv: Process manager not available");
                return SyscallResult::Err(12); // ENOMEM
            };

            let (new_entry_point, new_rsp, commit) = match manager.exec_process_with_argv(
                current_pid,
                elf_data,
                Some(program_name),
                &argv_slices,
            ) {
                Ok(value) => value,
                Err("exec blocked while CLONE_VM sibling shares old address space") => {
                    return SyscallResult::Err(11); // EAGAIN
                }
                Err(e) => {
                    log::error!("sys_execv: Failed to exec process: {}", e);
                    return SyscallResult::Err(12); // ENOMEM
                }
            };

            let new_cr3 = commit.new_page_table_root();

            // #721 K3/X1: read everything needed off the receipt above, then release PM
            // before taking the SCHEDULER lock inside commit.apply() — the process manager
            // must never be touched again after this drop (mirrors aarch64's
            // sys_exec_aarch64: drop(manager_guard) before commit.apply() and before the
            // CR3 install below).
            drop(manager_guard);

            commit.apply();

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

            // Set up CR3 for the new process page table, off the commit receipt (K3: never
            // read the process manager after drop).
            log::info!("sys_execv: Setting next_cr3 to {:#x}", new_cr3);
            unsafe {
                crate::per_cpu::set_next_cr3(new_cr3);
                core::arch::asm!(
                    "mov gs:[80], {}",
                    in(reg) new_cr3,
                    options(nostack, preserves_flags)
                );
            }

            log::info!(
                "sys_execv: Frame updated - RIP={:#x}, RSP={:#x}",
                frame.rip,
                frame.rsp
            );

            SyscallResult::Ok(0)
        })
    }
}

/// sys_spawn - Create a new process directly from an ELF path (no fork).
///
/// x86_64 counterpart to aarch64's `sys_spawn_aarch64`
/// (`kernel/src/arch_impl/aarch64/syscall_entry.rs`). Avoids fork+exec
/// entirely: the child's address space, ELF image and argv/envp/auxv stack
/// are built directly by `ProcessManager::spawn_process`, and the child is
/// handed to the scheduler only after its main thread is confirmed to
/// exist (#713 precheck C2 — a process created but never scheduled is a
/// hard error, not a degraded success: init's `waitpid` loop has no exit arm
/// for a child that never runs).
///
/// arg1 = path_ptr (null-terminated C string to ELF binary path)
/// arg2 = argv_ptr (null-terminated array of string pointers, or NULL)
///
/// Returns: child PID on success, or `SyscallResult::Err(errno)` with a
/// positive errno (the negative-`i64`-as-`u64` return convention belongs to
/// aarch64's raw-syscall encoding, not this arch's `SyscallResult`).
#[cfg(target_arch = "x86_64")]
pub fn sys_spawn(path_ptr: u64, argv_ptr: u64) -> SyscallResult {
    use super::errno::{EFAULT, EINVAL, EIO, EISDIR, ENOENT, ENOMEM, ESRCH};

    if path_ptr == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Read the path from userspace. 256 bytes matches sys_execv_with_frame's
    // program-name budget above.
    let path_bytes = match copy_string_from_user(path_ptr, 256) {
        Ok(bytes) => bytes,
        Err(_) => return SyscallResult::Err(EFAULT as u64),
    };
    let path_len = path_bytes
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(path_bytes.len());
    let program_path = match core::str::from_utf8(&path_bytes[..path_len]) {
        Ok(s) => s,
        Err(_) => return SyscallResult::Err(EINVAL as u64),
    };

    // Read argv from userspace (mirrors sys_execv_with_frame's own loop above,
    // same MAX_ARGS/MAX_ARG_LEN budget; not factored into a shared helper —
    // #713 spec section 2.1 marks that pure hygiene, not load-bearing for
    // this fix, and it is skipped here to keep the diff minimal).
    let mut argv_vec: Vec<Vec<u8>> = Vec::new();
    if argv_ptr != 0 {
        const MAX_ARGS: usize = 64;
        const MAX_ARG_LEN: usize = 4096;

        for i in 0..MAX_ARGS {
            let ptr_addr = match argv_ptr.checked_add((i * 8) as u64) {
                Some(addr) => addr,
                None => return SyscallResult::Err(EFAULT as u64),
            };
            let arg_ptr_bytes = match copy_from_user(ptr_addr, 8) {
                Ok(bytes) => bytes,
                Err(_) => return SyscallResult::Err(EFAULT as u64),
            };
            let arg_ptr = u64::from_le_bytes([
                arg_ptr_bytes[0],
                arg_ptr_bytes[1],
                arg_ptr_bytes[2],
                arg_ptr_bytes[3],
                arg_ptr_bytes[4],
                arg_ptr_bytes[5],
                arg_ptr_bytes[6],
                arg_ptr_bytes[7],
            ]);

            if arg_ptr == 0 {
                break;
            }

            let arg_bytes = match copy_string_from_user(arg_ptr, MAX_ARG_LEN) {
                Ok(bytes) => bytes,
                Err(_) => return SyscallResult::Err(EFAULT as u64),
            };
            let arg_len = arg_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(arg_bytes.len());
            let mut arg = arg_bytes[..arg_len].to_vec();
            arg.push(0);
            argv_vec.push(arg);
        }
    }
    if argv_vec.is_empty() {
        let mut arg0 = program_path.as_bytes().to_vec();
        arg0.push(0);
        argv_vec.push(arg0);
    }

    // Resolve the /bin/ prefix inline (mirrors sys_spawn_aarch64's own inline
    // resolution) and load via the production-safe, zero-feature ext2 reader.
    // Deliberately NOT load_elf_from_ext2 above: that helper is
    // #[cfg(feature = "testing")]-gated, and calling it here would make
    // sys_spawn silently ENOSYS-shaped in the zero-feature production build
    // while looking implemented (#713 anti-vacuity / precheck section 4.6).
    let resolved_path = if program_path.contains('/') {
        alloc::string::String::from(program_path)
    } else {
        alloc::format!("/bin/{}", program_path)
    };

    let elf_vec = match crate::boot::init_image::read_init_from_ext2(&resolved_path) {
        Ok(data) => data,
        Err(msg) => {
            let errno = match msg {
                "init not found" => ENOENT,
                "init is a directory" => EISDIR,
                _ => EIO,
            };
            return SyscallResult::Err(errno as u64);
        }
    };
    let elf_data = elf_vec.as_slice();

    // Reclaim scheduler-owned kernel stacks and deferred process resources
    // BEFORE consuming another finite kernel-stack pool slot (#713 precheck
    // C8, mirrors sys_fork_with_parent_context's ordering and its own comment
    // above; the PM lock is not held across either call).
    crate::task::process_task::reclaim_deferred_process_resources();
    crate::task::scheduler::reclaim_terminated_threads();

    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(ESRCH as u64),
    };

    // Window 1: look up the caller's PID under the PM lock, then drop it — no
    // I/O and no scheduler call inside this lock (#713 precheck C6).
    let parent_pid = {
        let manager_guard = crate::process::manager();
        match *manager_guard {
            Some(ref manager) => match manager.find_process_by_thread(current_thread_id) {
                Some((pid, _)) => pid,
                None => return SyscallResult::Err(ESRCH as u64),
            },
            None => return SyscallResult::Err(ENOMEM as u64),
        }
    };

    let short_name = program_path
        .rsplit('/')
        .next()
        .unwrap_or(program_path)
        .trim_end_matches(".elf");
    let process_name = alloc::string::String::from(short_name);
    let argv_slices: Vec<&[u8]> = argv_vec.iter().map(|v| v.as_slice()).collect();

    // Window 2: create the child under the PM lock. No arch_without_interrupts
    // / without_interrupts wrapper here — matches creation.rs's documented
    // reasoning (masking around process creation risks a MEMORY_INFO
    // lock-order deadlock against concurrent frame allocation) and
    // sys_spawn_aarch64's own comment that the PM lock alone is sufficient
    // synchronization (#713 precheck C5/C6).
    let child_pid = {
        let mut manager_guard = crate::process::manager();
        match *manager_guard {
            Some(ref mut manager) => {
                manager.spawn_process(parent_pid, process_name, elf_data, &argv_slices)
            }
            None => Err("Process manager not available"),
        }
    };

    let child_pid = match child_pid {
        Ok(pid) => pid,
        Err("Parent process not found") => return SyscallResult::Err(ESRCH as u64),
        Err(_) => return SyscallResult::Err(ENOMEM as u64),
    };

    // Window 3: publish the child's main thread to the scheduler. A missing
    // main thread is a hard failure, not a degraded success (#713 precheck
    // C2) — tear the row down here, under the same lock that discovered the
    // problem, exactly like ProcessManager::hold_init_publication's own
    // failure arm.
    //
    // Undo all three creation effects, in reverse-chronological (LIFO) order
    // of when spawn_process/create_process_with_argv performed them: the
    // parent's `children` entry was pushed LAST (spawn_process), the
    // ready-queue entry second-to-last (create_process_with_argv), and the
    // row itself first (build_process_with_argv_at's insert, undone here by
    // `remove_process`). Leaving the `children` entry dangling would make
    // the parent's later `waitpid(-1)` block forever instead of returning
    // ECHILD: the `children.is_empty()` guard would no longer fire, and the
    // row lookup for the phantom child would just silently skip it.
    let scheduler_thread = {
        let mut manager_guard = crate::process::manager();
        match *manager_guard {
            Some(ref mut manager) => {
                let thread = manager.get_process_mut(child_pid).and_then(|process| {
                    process
                        .main_thread
                        .as_mut()
                        .map(|main_thread| Box::new(main_thread.publish_to_scheduler()))
                });
                if thread.is_none() {
                    if let Some(parent) = manager.get_process_mut(parent_pid) {
                        parent.children.retain(|&pid| pid != child_pid);
                    }
                    manager.remove_from_ready_queue(child_pid);
                    manager.remove_process(child_pid);
                }
                thread
            }
            None => None,
        }
    };

    let scheduler_thread = match scheduler_thread {
        Some(thread) => thread,
        None => return SyscallResult::Err(ENOMEM as u64),
    };

    // Outside every PM window, matching #713 precheck C6 and creation.rs's own
    // "spawn() internally uses without_interrupts" note.
    crate::task::scheduler::spawn(scheduler_thread);

    SyscallResult::Ok(child_pid.as_u64())
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
#[cfg(target_arch = "x86_64")]
pub fn sys_exec(program_name_ptr: u64, elf_data_ptr: u64) -> SyscallResult {
    crate::arch_without_interrupts(|| {
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
        // Owned data must live long enough for exec_process to borrow
        let mut _elf_vec_storage2: Option<alloc::vec::Vec<u8>> = None;
        let mut _name_storage2: Option<alloc::string::String> = None;
        let (_elf_data, _exec_program_name): (&[u8], Option<&str>) = if program_name_ptr != 0 {
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
            let name_len = name_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_bytes.len());
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
                _elf_vec_storage2 = Some(elf_vec);
                _name_storage2 = Some(alloc::string::String::from(program_name));
                (
                    _elf_vec_storage2.as_ref().unwrap().as_slice(),
                    Some(_name_storage2.as_ref().unwrap().as_str()),
                )
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
                (
                    crate::userspace_test::get_test_binary_static("hello_world"),
                    Some("hello_world"),
                )
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
    crate::arch_without_interrupts(|| {
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

            // If no process found and the id is the no-thread sentinel, there
            // is no caller to name a process for.
            if thread_id == 0 {
                log::info!("sys_getpid: no current thread (id 0 is the no-thread sentinel)");
                return SyscallResult::Ok(0);
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

/// sys_getppid - Get the parent process ID
pub fn sys_getppid() -> SyscallResult {
    crate::arch_without_interrupts(|| {
        // Get current thread ID from scheduler
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            // Find the process that owns this thread
            if let Some(ref manager) = *crate::process::manager() {
                if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                    // Return parent PID if set, otherwise 1 (init)
                    if let Some(parent) = process.parent {
                        return SyscallResult::Ok(parent.as_u64());
                    }
                    return SyscallResult::Ok(1); // init
                }
            }
        }
        SyscallResult::Ok(1) // Fallback: init is parent
    })
}

/// sys_exit_group - Terminate all threads in the process group
///
/// For now this is an alias for sys_exit since we are single-threaded per process.
pub fn sys_exit_group(exit_code: i32) -> SyscallResult {
    sys_exit(exit_code)
}

/// sys_set_tid_address - Store TID address for thread exit notification
///
/// Minimal implementation: just return the current thread ID.
pub fn sys_set_tid_address(_tidptr: u64) -> SyscallResult {
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        return SyscallResult::Ok(thread_id);
    }
    SyscallResult::Ok(0)
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
    log::debug!(
        "sys_waitpid: pid={}, status_ptr={:#x}, options={}",
        pid,
        status_ptr,
        options
    );

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

    log::debug!(
        "sys_waitpid: Current process PID={}, has {} children",
        current_pid.as_u64(),
        current_process.children.len()
    );

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
                log::debug!(
                    "sys_waitpid: PID {} is not a child of {}",
                    p,
                    current_pid.as_u64()
                );
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
                return complete_wait(
                    child_pid,
                    exit_code,
                    status_ptr,
                    &children_copy,
                    current_pid,
                );
            }

            // Child exists but not terminated
            if options & WNOHANG != 0 {
                log::debug!("sys_waitpid: WNOHANG set, child {} not terminated", p);
                return SyscallResult::Ok(0);
            }

            // Blocking wait: set BlockedOnChildExit FIRST, then re-check child.
            //
            // CRITICAL: This ordering prevents a lost-wakeup TOCTOU race:
            //   1. Set BlockedOnChildExit (now unblock_for_child_exit WILL find us)
            //   2. Re-check child state (catches exit during the race window)
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_child_exit();
            });

            // Re-check child state to close the race window
            {
                let mg = crate::process::manager();
                if let Some(ref manager) = *mg {
                    if let Some(child) = manager.get_process(target_pid) {
                        if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                            drop(mg);
                            crate::task::scheduler::with_scheduler(|sched| {
                                if let Some(thread) = sched.current_thread_mut() {
                                    thread.blocked_in_syscall = false;
                                    thread.set_ready();
                                }
                            });
                            return complete_wait(
                                target_pid,
                                exit_code,
                                status_ptr,
                                &children_copy,
                                current_pid,
                            );
                        }
                    }
                }
            }

            crate::per_cpu::preempt_enable();

            loop {
                // Check for pending signals that should interrupt this syscall
                if let Some(e) = crate::syscall::check_signals_for_eintr() {
                    // Signal pending - clean up thread state and return EINTR
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                            thread.set_ready();
                        }
                    });
                    crate::per_cpu::preempt_disable();
                    log::debug!(
                        "sys_waitpid: Thread {} interrupted by signal (EINTR)",
                        thread_id
                    );
                    return SyscallResult::Err(e as u64);
                }

                // Yield and halt - timer interrupt will switch to another thread
                // since current thread is blocked
                crate::task::scheduler::yield_current();
                crate::arch_halt_with_interrupts();

                // After being rescheduled, check if child terminated
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    if let Some(child) = manager.get_process(target_pid) {
                        if let crate::process::ProcessState::Terminated(exit_code) = child.state {
                            drop(manager_guard);
                            crate::per_cpu::preempt_disable();
                            return complete_wait(
                                target_pid,
                                exit_code,
                                status_ptr,
                                &children_copy,
                                current_pid,
                            );
                        }
                    }
                }
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
                            if let crate::process::ProcessState::Terminated(exit_code) = child.state
                            {
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
                return complete_wait(
                    child_pid,
                    exit_code,
                    status_ptr,
                    &children_copy,
                    current_pid,
                );
            }

            // No terminated children yet
            if options & WNOHANG != 0 {
                log::debug!("sys_waitpid: WNOHANG set, no children terminated");
                return SyscallResult::Ok(0);
            }

            // Blocking wait: same TOCTOU prevention as the pid>0 path.
            crate::task::scheduler::with_scheduler(|sched| {
                sched.block_current_for_child_exit();
            });

            // Re-check all children to close the race window
            {
                let mg = crate::process::manager();
                if let Some(ref manager) = *mg {
                    for &child_pid in &children_copy {
                        if let Some(child) = manager.get_process(child_pid) {
                            if let crate::process::ProcessState::Terminated(exit_code) = child.state
                            {
                                drop(mg);
                                crate::task::scheduler::with_scheduler(|sched| {
                                    if let Some(thread) = sched.current_thread_mut() {
                                        thread.blocked_in_syscall = false;
                                        thread.set_ready();
                                    }
                                });
                                return complete_wait(
                                    child_pid,
                                    exit_code,
                                    status_ptr,
                                    &children_copy,
                                    current_pid,
                                );
                            }
                        }
                    }
                }
            }

            crate::per_cpu::preempt_enable();

            loop {
                // Check for pending signals that should interrupt this syscall
                if let Some(e) = crate::syscall::check_signals_for_eintr() {
                    // Signal pending - clean up thread state and return EINTR
                    crate::task::scheduler::with_scheduler(|sched| {
                        if let Some(thread) = sched.current_thread_mut() {
                            thread.blocked_in_syscall = false;
                            thread.set_ready();
                        }
                    });
                    crate::per_cpu::preempt_disable();
                    log::debug!(
                        "sys_waitpid: Thread {} interrupted by signal (EINTR)",
                        thread_id
                    );
                    return SyscallResult::Err(e as u64);
                }

                // Yield and halt - timer interrupt will switch to another thread
                // since current thread is blocked
                crate::task::scheduler::yield_current();
                crate::arch_halt_with_interrupts();

                // After being rescheduled, check if any child terminated
                let manager_guard = crate::process::manager();
                if let Some(ref manager) = *manager_guard {
                    for &child_pid in &children_copy {
                        if let Some(child) = manager.get_process(child_pid) {
                            if let crate::process::ProcessState::Terminated(exit_code) = child.state
                            {
                                drop(manager_guard);
                                crate::per_cpu::preempt_disable();
                                return complete_wait(
                                    child_pid,
                                    exit_code,
                                    status_ptr,
                                    &children_copy,
                                    current_pid,
                                );
                            }
                        }
                    }
                }
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
    reaper: crate::process::ProcessId,
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

    log::debug!(
        "complete_wait: child {} exited with code {}, wstatus={:#x}{}",
        child_pid.as_u64(),
        exit_code,
        wstatus,
        if exit_code < 0 {
            " (signal termination)"
        } else {
            " (normal exit)"
        }
    );

    // P6a reap arm. Condition C3: the claim is taken under PM *before* any
    // status reaches userspace. Two concurrent waiters can both pass the scan
    // that produced `exit_code`; only the one that installs the claim may report
    // it, and the loser returns ECHILD having copied nothing.
    let mut claim_refused = false;
    if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
        let mut evicted = None;
        {
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut manager) = *manager_guard {
                if let Some((_parent_pid, parent)) = manager.find_process_by_thread_mut(thread_id) {
                    parent.children.retain(|&id| id != child_pid);
                }
                match manager.reap_row(child_pid, reaper, exit_code) {
                    crate::process::manager::ReapOutcome::Claimed(row) => evicted = row,
                    crate::process::manager::ReapOutcome::Refused => claim_refused = true,
                }
            }
        }
        // Condition C8: the row destructor runs after the guard is released.
        drop(evicted);
        log::debug!(
            "complete_wait: reap arm for child {} ({})",
            child_pid.as_u64(),
            if claim_refused { "refused" } else { "claimed" }
        );
    }
    if claim_refused {
        return SyscallResult::Err(super::errno::ECHILD as u64);
    }
    // Disclosed consequence of C3's ordering, and it is forced: the claim
    // commits the reap before the status is copied, so a `copy_to_user` that
    // faults now loses that status instead of leaving the child reapable. It
    // cannot be avoided while the claim is the arbiter — a row claimed by this
    // caller is a tombstone whether or not the copy lands, and re-opening it on
    // a fault would hand the same status to a second waiter, which is exactly
    // the defect C3 exists to close. Linux has the same wart on the same path.

    // Write status to userspace if pointer is valid
    if status_ptr != 0 {
        if let Err(e) = copy_to_user(
            status_ptr,
            &wstatus as *const i32 as u64,
            core::mem::size_of::<i32>(),
        ) {
            log::error!("complete_wait: Failed to write status: {}", e);
            return SyscallResult::Err(super::errno::EFAULT as u64);
        }
    }

    // CRITICAL: Clear the blocked_in_syscall flag now that the syscall is completing.
    // This ensures future context switches will restore userspace context normally.
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            if thread.blocked_in_syscall {
                thread.blocked_in_syscall = false;
                log::debug!(
                    "complete_wait: Cleared blocked_in_syscall flag for thread {}",
                    thread.id
                );
            }
        }
    });

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

    let mut manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => {
            log::error!("sys_fcntl: Failed to get process manager");
            return SyscallResult::Err(11); // EAGAIN
        }
    };

    let process = match manager_guard
        .as_mut()
        .and_then(|m| m.find_process_by_thread_mut(thread_id))
        .map(|(_, p)| p)
    {
        Some(p) => p,
        None => {
            log::error!("sys_fcntl: Failed to find process for thread {}", thread_id);
            return SyscallResult::Err(9); // EBADF
        }
    };

    match cmd {
        F_DUPFD => match process.fd_table.dup_at_least(fd, arg, false) {
            Ok(new_fd) => {
                log::debug!("sys_fcntl F_DUPFD: {} -> {}", fd, new_fd);
                SyscallResult::Ok(new_fd as u64)
            }
            Err(e) => SyscallResult::Err(e as u64),
        },
        F_DUPFD_CLOEXEC => match process.fd_table.dup_at_least(fd, arg, true) {
            Ok(new_fd) => {
                log::debug!("sys_fcntl F_DUPFD_CLOEXEC: {} -> {}", fd, new_fd);
                SyscallResult::Ok(new_fd as u64)
            }
            Err(e) => SyscallResult::Err(e as u64),
        },
        F_GETFD => match process.fd_table.get_fd_flags(fd) {
            Ok(flags) => {
                log::debug!("sys_fcntl F_GETFD: fd={} flags={}", fd, flags);
                SyscallResult::Ok(flags as u64)
            }
            Err(e) => SyscallResult::Err(e as u64),
        },
        F_SETFD => match process.fd_table.set_fd_flags(fd, arg as u32) {
            Ok(()) => {
                log::debug!("sys_fcntl F_SETFD: fd={} flags={}", fd, arg);
                SyscallResult::Ok(0)
            }
            Err(e) => SyscallResult::Err(e as u64),
        },
        F_GETFL => match process.fd_table.get_status_flags(fd) {
            Ok(flags) => {
                log::debug!("sys_fcntl F_GETFL: fd={} flags={:#x}", fd, flags);
                SyscallResult::Ok(flags as u64)
            }
            Err(e) => SyscallResult::Err(e as u64),
        },
        F_SETFL => match process.fd_table.set_status_flags(fd, arg as u32) {
            Ok(()) => {
                log::debug!("sys_fcntl F_SETFL: fd={} flags={:#x}", fd, arg);
                SyscallResult::Ok(0)
            }
            Err(e) => SyscallResult::Err(e as u64),
        },
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
pub fn sys_poll(fds_ptr: u64, nfds: u64, timeout: i32) -> SyscallResult {
    use crate::ipc::fd::FileDescriptor;
    use crate::ipc::poll::{self, events, PollFd};

    // Drain loopback queue for localhost connections (127.x.x.x, own IP).
    crate::net::drain_loopback_queue();

    // Validate parameters
    if fds_ptr == 0 && nfds > 0 {
        return SyscallResult::Err(14); // EFAULT
    }
    if nfds > 256 {
        return SyscallResult::Err(22); // EINVAL
    }
    if nfds == 0 {
        if timeout > 0 {
            // poll(NULL, 0, timeout) is a valid way to sleep
            let req_ns = (timeout as u64) * 1_000_000;
            let (s, n) = crate::time::get_monotonic_time_ns();
            let wake = (s as u64) * 1_000_000_000 + (n as u64) + req_ns;
            poll_block_until(wake);
        }
        return SyscallResult::Ok(0);
    }

    // Read pollfd array from userspace
    let mut pollfds: Vec<PollFd> = Vec::with_capacity(nfds as usize);
    unsafe {
        let src = fds_ptr as *const PollFd;
        for i in 0..nfds as usize {
            pollfds.push(core::ptr::read(src.add(i)));
        }
    }

    // Snapshot fd entries under PROCESS_MANAGER lock, then release it.
    let mut fd_snapshots: Vec<Option<FileDescriptor>> = Vec::with_capacity(nfds as usize);
    {
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

        for pollfd in pollfds.iter() {
            if pollfd.fd < 0 {
                fd_snapshots.push(None);
            } else {
                fd_snapshots.push(process.fd_table.get(pollfd.fd).cloned());
            }
        }
    }

    // Helper: scan all fds and set revents. Returns number of ready fds.
    let scan_fds = |pollfds: &mut [PollFd], snapshots: &[Option<FileDescriptor>]| -> u64 {
        let mut count = 0u64;
        for (i, pollfd) in pollfds.iter_mut().enumerate() {
            pollfd.revents = 0;
            if pollfd.fd < 0 {
                continue;
            }
            match &snapshots[i] {
                Some(fd_entry) => {
                    pollfd.revents = poll::poll_fd(fd_entry, pollfd.events);
                }
                None => {
                    pollfd.revents = events::POLLNVAL;
                }
            }
            if pollfd.revents != 0 {
                count += 1;
            }
        }
        count
    };

    // Initial scan
    let mut ready_count = scan_fds(&mut pollfds, &fd_snapshots);

    // If fds are ready or this is a non-blocking poll, return immediately
    if ready_count > 0 || timeout == 0 {
        unsafe {
            let dst = fds_ptr as *mut PollFd;
            for (i, pollfd) in pollfds.iter().enumerate() {
                core::ptr::write(dst.add(i), *pollfd);
            }
        }
        return SyscallResult::Ok(ready_count);
    }

    // timeout > 0 or timeout == -1 (infinite): block until fds ready or timeout
    // Use the same blocking pattern as nanosleep: block_current_for_timer +
    // preempt_enable + HLT loop. Wake every timer tick (~5ms at 200Hz) to
    // re-check fds for responsiveness.
    let (s, n) = crate::time::get_monotonic_time_ns();
    let now_ns = (s as u64) * 1_000_000_000 + (n as u64);
    // #693: the instant the blocking phase begins, kept for the timeout report
    // below. The entry scan above has already run and found no ready fd, so a
    // publication stamped after this instant is one that landed while this
    // thread was parked -- which is exactly the quantity #693 needed and could
    // not obtain.
    let entry_ns = now_ns;
    let deadline_ns = if timeout < 0 {
        u64::MAX // infinite — will keep looping until fds ready or signal
    } else {
        now_ns.saturating_add((timeout as u64) * 1_000_000)
    };

    // Block for a short interval (1ms) at a time so we can re-check fds.
    // Use block_current_for_timer to properly yield the CPU.
    // 1 ms. This is a re-check cadence, not a tick count: on x86 the PIT runs at
    // 200 Hz, so a tick is 5 ms and this interval is shorter than one tick. The
    // comment it replaces said "one timer tick at 1000Hz", which is true on
    // aarch64 and false on x86.
    let poll_interval_ns: u64 = 1_000_000;

    loop {
        let (s, n) = crate::time::get_monotonic_time_ns();
        let now = (s as u64) * 1_000_000_000 + (n as u64);
        if now >= deadline_ns {
            break; // Timeout expired
        }

        // Block for min(poll_interval, remaining time)
        let remaining = deadline_ns.saturating_sub(now);
        let sleep_until = now.saturating_add(remaining.min(poll_interval_ns));

        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current_for_timer(sleep_until);
        });

        #[cfg(target_arch = "aarch64")]
        crate::per_cpu_aarch64::preempt_enable();
        #[cfg(target_arch = "x86_64")]
        crate::per_cpu::preempt_enable();

        // HLT loop — sleep until timer expires
        loop {
            if let Some(_e) = crate::syscall::check_signals_for_eintr() {
                crate::task::scheduler::with_scheduler(|sched| {
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.blocked_in_syscall = false;
                        thread.wake_time_ns = None;
                        thread.set_ready();
                    }
                });
                #[cfg(target_arch = "aarch64")]
                crate::per_cpu_aarch64::preempt_disable();
                #[cfg(target_arch = "x86_64")]
                crate::per_cpu::preempt_disable();
                #[cfg(target_arch = "aarch64")]
                poll_ensure_address_space();
                // Write back current revents (all zero) before returning EINTR
                unsafe {
                    let dst = fds_ptr as *mut PollFd;
                    for (i, pollfd) in pollfds.iter().enumerate() {
                        core::ptr::write(dst.add(i), *pollfd);
                    }
                }
                return SyscallResult::Err(4); // EINTR
            }

            let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
                sched.wake_expired_timers();
                sched
                    .current_thread_mut()
                    .map(|t| t.state == crate::task::thread::ThreadState::BlockedOnTimer)
                    .unwrap_or(false)
            });

            if !still_blocked.unwrap_or(false) {
                break;
            }

            crate::task::scheduler::yield_current();
            // Enable interrupts and halt as ONE step, via the shared primitive
            // every other blocking syscall wait in this kernel uses (nanosleep,
            // futex, wait, pause, accept, connect, recv, the completion and
            // waitqueue waits, and the nine other sites in this file).
            //
            // #568: these two poll loops were the only blocking waits in the
            // tree that hand-inlined the halt instead, and the x86 half of that
            // hand-inline was a bare `hlt` with no `sti`. `yield_current()`
            // returns with interrupts disabled, so the bare `hlt` halted the CPU
            // with IF=0 -- a state only an NMI or a reset leaves. That is the
            // recorded #568 signature exactly: the guest stops with no
            // scheduling and no serial output, indefinitely. AArch64 escaped it
            // because its hand-inline spelled out `msr daifclr, #3` before the
            // `wfi`, which is precisely what the shared primitive emits there.
            //
            // Do not re-split this by architecture: the split is what let the
            // two arches drift apart in the first place.
            crate::arch_halt_with_interrupts();
        }

        // Clear blocked state + re-disable preemption
        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.blocked_in_syscall = false;
            }
        });

        #[cfg(target_arch = "aarch64")]
        crate::per_cpu_aarch64::preempt_disable();
        #[cfg(target_arch = "x86_64")]
        crate::per_cpu::preempt_disable();

        #[cfg(target_arch = "aarch64")]
        poll_ensure_address_space();

        // Re-check fds after waking
        crate::net::drain_loopback_queue();
        ready_count = scan_fds(&mut pollfds, &fd_snapshots);
        if ready_count > 0 {
            break;
        }
    }

    // #693: a blocking poll about to hand back 0 ready fds is the exact event
    // the issue was filed on, and on its own bytes it was silent. Say it now,
    // from kernel state, before the answer is unreachable.
    if ready_count == 0 {
        poll_report_timeout(&pollfds, &fd_snapshots, entry_ns, deadline_ns, timeout);
    }

    // Write updated pollfds back to userspace
    unsafe {
        let dst = fds_ptr as *mut PollFd;
        for (i, pollfd) in pollfds.iter().enumerate() {
            core::ptr::write(dst.add(i), *pollfd);
        }
    }

    SyscallResult::Ok(ready_count)
}

/// A blocking `poll()` shorter than this does not get the informational timeout
/// line. `bssh` and `bsshd` poll connected TCP fds on a 100 ms cadence
/// (`bssh.rs:160`, `bssh.rs:515`, `bsshd.rs:344` -- 3 of the 3 `io::poll`
/// calls in those two programs) and time out on most calls by design, so a
/// line each would be noise. 101 would exclude them too: what 120 is, is the
/// LARGEST bound that still ADMITS `poll_tcp_oracle`'s stage 1, which asks for
/// exactly 120 ms.
///
/// That is the property this constant needs. `poll_tcp_oracle`'s stage 1 asks
/// for exactly 120 ms and stage 4 for 150 ms, and both are built to time out,
/// so a boot that runs the oracle emits this line twice. That is the point: a reporting
/// path that runs only on the rare failure is a path whose death goes unnoticed
/// until the failure arrives and the path stays silent.
const POLL_TIMEOUT_REPORT_MS: i32 = 120;

/// Report what the kernel knows at the instant a blocking `poll()` gives up.
///
/// #693 is a `poll()` on a connected TCP fd that returned `ready=0`,
/// `revents=0x0000` after its full 5 s timeout while a peer process wrote and
/// exited. Two explanations fit that description -- the kernel lost a readiness
/// publication, or the peer had not published one yet -- and no line the boot
/// emitted separated them, so the issue stalled. The separating fact is the
/// publication instant of the polled connection, which lives in the kernel and
/// was simply not stated.
///
/// Both lines go to the console via `serial_println!`, not through `log::`, and
/// that is load-bearing rather than a style choice: on aarch64 the `log::` sink
/// is a second UART, and the aarch64 gate scripts boot QEMU with a single
/// `-serial file:` (3 of 3 checked: the service-sequence, strict and
/// prod-profile gates), so a marker emitted with `log::error!` is invisible to
/// the gates that are supposed to fail on it. The #584 futex oracle marker in
/// `syscall/futex_oracle.rs` is emitted the same way for the same reason.
///
/// Two lines come out of here, and they mean different things:
///
/// * `[POLL_TCP_READY_LOST]` -- bytes were published into this connection's
///   receive buffer strictly inside this poll's own window, they are STILL in
///   that buffer now, and this poll is nevertheless returning without `POLLIN`
///   for the fd. The last in-loop scan ran at the deadline and reads the buffer
///   live through the same connection lock, so it had to have seen those bytes.
///   That is a contradiction in kernel state, it is the genuine lost wake, and
///   gates fail on it. The "still in the buffer" clause is what keeps it sound:
///   without it, a publication consumed by another thread on the same fd before
///   the deadline would be reported as a loss. The strict `< deadline_ns` is the
///   other half: bytes that land in the microseconds between the last scan and
///   this report arrived after the poll's deadline, and reporting them would
///   make an on-time timeout look like a defect.
/// * `[POLL_TCP_TIMEOUT]` -- the ordinary case, emitted only for polls that
///   asked for at least `POLL_TIMEOUT_REPORT_MS`. It carries the publication
///   instant relative to entry, so "the peer had not published yet" is legible
///   directly rather than being reconstructed afterwards from two userspace
///   stamps and the interleaving of console prints, which is what the #693
///   investigation had to do.
fn poll_report_timeout(
    pollfds: &[crate::ipc::poll::PollFd],
    snapshots: &[Option<crate::ipc::fd::FileDescriptor>],
    entry_ns: u64,
    deadline_ns: u64,
    timeout_ms: i32,
) {
    use crate::ipc::poll::events;

    let mut reported_ordinary = false;
    for (i, pollfd) in pollfds.iter().enumerate() {
        if pollfd.fd < 0 || (pollfd.events & events::POLLIN) == 0 {
            continue;
        }
        let fd_entry = match snapshots.get(i).and_then(|s| s.as_ref()) {
            Some(entry) => entry,
            None => continue,
        };
        let (publish_ns, rx_len) = match crate::ipc::poll::tcp_rx_publication(fd_entry) {
            Some(state) => state,
            None => continue,
        };

        let published_in_window = publish_ns > entry_ns && publish_ns < deadline_ns;
        if published_in_window && rx_len > 0 && (pollfd.revents & events::POLLIN) == 0 {
            crate::serial_println!(
                "[POLL_TCP_READY_LOST] fd={} timeout_ms={} publish_after_entry_us={} before_deadline_us={} rx_len={} revents={:#06x}",
                pollfd.fd,
                timeout_ms,
                (publish_ns - entry_ns) / 1_000,
                (deadline_ns - publish_ns) / 1_000,
                rx_len,
                pollfd.revents
            );
            continue;
        }

        if timeout_ms >= POLL_TIMEOUT_REPORT_MS && !reported_ordinary {
            reported_ordinary = true;
            if published_in_window {
                crate::serial_println!(
                    "[POLL_TCP_TIMEOUT] fd={} timeout_ms={} publish=in_window publish_after_entry_us={} rx_len={} revents={:#06x}",
                    pollfd.fd,
                    timeout_ms,
                    (publish_ns - entry_ns) / 1_000,
                    rx_len,
                    pollfd.revents
                );
            } else {
                crate::serial_println!(
                    "[POLL_TCP_TIMEOUT] fd={} timeout_ms={} publish=none_in_window rx_len={} revents={:#06x}",
                    pollfd.fd,
                    timeout_ms,
                    rx_len,
                    pollfd.revents
                );
            }
        }
    }
}

/// Block the current thread until `wake_ns` (monotonic nanoseconds).
/// Used by poll(NULL, 0, timeout) as a simple sleep.
fn poll_block_until(wake_ns: u64) {
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_timer(wake_ns);
    });

    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_enable();
    #[cfg(target_arch = "x86_64")]
    crate::per_cpu::preempt_enable();

    loop {
        let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
            sched.wake_expired_timers();
            sched
                .current_thread_mut()
                .map(|t| t.state == crate::task::thread::ThreadState::BlockedOnTimer)
                .unwrap_or(false)
        });
        if !still_blocked.unwrap_or(false) {
            break;
        }
        crate::task::scheduler::yield_current();
        // Enable interrupts and halt as ONE step, via the shared primitive
        // every other blocking syscall wait in this kernel uses (nanosleep,
        // futex, wait, pause, accept, connect, recv, the completion and
        // waitqueue waits, and the nine other sites in this file).
        //
        // #568: these two poll loops were the only blocking waits in the
        // tree that hand-inlined the halt instead, and the x86 half of that
        // hand-inline was a bare `hlt` with no `sti`. `yield_current()`
        // returns with interrupts disabled, so the bare `hlt` halted the CPU
        // with IF=0 -- a state only an NMI or a reset leaves. That is the
        // recorded #568 signature exactly: the guest stops with no
        // scheduling and no serial output, indefinitely. AArch64 escaped it
        // because its hand-inline spelled out `msr daifclr, #3` before the
        // `wfi`, which is precisely what the shared primitive emits there.
        //
        // Do not re-split this by architecture: the split is what let the
        // two arches drift apart in the first place.
        crate::arch_halt_with_interrupts();
    }

    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
        }
    });

    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_disable();
    #[cfg(target_arch = "x86_64")]
    crate::per_cpu::preempt_disable();

    #[cfg(target_arch = "aarch64")]
    poll_ensure_address_space();
}

/// Restore TTBR0 after blocking in poll. Same pattern as nanosleep/waitpid.
#[cfg(target_arch = "aarch64")]
fn poll_ensure_address_space() {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if let Some(ref page_table) = process.page_table {
                let ttbr0_value = page_table.level_4_frame().start_address().as_u64();
                crate::arch_impl::aarch64::ttbr0::adopt_process_ttbr0(ttbr0_value);
            }
        }
    }
}

/// sys_ppoll - Poll file descriptors with timespec timeout
///
/// This implements the ppoll() syscall, which is the same as poll() but takes
/// a timespec instead of milliseconds and an optional signal mask (ignored).
///
/// Arguments:
/// - fds_ptr: Pointer to array of pollfd structures
/// - nfds: Number of file descriptors to poll
/// - timeout_ts_ptr: Pointer to timespec (NULL = infinite timeout)
/// - sigmask: Signal mask pointer (ignored)
/// - sigsetsize: Size of signal mask (ignored)
///
/// Delegates to sys_poll after converting timespec to milliseconds.
pub fn sys_ppoll(
    fds_ptr: u64,
    nfds: u64,
    timeout_ts_ptr: u64,
    _sigmask: u64,
    _sigsetsize: u64,
) -> SyscallResult {
    let timeout_ms: i32 = if timeout_ts_ptr == 0 {
        -1 // NULL timespec = infinite timeout
    } else {
        // Read timespec from userspace
        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }
        let ts = unsafe { core::ptr::read(timeout_ts_ptr as *const Timespec) };
        // Convert to milliseconds, clamping to i32 range
        let ms = ts
            .tv_sec
            .saturating_mul(1000)
            .saturating_add(ts.tv_nsec / 1_000_000);
        if ms > i32::MAX as i64 {
            i32::MAX
        } else {
            ms as i32
        }
    };
    sys_poll(fds_ptr, nfds, timeout_ms)
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
    use crate::ipc::fd::FileDescriptor;
    use crate::ipc::poll;

    log::debug!(
        "sys_select: nfds={}, readfds={:#x}, writefds={:#x}, exceptfds={:#x}, timeout={:#x}",
        nfds,
        readfds_ptr,
        writefds_ptr,
        exceptfds_ptr,
        _timeout_ptr
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
        readfds,
        writefds,
        exceptfds
    );

    // Snapshot fd entries under PROCESS_MANAGER lock, then release it.
    // Same rationale as sys_poll: avoid holding PM (which masks interrupts on
    // ARM64) across poll_fd/check_readable/check_writable calls that acquire
    // PTY buffer locks, TCP connection locks, etc.
    let mut fd_snapshots: Vec<(i32, Option<FileDescriptor>)> = Vec::new();
    {
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

        for fd in 0..nfds {
            let fd_bit = 1u64 << fd;
            let in_any =
                (readfds & fd_bit) != 0 || (writefds & fd_bit) != 0 || (exceptfds & fd_bit) != 0;
            if in_any {
                fd_snapshots.push((fd, process.fd_table.get(fd).cloned()));
            }
        }
        // manager_guard (and PM lock) dropped here
    }

    // Check each fd without holding PROCESS_MANAGER
    let mut ready_count: u64 = 0;
    let mut result_readfds: u64 = 0;
    let mut result_writefds: u64 = 0;
    let mut result_exceptfds: u64 = 0;

    for (fd, snapshot) in fd_snapshots.iter() {
        let fd_bit = 1u64 << fd;
        let in_readfds = (readfds & fd_bit) != 0;
        let in_writefds = (writefds & fd_bit) != 0;
        let in_exceptfds = (exceptfds & fd_bit) != 0;

        let fd_entry = match snapshot {
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
        unsafe {
            *(readfds_ptr as *mut u64) = result_readfds;
        }
    }
    if writefds_ptr != 0 {
        unsafe {
            *(writefds_ptr as *mut u64) = result_writefds;
        }
    }
    if exceptfds_ptr != 0 {
        unsafe {
            *(exceptfds_ptr as *mut u64) = result_exceptfds;
        }
    }

    log::debug!(
        "sys_select: {} fds ready (read={:#x}, write={:#x}, except={:#x})",
        ready_count,
        result_readfds,
        result_writefds,
        result_exceptfds
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

/// Take over the display from the kernel.
/// After this syscall, the calling process is responsible for rendering
/// to the framebuffer.
pub fn sys_take_over_display() -> SyscallResult {
    #[cfg(any(feature = "interactive", target_arch = "aarch64"))]
    {
        // Mark the calling process as the display owner
        use crate::syscall::memory_common::get_current_thread_id;
        if let Some(tid) = get_current_thread_id() {
            let mut mgr_guard = crate::process::manager();
            if let Some(ref mut mgr) = *mgr_guard {
                if let Some((_pid, process)) = mgr.find_process_by_thread_mut(tid) {
                    process.has_display_ownership = true;
                }
            }
        }

        // Tell the render thread to stop flushing the framebuffer.
        // BWM will handle all GPU operations via its own fb_flush() syscall.
        crate::graphics::render_task::set_display_taken();
    }
    SyscallResult::Ok(0)
}

/// Give back the display to the kernel.
/// Called by init when BWM crashes so the kernel can resume rendering.
pub fn sys_give_back_display() -> SyscallResult {
    SyscallResult::Ok(0)
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
    use crate::memory::cow_stats;

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

// =============================================================================
// Resource Limits and System Information
// =============================================================================

/// Linux rlimit structure
#[repr(C)]
#[derive(Copy, Clone)]
struct Rlimit {
    rlim_cur: u64,
    rlim_max: u64,
}

const RLIMIT_STACK: u64 = 3;
const RLIMIT_NOFILE: u64 = 7;
const RLIMIT_CORE: u64 = 4;
const RLIM_INFINITY: u64 = u64::MAX;

/// getrlimit - Get resource limits
pub fn sys_getrlimit(resource: u64, rlim_ptr: u64) -> SyscallResult {
    if rlim_ptr == 0 {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }

    let rlim = match resource {
        RLIMIT_STACK => Rlimit {
            rlim_cur: 8 * 1024 * 1024,
            rlim_max: RLIM_INFINITY,
        },
        RLIMIT_NOFILE => Rlimit {
            rlim_cur: 1024,
            rlim_max: 4096,
        },
        RLIMIT_CORE => Rlimit {
            rlim_cur: 0,
            rlim_max: RLIM_INFINITY,
        },
        _ => Rlimit {
            rlim_cur: RLIM_INFINITY,
            rlim_max: RLIM_INFINITY,
        },
    };

    if super::userptr::copy_to_user(rlim_ptr as *mut Rlimit, &rlim).is_err() {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }
    SyscallResult::Ok(0)
}

/// prlimit64 - Get/set resource limits
pub fn sys_prlimit64(
    _pid: u64,
    resource: u64,
    _new_rlim_ptr: u64,
    old_rlim_ptr: u64,
) -> SyscallResult {
    if old_rlim_ptr != 0 {
        return sys_getrlimit(resource, old_rlim_ptr);
    }
    SyscallResult::Ok(0)
}

/// Linux utsname structure
#[repr(C)]
#[derive(Clone, Copy)]
struct Utsname {
    sysname: [u8; 65],
    nodename: [u8; 65],
    release: [u8; 65],
    version: [u8; 65],
    machine: [u8; 65],
    domainname: [u8; 65],
}

fn copy_utsname_field(field: &mut [u8; 65], value: &[u8]) {
    let len = core::cmp::min(value.len(), 64);
    field[..len].copy_from_slice(&value[..len]);
}

/// uname - Get system identification
pub fn sys_uname(buf_ptr: u64) -> SyscallResult {
    if buf_ptr == 0 {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }

    let mut utsname = Utsname {
        sysname: [0u8; 65],
        nodename: [0u8; 65],
        release: [0u8; 65],
        version: [0u8; 65],
        machine: [0u8; 65],
        domainname: [0u8; 65],
    };

    copy_utsname_field(&mut utsname.sysname, b"Breenix");
    copy_utsname_field(&mut utsname.nodename, b"breenix");
    copy_utsname_field(&mut utsname.release, b"0.1.0");
    copy_utsname_field(&mut utsname.version, b"Breenix 0.1");
    #[cfg(target_arch = "x86_64")]
    copy_utsname_field(&mut utsname.machine, b"x86_64");
    #[cfg(target_arch = "aarch64")]
    copy_utsname_field(&mut utsname.machine, b"aarch64");
    copy_utsname_field(&mut utsname.domainname, b"(none)");

    if super::userptr::copy_to_user(buf_ptr as *mut Utsname, &utsname).is_err() {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }
    SyscallResult::Ok(0)
}

// =============================================================================
// Identity syscalls (getuid, geteuid, getgid, getegid, setuid, setgid)
// =============================================================================

/// getuid - Get real user ID
pub fn sys_getuid() -> SyscallResult {
    crate::arch_without_interrupts(|| {
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            if let Some(ref manager) = *crate::process::manager() {
                if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                    return SyscallResult::Ok(process.uid as u64);
                }
            }
        }
        SyscallResult::Ok(0)
    })
}

/// geteuid - Get effective user ID
pub fn sys_geteuid() -> SyscallResult {
    crate::arch_without_interrupts(|| {
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            if let Some(ref manager) = *crate::process::manager() {
                if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                    return SyscallResult::Ok(process.euid as u64);
                }
            }
        }
        SyscallResult::Ok(0)
    })
}

/// getgid - Get real group ID
pub fn sys_getgid() -> SyscallResult {
    crate::arch_without_interrupts(|| {
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            if let Some(ref manager) = *crate::process::manager() {
                if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                    return SyscallResult::Ok(process.gid as u64);
                }
            }
        }
        SyscallResult::Ok(0)
    })
}

/// getegid - Get effective group ID
pub fn sys_getegid() -> SyscallResult {
    crate::arch_without_interrupts(|| {
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            if let Some(ref manager) = *crate::process::manager() {
                if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                    return SyscallResult::Ok(process.egid as u64);
                }
            }
        }
        SyscallResult::Ok(0)
    })
}

/// setuid - Set user ID
///
/// If euid == 0 (root): set both uid and euid to the new value.
/// Otherwise: can only set euid to uid or euid (no-op).
pub fn sys_setuid(uid: u32) -> SyscallResult {
    crate::arch_without_interrupts(|| {
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut manager) = *manager_guard {
                if let Some((_pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                    if process.euid == 0 {
                        process.uid = uid;
                        process.euid = uid;
                    } else if uid == process.uid || uid == process.euid {
                        process.euid = uid;
                    } else {
                        return SyscallResult::Err(super::errno::EPERM as u64);
                    }
                    return SyscallResult::Ok(0);
                }
            }
        }
        SyscallResult::Err(super::errno::EPERM as u64)
    })
}

/// setgid - Set group ID
///
/// If euid == 0 (root): set both gid and egid to the new value.
/// Otherwise: can only set egid to gid or egid (no-op).
pub fn sys_setgid(gid: u32) -> SyscallResult {
    crate::arch_without_interrupts(|| {
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut manager) = *manager_guard {
                if let Some((_pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                    if process.euid == 0 {
                        process.gid = gid;
                        process.egid = gid;
                    } else if gid == process.gid || gid == process.egid {
                        process.egid = gid;
                    } else {
                        return SyscallResult::Err(super::errno::EPERM as u64);
                    }
                    return SyscallResult::Ok(0);
                }
            }
        }
        SyscallResult::Err(super::errno::EPERM as u64)
    })
}

// =============================================================================
// umask syscall
// =============================================================================

/// umask - Set file creation mask
///
/// Sets the process's file creation mask to `mask & 0o777` and returns the old mask.
pub fn sys_umask(mask: u32) -> SyscallResult {
    crate::arch_without_interrupts(|| {
        if let Some(thread_id) = crate::task::scheduler::current_thread_id() {
            let mut manager_guard = crate::process::manager();
            if let Some(ref mut manager) = *manager_guard {
                if let Some((_pid, process)) = manager.find_process_by_thread_mut(thread_id) {
                    let old = process.umask;
                    process.umask = mask & 0o777;
                    return SyscallResult::Ok(old as u64);
                }
            }
        }
        SyscallResult::Ok(0o022)
    })
}

// =============================================================================
// pread64 / pwrite64 syscalls
// =============================================================================

/// pread64 - Read from file at given offset without changing file position
pub fn sys_pread64(fd: i32, buf_ptr: u64, count: u64, offset: i64) -> SyscallResult {
    use crate::ipc::FdKind;

    if buf_ptr == 0 || count == 0 {
        // Linux checks the descriptor before honouring a degenerate transfer
        // (#670): a zero-length operation on a bad descriptor is EBADF, not 0.
        if let Err(e) = validate_fd_for_degenerate_transfer(fd) {
            return SyscallResult::Err(e);
        }
        return SyscallResult::Ok(0);
    }
    if offset < 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::EBADF as u64),
    };

    // Extract file info from fd table under process lock
    let fd_result: Result<(u64, usize), u64> = crate::arch_without_interrupts(|| {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                if let Some(fd_entry) = process.fd_table.get(fd) {
                    match &fd_entry.kind {
                        FdKind::RegularFile(file_ref) => {
                            let file = file_ref.lock();
                            return Ok((file.inode_num, file.mount_id));
                        }
                        FdKind::PipeRead(_) | FdKind::PipeWrite(_) => {
                            return Err(super::errno::ESPIPE as u64);
                        }
                        _ => return Err(super::errno::ESPIPE as u64),
                    }
                }
                return Err(super::errno::EBADF as u64);
            }
            Err(super::errno::EBADF as u64)
        } else {
            Err(super::errno::EBADF as u64)
        }
    });

    let (inode_num, mount_id) = match fd_result {
        Ok((ino, mid)) => (ino, mid),
        Err(e) => return SyscallResult::Err(e),
    };

    let file_offset = offset as u64;

    // Read from ext2 at the given offset (no process lock held)
    use crate::fs::ext2;
    let read_fn = |fs: &ext2::Ext2Fs| -> SyscallResult {
        let inode = match fs.read_inode(inode_num as u32) {
            Ok(i) => i,
            Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
        };
        let file_size = inode.size();
        if file_offset >= file_size {
            return SyscallResult::Ok(0);
        }
        let to_read = core::cmp::min(count, file_size - file_offset) as usize;
        match fs.read_file_range(&inode, file_offset, to_read) {
            Ok(data) => {
                let actual = core::cmp::min(data.len(), to_read);
                unsafe {
                    core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr as *mut u8, actual);
                }
                SyscallResult::Ok(actual as u64)
            }
            Err(_) => SyscallResult::Err(super::errno::EIO as u64),
        }
    };

    let is_home = ext2::home_mount_id().map_or(false, |id| id == mount_id);
    if is_home {
        let fs_guard = ext2::home_fs_read();
        match fs_guard.as_ref() {
            Some(fs) => read_fn(fs),
            None => SyscallResult::Err(super::errno::EIO as u64),
        }
    } else {
        let fs_guard = ext2::root_fs_read();
        match fs_guard.as_ref() {
            Some(fs) => read_fn(fs),
            None => SyscallResult::Err(super::errno::EIO as u64),
        }
    }
}

/// pwrite64 - Write to file at given offset without changing file position
pub fn sys_pwrite64(fd: i32, buf_ptr: u64, count: u64, offset: i64) -> SyscallResult {
    use crate::ipc::FdKind;

    if buf_ptr == 0 || count == 0 {
        // Linux checks the descriptor before honouring a degenerate transfer
        // (#670): a zero-length operation on a bad descriptor is EBADF, not 0.
        if let Err(e) = validate_fd_for_degenerate_transfer(fd) {
            return SyscallResult::Err(e);
        }
        return SyscallResult::Ok(0);
    }
    if offset < 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::EBADF as u64),
    };

    // Extract file info from fd table under process lock
    let fd_result: Result<(u64, usize), u64> = crate::arch_without_interrupts(|| {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                if let Some(fd_entry) = process.fd_table.get(fd) {
                    match &fd_entry.kind {
                        FdKind::RegularFile(file_ref) => {
                            let file = file_ref.lock();
                            return Ok((file.inode_num, file.mount_id));
                        }
                        FdKind::PipeRead(_) | FdKind::PipeWrite(_) => {
                            return Err(super::errno::ESPIPE as u64);
                        }
                        _ => return Err(super::errno::ESPIPE as u64),
                    }
                }
                return Err(super::errno::EBADF as u64);
            }
            Err(super::errno::EBADF as u64)
        } else {
            Err(super::errno::EBADF as u64)
        }
    });

    let (inode_num, mount_id) = match fd_result {
        Ok((ino, mid)) => (ino, mid),
        Err(e) => return SyscallResult::Err(e),
    };

    let file_offset = offset as u64;

    // Read user data (no process lock held)
    let data = match copy_from_user(buf_ptr, count as usize) {
        Ok(d) => d,
        Err(_) => return SyscallResult::Err(super::errno::EFAULT as u64),
    };

    use crate::fs::ext2;
    let write_fn = |fs: &mut ext2::Ext2Fs| -> SyscallResult {
        match fs.write_file_range(inode_num as u32, file_offset, &data) {
            Ok(written) => SyscallResult::Ok(written as u64),
            Err(_) => SyscallResult::Err(super::errno::EIO as u64),
        }
    };

    let is_home = ext2::home_mount_id().map_or(false, |id| id == mount_id);
    if is_home {
        let mut fs_guard = ext2::home_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => write_fn(fs),
            None => SyscallResult::Err(super::errno::EIO as u64),
        }
    } else {
        let mut fs_guard = ext2::root_fs_write();
        match fs_guard.as_mut() {
            Some(fs) => write_fn(fs),
            None => SyscallResult::Err(super::errno::EIO as u64),
        }
    }
}
