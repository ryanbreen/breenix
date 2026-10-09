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
    // An unmapped or unreadable user page is a failed copy, not a kernel fault.
    super::userptr::read_user_bytes(buffer.as_mut_ptr(), user_ptr, len)
        .map_err(|_| "unmapped userspace address")?;
    // SAFETY: read_user_bytes wrote all `len` bytes of the reserved capacity.
    unsafe { buffer.set_len(len) };

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

        let mut byte = 0u8;
        super::userptr::read_user_bytes(&mut byte, addr, 1)
            .map_err(|_| "unmapped userspace address")?;
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
    // using the process's page table, which has both kernel and user mappings.
    // An unmapped or read-only user page is a failed copy, not a kernel fault.
    super::userptr::write_user_bytes(user_ptr, kernel_ptr as *const u8, len)
        .map_err(|_| "unmapped or read-only userspace address")
}

/// sys_exit - Terminate the current process
pub fn sys_exit(exit_code: i32) -> SyscallResult {
    log::debug!("USERSPACE: sys_exit called with code: {}", exit_code);

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
            if !crate::task::userspace_completion::reporter_started() {
                report_userspace_completion();
            }
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


/// One line of the userspace report. x86 sends it through `log`, as the gate
/// scorers expect; ARM64 installs no `log` backend, so it prints to serial.
macro_rules! report_line {
    ($level:ident, $($arg:tt)*) => {{
        #[cfg(target_arch = "x86_64")]
        log::$level!($($arg)*);
        #[cfg(target_arch = "aarch64")]
        crate::serial_println!($($arg)*);
    }};
}

/// Publish the userspace verdict outside process-manager and scheduler locks.
pub(crate) fn report_userspace_completion() {
    if USERSPACE_TEST_COMPLETE.swap(true, Ordering::SeqCst) {
        return;
    }
    let failure_records = crate::task::exit_tally::snapshot_failures();
    let failures = failure_records.as_slice();
    crate::arch_without_interrupts(|| {
        // No more userspace threads remaining
        report_line!(info, "No more userspace threads remaining");

        // Wake the keyboard task to ensure it can process any pending input
        #[cfg(target_arch = "x86_64")]
        {
            crate::keyboard::stream::wake_keyboard_task();
            report_line!(info, "Woke keyboard task to ensure input processing continues");
        }

        // Signal that userspace testing is complete with clear markers
        report_line!(info, "🎯 USERSPACE TEST COMPLETE - All processes finished");
        let (exited, nonzero) = crate::task::exit_tally::totals();


        report_line!(
            info,
            "TEST_TALLY: exited={} nonzero={} failed=[{}] started={}",
            exited,
            nonzero,
            crate::task::exit_tally::FailureList::new(failures, nonzero),
            crate::task::exit_tally::started()
        );
        report_line!(
            info,
            "[smp] user-thread dispatches per CPU:{}",
            crate::task::user_dispatch::PerCpu
        );
        // The scheduler milestone's stage: more than one CPU online, and every
        // one of them ran a user thread.
        let (online, ran_user_work) = crate::task::user_dispatch::coverage();
        if online > 1 && ran_user_work == online {
            report_line!(
                info,
                "[smp] user work ran on every online CPU ({} of {})",
                ran_user_work,
                online
            );
        }

        if nonzero == 0 {
            report_line!(info, "=====================================");
            report_line!(info, "✅ USERSPACE EXECUTION SUCCESSFUL ✅");
            report_line!(info, "✅ Ring 3 execution confirmed       ✅");
            report_line!(info, "✅ System calls working correctly   ✅");
            report_line!(info, "✅ Process lifecycle complete       ✅");
            report_line!(info, "=====================================");
            report_line!(info, "🏁 TEST RUNNER: All tests passed - you can exit QEMU now 🏁");
        } else {
            report_line!(
                error,
                "🚨 Failing userspace processes: {} 🚨",
                crate::task::exit_tally::FailureList::new(failures, nonzero)
            );
            report_line!(
                error,
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
        report_line!(info, "USERSPACE TEST REPORT DONE");
    });
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
/// descriptor table has no such entry, or the entry is a regular file whose
/// open file description lacks the access the transfer needs (`write` says
/// which), as the ordinary path refuses it. Kernel threads have no descriptor
/// table at all, so they keep whatever fallback their handler already applied.
///
/// A regular file not opened for the transfer's direction (`for_write`) fails
/// with `EBADF` too, as the ordinary path does.
///
/// This runs only on the degenerate path. The ordinary path already performs
/// the same lookup, so no non-degenerate call gains work.
pub(crate) fn validate_fd_for_degenerate_transfer(fd: i32, write: bool) -> Result<(), u64> {
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
        match process.fd_table.get(fd) {
            Some(entry)
                if matches!(entry.kind, crate::ipc::FdKind::RegularFile(_))
                    && !(if write { entry.writable() } else { entry.readable() }) =>
            {
                Err(super::errno::EBADF as u64)
            }
            Some(_) => Ok(()),
            None => Err(super::errno::EBADF as u64),
        }
    })
}

/// sys_write - Write to a file descriptor
///
/// Supports stdout/stderr (serial port) and pipe write ends.
pub fn sys_write(fd: u64, buf_ptr: u64, count: u64) -> SyscallResult {
    write_with_limit(fd, buf_ptr, count, true)
}

/// A vectored call that has already transferred data must not signal for its
/// next vector merely reaching the limit; it returns the successful prefix.
pub(super) fn write_vector(fd: u64, buf_ptr: u64, count: u64, first: bool) -> SyscallResult {
    write_with_limit(fd, buf_ptr, count, first)
}

fn write_with_limit(fd: u64, buf_ptr: u64, count: u64, signal_limit: bool) -> SyscallResult {
    use crate::ipc::FdKind;

    // Note: Logging removed from hot path to prevent stack overflow.
    // Each log call in interactive mode writes to the Logs terminal,
    // which adds significant stack depth during syscall handling.

    // Validate buffer pointer and count
    if buf_ptr == 0 || count == 0 {
        // Linux checks the descriptor before honouring a degenerate transfer
        // (#670): a zero-length operation on a bad descriptor is EBADF, not 0.
        if let Err(e) = validate_fd_for_degenerate_transfer(fd as i32, true) {
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
            is_nonblocking: bool,
        },
        RegularFile {
            file: alloc::sync::Arc<spin::Mutex<crate::ipc::fd::RegularFile>>,
            append: bool,
            unprivileged: bool,
            size_limit: u64,
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
                is_nonblocking: (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK)
                    != 0,
            },
            FdKind::PipeRead(_) => WriteOperation::Ebadf,
            FdKind::FifoWrite(_path, pipe_buffer, _) => WriteOperation::Fifo {
                pipe_buffer: pipe_buffer.clone(),
                is_nonblocking: (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK)
                    != 0,
            },
            FdKind::FifoRead(_, _, _) => WriteOperation::Ebadf,
            FdKind::TcpSocket(_) => WriteOperation::Enotconn,
            FdKind::TcpListener(_) => WriteOperation::Enotconn,
            FdKind::TcpConnection(conn_id) => WriteOperation::TcpConnection { conn_id: *conn_id },
            FdKind::UdpSocket(_) => WriteOperation::Eopnotsupp, // UDP must use sendto
            FdKind::UnixStream(socket) => WriteOperation::UnixStream {
                socket: socket.clone(),
                is_nonblocking: (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK)
                    != 0,
            },
            FdKind::UnixSocket(_) => WriteOperation::Enotconn, // Unconnected Unix socket
            FdKind::UnixListener(_) => WriteOperation::Enotconn, // Listener can't write
            // A description opened O_RDONLY is not writable (EBADF).
            FdKind::RegularFile(_) if !fd_entry.writable() => WriteOperation::Ebadf,
            FdKind::RegularFile(file) => WriteOperation::RegularFile {
                file: file.clone(),
                append: (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_APPEND) != 0,
                unprivileged: !process.cred.privileged(),
                size_limit: process.limits.get(crate::process::limits::FSIZE).soft,
            },
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
        }
        | WriteOperation::Fifo {
            pipe_buffer,
            is_nonblocking,
        } => super::blocking_io::write_pipe(&pipe_buffer, &buffer, is_nonblocking),
        WriteOperation::UnixStream {
            socket,
            is_nonblocking,
        } => {
            let writer = socket.lock().writer();
            super::blocking_io::write_unix(writer, &buffer, is_nonblocking)
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
                        super::signal::raise_sigpipe();
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
        WriteOperation::RegularFile {
            file,
            append,
            unprivileged,
            size_limit,
        } => {
            // Write to ext2 regular file
            let (handle, position, file_mount_id) = {
                let file_guard = file.lock();
                (
                    file_guard.handle.clone(),
                    file_guard.position,
                    file_guard.mount_id,
                )
            };

            let inode_num = handle.object.key.inode;
            // Dispatch to correct filesystem based on mount_id
            let is_home = crate::fs::ext2::home_mount_id().map_or(false, |id| id == file_mount_id);

            let (write_offset, bytes_written) = if is_home {
                let mut fs_guard = crate::fs::ext2::home_fs_write();
                let fs = match fs_guard.as_mut() {
                    Some(fs) => fs,
                    None => return SyscallResult::Err(super::errno::ENOSYS as u64),
                };
                if handle.verify(fs).is_err() { return SyscallResult::Err(super::errno::EIO as u64); }
                let wo = if append {
                    match fs.read_inode(inode_num as u32) {
                        Ok(inode) => inode.size(),
                        Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
                    }
                } else {
                    position
                };
                let length = match super::resource::write_length(wo, buffer.len(), size_limit) {
                    Ok(n) => n,
                    Err(e) => {
                        drop(fs_guard);
                        if signal_limit {
                            super::resource::signal_fsize();
                        }
                        return SyscallResult::Err(e);
                    }
                };
                let bw = match fs.write_file_range_as(
                    inode_num as u32,
                    wo,
                    &buffer[..length],
                    unprivileged,
                ) {
                    Ok(n) => n,
                    Err(error) => {
                        return SyscallResult::Err(crate::memory::file_map::mutation_errno(error))
                    }
                };
                (wo, bw)
            } else {
                let mut fs_guard = crate::fs::ext2::root_fs_write();
                let fs = match fs_guard.as_mut() {
                    Some(fs) => fs,
                    None => return SyscallResult::Err(super::errno::ENOSYS as u64),
                };
                if handle.verify(fs).is_err() { return SyscallResult::Err(super::errno::EIO as u64); }
                let wo = if append {
                    match fs.read_inode(inode_num as u32) {
                        Ok(inode) => inode.size(),
                        Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
                    }
                } else {
                    position
                };
                let length = match super::resource::write_length(wo, buffer.len(), size_limit) {
                    Ok(n) => n,
                    Err(e) => {
                        drop(fs_guard);
                        if signal_limit {
                            super::resource::signal_fsize();
                        }
                        return SyscallResult::Err(e);
                    }
                };
                let bw = match fs.write_file_range_as(
                    inode_num as u32,
                    wo,
                    &buffer[..length],
                    unprivileged,
                ) {
                    Ok(n) => n,
                    Err(error) => {
                        return SyscallResult::Err(crate::memory::file_map::mutation_errno(error))
                    }
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
        if let Err(e) = validate_fd_for_degenerate_transfer(fd as i32, false) {
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
    // #821 keeps `reader_pid`: the stdin arm below is the console's
    // thread-context consumer, and the pid the input IRQ entry must not block
    // on PROCESS_MANAGER to resolve is already in hand right here.
    let (reader_pid, process) = match &*manager_guard {
        Some(manager) => match manager.find_process_by_thread(thread_id) {
            Some((pid, p)) => (pid, p),
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

            // #821: take any foreground-pgrp adoption the input IRQ entry
            // deferred. Deliberately after the guard is dropped, so this holds
            // no PROCESS_MANAGER while it touches the TTY's own locks.
            crate::tty::driver::adopt_foreground_pgrp_from_reader(reader_pid);

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
                            if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
                                // Not preemptible from here: a kill must never find this thread
                                // switched out while it holds the lock its waiter list is under.
                                crate::per_cpu::preempt_disable();
                                // Signal pending - unblock and return EINTR
                                crate::ipc::stdin::unregister_blocked_reader(thread_id);
                                crate::task::scheduler::with_scheduler(|sched| {
                                    if let Some(thread) = sched.current_thread_mut() {
                                        thread.blocked_in_syscall = false;
                                        thread.set_ready();
                                    }
                                });
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
                (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
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
                            if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
                                // Not preemptible from here: a kill must never find this thread
                                // switched out while it holds the lock its waiter list is under.
                                crate::per_cpu::preempt_disable();
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
        FdKind::FifoRead(_path, pipe_buffer, _) => {
            // FIFO read - with blocking support
            let is_nonblocking =
                (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
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
                            if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
                                // Not preemptible from here: a kill must never find this thread
                                // switched out while it holds the lock its waiter list is under.
                                crate::per_cpu::preempt_disable();
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
        FdKind::FifoWrite(_, _, _) => {
            // Can't read from write end of FIFO
            SyscallResult::Err(9) // EBADF
        }
        FdKind::UdpSocket(_) => {
            // Can't read from UDP socket - must use recvfrom
            log::error!("sys_read: Cannot read from UDP socket, use recvfrom instead");
            SyscallResult::Err(95) // EOPNOTSUPP
        }
        // A description opened O_WRONLY is not readable (EBADF).
        FdKind::RegularFile(_) if !fd_entry.readable() => SyscallResult::Err(9),
        FdKind::RegularFile(file_ref) => {
            // Read from ext2 regular file.
            //
            // CRITICAL: clone the Arc and extract values while PM lock held, then
            // drop PM lock BEFORE doing disk I/O.  On ARM64 the PM lock disables ALL
            // IRQs, and AHCI completions arrive as interrupts — holding the lock
            // during disk I/O deadlocks the system.
            let file_ref_owned = file_ref.clone();
            let (handle, position, file_mount_id) = {
                let file = file_ref.lock();
                (file.handle.clone(), file.position, file.mount_id)
            };
            let inode_num = handle.object.key.inode;
            // Release PM lock now — disk I/O below needs IRQs enabled.
            drop(manager_guard);

            // Dispatch to correct filesystem based on mount_id
            let is_home = crate::fs::ext2::home_mount_id().map_or(false, |id| id == file_mount_id);
            let (data, update_atime) = if is_home {
                let fs_guard = crate::fs::ext2::home_fs_read();
                let fs = match fs_guard.as_ref() {
                    Some(fs) => fs,
                    None => {
                        log::error!("sys_read: ext2 home filesystem not mounted");
                        return SyscallResult::Err(super::errno::ENOSYS as u64);
                    }
                };
                if handle.verify(fs).is_err() { return SyscallResult::Err(super::errno::EIO as u64); }
                let inode = match fs.read_inode(inode_num as u32) {
                    Ok(inode) => inode,
                    Err(e) => {
                        log::error!("sys_read: Failed to read inode {}: {}", inode_num, e);
                        return SyscallResult::Err(super::errno::EIO as u64);
                    }
                };
                match fs.read_file_range_coherent(inode_num, &inode, position, count as usize) {
                    Ok(data) => (data, inode.needs_atime_update()),
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
                if handle.verify(fs).is_err() { return SyscallResult::Err(super::errno::EIO as u64); }
                let inode = match fs.read_inode(inode_num as u32) {
                    Ok(inode) => inode,
                    Err(e) => {
                        log::error!("sys_read: Failed to read inode {}: {}", inode_num, e);
                        return SyscallResult::Err(super::errno::EIO as u64);
                    }
                };
                match fs.read_file_range_coherent(inode_num, &inode, position, count as usize) {
                    Ok(data) => (data, inode.needs_atime_update()),
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

            if bytes_read > 0 && update_atime {
                // Timestamp persistence must not turn a completed read into EIO.
                let _ = super::fs::update_read_atime(inode_num as u32, file_mount_id);
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
            let is_nonblocking =
                (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
            drop(manager_guard);
            let mut user_buf = alloc::vec![0u8; count as usize];
            let result = match device_type {
                crate::fs::devfs::DeviceType::Console | crate::fs::devfs::DeviceType::Tty => {
                    super::blocking_io::read_console(&mut user_buf, is_nonblocking).map_err(|e| -e)
                }
                _ => crate::fs::devfs::device_read(device_type, &mut user_buf),
            };
            match result {
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
                (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
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
                    if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
                        // Not preemptible from here: a kill must never find this thread
                        // switched out while it holds the lock its waiter list is under.
                        crate::per_cpu::preempt_disable();
                        // Signal pending - clean up and return EINTR
                        crate::net::tcp::tcp_unregister_recv_waiter(&conn_id, thread_id);
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                thread.set_ready();
                            }
                        });
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
                (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
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
                    if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
                        // Not preemptible from here: a kill must never find this thread
                        // switched out while it holds the lock its waiter list is under.
                        crate::per_cpu::preempt_disable();
                        pair.unregister_master_waiter(thread_id);
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                thread.set_ready();
                            }
                        });
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
                (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
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
                    if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
                        // Not preemptible from here: a kill must never find this thread
                        // switched out while it holds the lock its waiter list is under.
                        crate::per_cpu::preempt_disable();
                        pair.unregister_slave_waiter(thread_id);
                        crate::task::scheduler::with_scheduler(|sched| {
                            if let Some(thread) = sched.current_thread_mut() {
                                thread.blocked_in_syscall = false;
                                thread.set_ready();
                            }
                        });
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
                (fd_entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK) != 0;
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
                            if let Some(e) = crate::syscall::check_signals_for_restartable_wait() {
                                // Not preemptible from here: a kill must never find this thread
                                // switched out while it holds the lock its waiter list is under.
                                crate::per_cpu::preempt_disable();
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
/// Timer dispatch uses non-blocking process-manager acquisition and re-arms
/// need_resched when the lock is held. Fault handlers also use try_manager,
/// except the user page-fault and GPF termination arms. Those blocking arms
/// cannot be reached by a kernel-mode fork on the same CPU; on another CPU
/// the runnable fork holder can release the lock. See interrupts.rs and
/// interrupts/context_switch.rs for the acquisition sites.
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
    // Declared before PM guards: removed FD tables are destroyed after PM unlock.
    let mut retired_rows = alloc::vec::Vec::new();
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
        Ok(pt) => crate::memory::process_memory::UnpublishedPageTable::new(pt, parent_pid.as_u64()),
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
            if !crate::process::limits::fork_allowed(manager, parent_pid) {
                return SyscallResult::Err(super::errno::EAGAIN as u64);
            }
            manager.fork_process_with_parent_context(
                parent_pid,
                parent_context,
                child_page_table.publish(),
            )
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
                        (
                            thread_id,
                            tls_block,
                            Box::new(thread.publish_to_scheduler()),
                        )
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
                    retired_rows.push(manager.remove_process(child_pid));
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

            log::debug!(
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
    #[cfg(feature = "testing")]
    let mut closes = crate::ipc::fd::DeferredFdCloses::default();
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
                match manager.exec_process(current_pid, elf_data, exec_program_name, &mut closes) {
                    Ok(new_entry_point) => {
                        log::info!(
                            "sys_exec: Successfully replaced process address space, entry point: {:#x}",
                            new_entry_point
                        );
                        crate::tls::install_exec_fs_base(current_thread_id);

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

                        // The new image starts from the initial x87/SSE state, not the old one's.
                        crate::arch_impl::x86_64::fpu::reset_current();

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
/// The program image at `path` and the identity its set-ID bits confer.
fn load_elf_from_ext2(
    path: &str,
) -> Result<(Vec<u8>, crate::process::credentials::ExecIdentity), i32> {
    use super::errno::{EACCES, EIO};

    // Taken before the filesystem lock: who may execute the file.
    let cred = crate::fs::permissions::Credentials::current(false);

    // The handle holds the inode until its content is read.
    let (mount, inode_num, _held) =
        crate::fs::namei::resolve_file(path).map_err(|errno| errno as i32)?;
    let fs_guard = mount.read();
    let fs = fs_guard.as_ref().ok_or(EIO)?;

    let inode = fs.read_inode(inode_num).map_err(|_| EIO)?;

    if inode.is_dir() {
        return Err(EACCES);
    }

    if !cred.permits(&inode, 1) {
        return Err(EACCES);
    }

    let data = fs
        .read_file_content_coherent_unless(inode_num, &inode, super::exec::caller_killed)
        .map_err(|_| EIO)?
        .ok_or(super::errno::EINTR)?;
    Ok((data, crate::process::credentials::ExecIdentity::of(&inode)))
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
    envp_ptr: u64,
) -> SyscallResult {
    let mut closes = crate::ipc::fd::DeferredFdCloses::default();
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

    // The whole pathname, up to PATH_MAX, reaches the resolver, which
    // reports ENAMETOOLONG for one that is too long.
    let program_name = match super::userptr::copy_cstr_from_user(program_name_ptr) {
        Ok(name) => name,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let program_name = program_name.as_str();

    log::info!("sys_execv: Loading program '{}'", program_name);

    let mut arguments = match super::exec::Arguments::copy_from_user(program_name, argv_ptr, envp_ptr) {
        Ok(arguments) => arguments,
        Err(errno) => return SyscallResult::Err(errno),
    };

    #[cfg(feature = "testing")]
    {
        // Load ELF binary WITH interrupts enabled - ext2 I/O needs timer interrupts
        // for proper VirtIO operation
        let (elf_vec, image_identity) = match super::exec::read_image(program_name, &mut arguments, |path| {
            if path.contains('/') {
                load_elf_from_ext2(path)
            } else {
                let bin_path = alloc::format!("/bin/{}", path);
                match load_elf_from_ext2(&bin_path) {
                    Ok(data) => Ok(data),
                    Err(errno) if errno == super::errno::ENOENT => {
                        crate::userspace_test::load_test_binary_from_disk(path)
                            .map(|data| (data, Default::default()))
                            .map_err(|_| errno)
                    }
                    Err(errno) => Err(errno),
                }
            }
        }) {
            Ok(data) => data,
            Err(errno) => return SyscallResult::Err(errno),
        };
        let elf_data = elf_vec.as_slice();

        // Find current process
        // The new image's credentials are decided before its stack is built,
        // so the auxiliary vector reports them; the commit refuses if they change.
        let (current_pid, image_cred) = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((pid, _)) = manager.find_process_by_thread(current_thread_id) {
                    match manager.credentials_after_exec(pid, image_identity) {
                        Some(cred) => (pid, cred),
                        None => return SyscallResult::Err(3), // ESRCH
                    }
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

        // Borrow the prepared arguments and environment for the stack builder
        let (argv_slices, envp_slices) = match arguments.slices() {
            Ok(slices) => slices,
            Err(errno) => return SyscallResult::Err(errno),
        };
        let mut prepared = match crate::process::manager::ProcessManager::prepare_exec_image(
            current_pid,
            elf_data,
            Some(program_name),
            &argv_slices,
            &envp_slices,
            image_identity,
            image_cred,
        ) {
            Ok(image) => Some(image),
            Err(error) => return SyscallResult::Err(super::exec::manager_errno(error)),
        };

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
                &mut prepared,
                &mut closes,
            ) {
                Ok(value) => value,
                Err("exec blocked while CLONE_VM sibling shares old address space") => {
                    return SyscallResult::Err(11); // EAGAIN
                }
                Err(e) => {
                    log::error!("sys_execv: Failed to exec process: {}", e);
                    return SyscallResult::Err(super::exec::manager_errno(e));
                }
            };

            let new_cr3 = commit.new_page_table_root();

            // #721 K3/X1: read everything needed off the receipt above, then release PM
            // before taking the SCHEDULER lock inside commit.apply() — never read the
            // process manager again after this drop.
            drop(manager_guard);

            commit.apply();
            crate::tls::install_exec_fs_base(current_thread_id);

            log::info!(
                "sys_execv: Successfully replaced process address space, entry={:#x}, rsp={:#x}",
                new_entry_point,
                new_rsp
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

            // The new image starts from the initial x87/SSE state, not the old one's.
            crate::arch_impl::x86_64::fpu::reset_current();

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
        let (elf_vec, image_identity) = match super::exec::read_image(program_name, &mut arguments, |path| {
            if path.contains('/') {
                crate::boot::init_image::read_program_image(path)
            } else {
                crate::boot::init_image::read_program_image(&alloc::format!("/bin/{}", path))
            }
        }) {
            Ok(data) => data,
            Err(errno) => return SyscallResult::Err(errno),
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
        // The new image's credentials are decided before its stack is built,
        // so the auxiliary vector reports them; the commit refuses if they change.
        let (current_pid, image_cred) = {
            let manager_guard = crate::process::manager();
            if let Some(ref manager) = *manager_guard {
                if let Some((pid, _)) = manager.find_process_by_thread(current_thread_id) {
                    match manager.credentials_after_exec(pid, image_identity) {
                        Some(cred) => (pid, cred),
                        None => return SyscallResult::Err(3), // ESRCH
                    }
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

        let (argv_slices, envp_slices) = match arguments.slices() {
            Ok(slices) => slices,
            Err(errno) => return SyscallResult::Err(errno),
        };
        let mut prepared = match crate::process::manager::ProcessManager::prepare_exec_image(
            current_pid,
            elf_data,
            Some(program_name),
            &argv_slices,
            &envp_slices,
            image_identity,
            image_cred,
        ) {
            Ok(image) => Some(image),
            Err(error) => return SyscallResult::Err(super::exec::manager_errno(error)),
        };

        // Mask only publication, scheduler commit, CR3 install and frame patch.
        // The replacement image and arguments were built above without PM held.
        crate::arch_without_interrupts(|| {
            let mut manager_guard = crate::process::manager();
            let Some(manager) = manager_guard.as_mut() else {
                log::error!("sys_execv: Process manager not available");
                return SyscallResult::Err(12); // ENOMEM
            };

            let (new_entry_point, new_rsp, commit) = match manager.exec_process_with_argv(
                current_pid,
                &mut prepared,
                &mut closes,
            ) {
                Ok(value) => value,
                Err("exec blocked while CLONE_VM sibling shares old address space") => {
                    return SyscallResult::Err(11); // EAGAIN
                }
                Err(e) => {
                    log::error!("sys_execv: Failed to exec process: {}", e);
                    return SyscallResult::Err(super::exec::manager_errno(e));
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
            crate::tls::install_exec_fs_base(current_thread_id);

            log::info!(
                "sys_execv: Successfully replaced process address space, entry={:#x}, rsp={:#x}",
                new_entry_point,
                new_rsp
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

            // The new image starts from the initial x87/SSE state, not the old one's.
            crate::arch_impl::x86_64::fpu::reset_current();

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
    // Declared before PM guards: removed FD tables are destroyed after PM unlock.
    let mut retired_rows = alloc::vec::Vec::new();
    use super::errno::{EFAULT, ENOMEM, ESRCH};

    if path_ptr == 0 {
        return SyscallResult::Err(EFAULT as u64);
    }

    // Read the whole pathname, up to PATH_MAX, as sys_execv_with_frame does.
    let program_path = match super::userptr::copy_cstr_from_user(path_ptr) {
        Ok(path) => path,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let program_path = program_path.as_str();

    // Spawn retains its separate 64-argument, 4096-byte per-string budget.
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

    let elf_vec = match crate::boot::init_image::read_program(&resolved_path) {
        Ok(data) => data,
        Err(errno) => return SyscallResult::Err(errno as u64),
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
        Err("Process limit exceeded") => return SyscallResult::Err(super::errno::EAGAIN as u64),
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
                    retired_rows.push(manager.remove_process(child_pid));
                }
                thread
            }
            None => None,
        }
    };
    // #813: PM scope ended; release removed rows before further observations.
    retired_rows.clear();

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
    #[cfg(feature = "testing")]
    let mut closes = crate::ipc::fd::DeferredFdCloses::default();
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
                match manager.exec_process(current_pid, _elf_data, _exec_program_name, &mut closes)
                {
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
                    // Return the process ID. Debug level: this runs under the
                    // process manager, which every other CPU waits for while a
                    // serial line is written.
                    log::debug!(
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
    // The descriptors are 32-bit (`unsigned int`) in the syscall ABI; the
    // register's upper half is not part of the argument.
    dup_to(old_fd as u32, new_fd as u32, false)
}

/// sys_dup3 - dup2 with flags
///
/// dup3(old_fd, new_fd, flags) is dup2 except that `flags` may hold only
/// O_CLOEXEC, which sets FD_CLOEXEC on new_fd, and old_fd == new_fd is EINVAL
/// rather than a no-op.
///
/// Returns: new_fd on success, negative error code on failure
pub fn sys_dup3(old_fd: u64, new_fd: u64, flags: u64) -> SyscallResult {
    use crate::ipc::fd::status_flags::O_CLOEXEC;

    // The ABI types are `unsigned int` descriptors and an `int` flags word.
    // Take the 32-bit values before any check, so an upper register half can
    // neither hide old_fd == new_fd nor count as an unknown flag.
    let (old_fd, new_fd, flags) = (old_fd as u32, new_fd as u32, flags as u32);
    if flags & !O_CLOEXEC != 0 || old_fd == new_fd {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    dup_to(old_fd, new_fd, flags & O_CLOEXEC != 0)
}

/// Shared body of dup2 and dup3: duplicate old_fd onto new_fd, closing what
/// new_fd held, with FD_CLOEXEC on new_fd set only when `set_cloexec`.
fn dup_to(old_fd: u32, new_fd: u32, set_cloexec: bool) -> SyscallResult {
    log::debug!(
        "sys_dup2: old_fd={}, new_fd={}, cloexec={}",
        old_fd,
        new_fd,
        set_cloexec
    );

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
    let duplicated = process
        .fd_table
        .dup2(old_fd as i32, new_fd as i32, set_cloexec);
    let lock_owner = process.lock_owner.id();
    drop(manager_guard);
    match duplicated {
        Ok((fd, overwritten)) => {
            if let Some(entry) = overwritten {
                // dup2 closes new_fd first, and that close drops record locks.
                crate::fs::locks::release_closed(lock_owner, &entry.kind);
                crate::task::process_task::close_extracted_fds(alloc::vec![(
                    new_fd as usize,
                    entry
                )]);
            }
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
/// - F_GETLK, F_SETLK, F_SETLKW: POSIX advisory record locks
pub fn sys_fcntl(fd: u64, cmd: u64, arg: u64) -> SyscallResult {
    use crate::ipc::fd::fcntl_cmd::*;

    let fd = fd as i32;
    let cmd = cmd as i32;
    // The record-lock commands take a `struct flock *`, so they see the whole
    // argument; every other command takes an int.
    let flock_ptr = arg;
    let arg = arg as i32;

    log::debug!("sys_fcntl: fd={}, cmd={}, arg={}", fd, cmd, arg);

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => {
            log::error!("sys_fcntl: No current thread!");
            return SyscallResult::Err(9); // EBADF
        }
    };

    if matches!(cmd, F_GETLK | F_SETLK | F_SETLKW) {
        return fcntl_record_lock(thread_id, fd, cmd, flock_ptr);
    }

    // #796: this used to be `try_manager()` with an `EAGAIN` arm, which made a
    // momentarily contended process-manager lock look to userspace like an fcntl
    // error. POSIX permits `[EAGAIN]` from `fcntl()` only in the `F_SETLK`
    // record-locking arm, which returns above, so no command reachable through
    // the dispatch below has a legal `EAGAIN`; the observed
    // failure was `fcntl(F_SETFD)` reporting it before `set_fd_flags` -- whose
    // only error is `EBADF` -- was ever reached.
    //
    // Blocking here is the discipline this file already follows: `sys_dup` and
    // `sys_dup2` directly above take `crate::process::manager()` for the same
    // `process.fd_table` mutation from the same context. The safety argument,
    // re-derived at these bytes rather than cited:
    //
    //   * This function is reachable only from the syscall dispatcher, i.e. from
    //     a trap taken at EL0/ring 3. That is the same context class in which
    //     `arch_impl/aarch64/exception.rs:2331` takes the blocking `manager()`
    //     for a CoW abort, with the same justification: a CPU that was running
    //     userspace when it trapped cannot already own PROCESS_MANAGER.
    //   * An asynchronous bottom half CAN block on this lock, so the guard is
    //     not "no asynchronous waiter for PM exists": `net/udp.rs`'s
    //     `deliver_to_socket` takes the blocking `with_process_manager()`, and
    //     it is reached from `net/mod.rs`'s `net_rx_softirq_handler`, which
    //     `task::softirqd::do_softirq()` runs at IRQ exit on both architectures
    //     (`arch_impl/aarch64/exception.rs`'s `handle_irq`, `per_cpu.rs`'s
    //     `irq_exit`). What makes waiting here safe is narrower: no bottom half
    //     can run on THIS CPU while this frame awaits or owns the lock.
    //     - aarch64: `manager()` executes `msr daifset, #0xf` before it takes
    //       the mutex and restores DAIF only in `Drop` (`process/mod.rs`), so no
    //       IRQ -- hence no softirq -- is taken across the wait or the hold.
    //     - x86_64: `manager()` masks no interrupt state, but `rust_syscall_handler`
    //       brackets the whole syscall in `preempt_disable`/`preempt_enable`
    //       (`syscall/handler.rs`), and `per_cpu::irq_exit()` runs
    //       `do_softirq()` only when `preempt_count() == 0`.
    //     A peer CPU's softirq that blocks on PM waits this window out; the wait
    //     is one-directional, so it cannot close a cycle: 0 of the 2 operations
    //     inside this window (the process lookup and the `fd_table` mutation)
    //     waits on the network stack.
    //     claim-lint:ok: the window's 2 operations are the
    //     `find_process_by_thread_mut` lookup and the `match cmd` arms directly
    //     below this comment; neither reaches the network stack.
    //   * A holder is not preempted away from under the waiter, but the two
    //     architectures reach that by different mechanisms -- they are not the
    //     same refusal. aarch64's `manager()` masks DAIF for the guard's
    //     lifetime, so its holder cannot be preempted at all. x86's timer
    //     dispatch instead refuses to switch when it cannot get PM, "leaving the
    //     lock-holding context -- the only one that can release it -- running"
    //     (`interrupts/context_switch.rs`); that comment's "both entry paths"
    //     are the two x86 dispatch entry paths, not an x86/aarch64 pair. The
    //     aarch64 dispatch's `TtbrResult::PmLockBusy` arm does something else:
    //     it redirects this CPU to idle and requeues the INCOMING thread, which
    //     leaves no lock-holding context running. The DAIF argument covers
    //     `manager()` holders only; a `try_manager()` holder masks no interrupt
    //     state and is outside it -- filed as #812, not repaired here.
    //     claim-lint:ok: #812 carries the reachability chain and its citations.
    //   * The window is not free, and the cost is named rather than left
    //     implicit. On aarch64 it is DAIF-masked across both the wait and the
    //     critical section below, including this function's per-arm
    //     `log::debug!` calls, which do reach COM2 (this build sets no
    //     `release_max_level_*` feature and `logger.rs:977-978` admits each of
    //     the 5 levels below `Trace`). The wait's worst case is the longest PM hold in the
    //     tree -- `ProcessManager::exec_process_with_argv` holds PM across
    //     `load_elf_into_page_table` -- not a short one. `sys_dup`/`sys_dup2`
    //     directly above already pay exactly this cost, so it is not new here.
    //     claim-lint:ok: 2 of 2 precedents are `handlers.rs:3774` and `:3821`;
    //     the cost is itemised in the #796 doc's STEP 2 point 6.
    //   * The window opened here takes no scheduler lock and publishes no
    //     thread, so it cannot trip the PM->SCHEDULER order marker
    //     (`SCHED_AFTER_PM_VIOLATIONS`).
    //
    // Census and per-site reasoning:
    // docs/planning/green-program/syscalls/796-FCNTL-EAGAIN-2026-09-05.md
    let mut manager_guard = crate::process::manager();

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

/// `struct flock`, identical on x86_64 and aarch64.
#[repr(C)]
#[derive(Clone, Copy)]
struct Flock {
    l_type: i16,
    l_whence: i16,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
}

/// fcntl `F_GETLK`, `F_SETLK` and `F_SETLKW` on a regular file.
///
/// The descriptor's file, lock owner (the thread group's) and access mode are
/// read under PM; the lock table is used only after PM is released, because
/// `F_SETLKW` sleeps. The `struct flock` is copied through the fault-tolerant
/// user-copy routine, so a bad pointer is EFAULT.
fn fcntl_record_lock(thread_id: u64, fd: i32, cmd: i32, flock_ptr: u64) -> SyscallResult {
    use super::errno::{EBADF, EINVAL, EIO, EOVERFLOW};
    use super::fs::{O_RDONLY, O_WRONLY, SEEK_CUR, SEEK_END, SEEK_SET};
    use super::userptr::{copy_from_user, copy_to_user};
    use crate::fs::locks::{self, FileKey, LockKind, F_RDLCK, F_UNLCK, F_WRLCK, OFFSET_MAX};
    use crate::ipc::fd::fcntl_cmd::{F_GETLK, F_SETLKW};
    use crate::ipc::FdKind;

    let (owner, key, access, position, handle) = {
        let manager_guard = crate::process::manager();
        let Some((_pid, process)) = manager_guard
            .as_ref()
            .and_then(|m| m.find_process_by_thread(thread_id))
        else {
            return SyscallResult::Err(EBADF as u64);
        };
        let Some(entry) = process.fd_table.get(fd) else {
            return SyscallResult::Err(EBADF as u64);
        };
        match &entry.kind {
            FdKind::RegularFile(file) => {
                let file = file.lock();
                (
                    process.lock_owner.id(),
                    FileKey {
                        mount_id: file.mount_id,
                        inode: file.inode_num,
                    },
                    entry.status_flags() & crate::ipc::fd::status_flags::O_ACCMODE,
                    file.position,
                    file.handle.clone(),
                )
            }
            // A descriptor for anything but a regular file does not support locking.
            _ => return SyscallResult::Err(EINVAL as u64),
        }
    };

    let user = flock_ptr as *mut Flock;
    let mut flock = match copy_from_user(user as *const Flock) {
        Ok(flock) => flock,
        Err(e) => return SyscallResult::Err(e),
    };

    let kind = match flock.l_type {
        F_RDLCK => Some(LockKind::Read),
        F_WRLCK => Some(LockKind::Write),
        F_UNLCK if cmd != F_GETLK => None,
        _ => return SyscallResult::Err(EINVAL as u64),
    };

    let base = match flock.l_whence as i32 {
        SEEK_SET => 0,
        SEEK_CUR => match i64::try_from(position) {
            Ok(position) => position,
            Err(_) => return SyscallResult::Err(EOVERFLOW as u64),
        },
        SEEK_END => match super::fs::get_ext2_file_size_for_handle(&handle) {
            Some(size) => size as i64,
            None => return SyscallResult::Err(EIO as u64),
        },
        _ => return SyscallResult::Err(EINVAL as u64),
    };
    let range = match flock_range(base, flock.l_start, flock.l_len) {
        Ok(range) => range,
        Err(e) => return SyscallResult::Err(e as u64),
    };

    if cmd == F_GETLK {
        let kind = kind.expect("F_GETLK rejected F_UNLCK above");
        match locks::get(key, owner, kind, range) {
            Some(conflict) => {
                flock.l_type = conflict.kind.l_type();
                flock.l_whence = SEEK_SET as i16;
                flock.l_start = conflict.range.start;
                flock.l_len = if conflict.range.end == OFFSET_MAX {
                    0
                } else {
                    conflict.range.end - conflict.range.start + 1
                };
                flock.l_pid = conflict.owner as i32;
            }
            None => flock.l_type = F_UNLCK,
        }
        return match copy_to_user(user, &flock) {
            Ok(()) => SyscallResult::Ok(0),
            Err(e) => SyscallResult::Err(e),
        };
    }

    // A read lock needs a descriptor open for reading, a write lock one open
    // for writing.
    let permitted = match kind {
        Some(LockKind::Read) => access != O_WRONLY,
        Some(LockKind::Write) => access != O_RDONLY,
        None => true,
    };
    if !permitted {
        return SyscallResult::Err(EBADF as u64);
    }

    match locks::set(key, owner, kind, range, cmd == F_SETLKW) {
        Ok(()) => SyscallResult::Ok(0),
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// The inclusive byte range a `struct flock` names, from the offset its
/// `l_whence` selects. `l_len` of 0 runs to the largest offset; a negative
/// `l_len` names the bytes before `l_start`.
fn flock_range(base: i64, l_start: i64, l_len: i64) -> Result<crate::fs::locks::Range, i32> {
    use super::errno::{EINVAL, EOVERFLOW};
    use crate::fs::locks::{Range, OFFSET_MAX};

    let start = base.checked_add(l_start).ok_or(EOVERFLOW)?;
    if start < 0 {
        return Err(EINVAL);
    }
    if l_len > 0 {
        let end = start.checked_add(l_len - 1).ok_or(EOVERFLOW)?;
        Ok(Range { start, end })
    } else if l_len == 0 {
        Ok(Range {
            start,
            end: OFFSET_MAX,
        })
    } else {
        match start.checked_add(l_len) {
            Some(first) if first >= 0 => Ok(Range {
                start: first,
                end: start - 1,
            }),
            _ => Err(EINVAL),
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
    super::multiplex::poll(fds_ptr, nfds, timeout)
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
pub(super) fn poll_report_timeout(
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

/// Restore TTBR0 after blocking in poll. Same pattern as nanosleep/waitpid.
#[cfg(target_arch = "aarch64")]
pub(super) fn poll_ensure_address_space() {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if let Some(ref page_table) = process.page_table {
                let ttbr0_value = page_table.level_4_frame().start_address().as_u64();
                crate::arch_impl::aarch64::ttbr0::restore_process_ttbr0(ttbr0_value);
            }
        }
    }
}

pub fn sys_ppoll(fds_ptr: u64, nfds: u64, timeout: u64, mask: u64, size: u64) -> SyscallResult {
    super::multiplex::ppoll(fds_ptr, nfds, timeout, mask, size)
}

pub fn sys_select(nfds: i32, read: u64, write: u64, except: u64, timeout: u64) -> SyscallResult {
    super::multiplex::select(nfds, read, write, except, timeout)
}

pub fn sys_pselect6(nfds: i32, read: u64, write: u64, except: u64, timeout: u64, mask: u64) -> SyscallResult {
    super::multiplex::pselect6(nfds, read, write, except, timeout, mask)
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
    {
        // Mark the calling process as the display owner. Ownership moves: a
        // previous owner's whole-screen mapping can no longer draw.
        use crate::syscall::memory_common::get_current_thread_id;
        if let Some(tid) = get_current_thread_id() {
            let mut mgr_guard = crate::process::manager();
            if let Some(ref mut mgr) = *mgr_guard {
                if let Some((pid, process)) = mgr.find_process_by_thread_mut(tid) {
                    process.has_display_ownership = true;
                    crate::syscall::graphics::DISPLAY_OWNER_PID
                        .store(pid.as_u64(), core::sync::atomic::Ordering::Release);
                }
            }
        }

        // The kernel stops drawing its boot screen but leaves it up until the new
        // owner draws, so an owner that fails first does not blank the screen.
        #[cfg(target_arch = "aarch64")]
        let _ = crate::graphics::boot_screen::stop_drawing();

        // Tell the render thread to stop flushing the framebuffer.
        // BWM will handle all GPU operations via its own fb_flush() syscall.
        #[cfg(any(feature = "interactive", target_arch = "aarch64"))]
        crate::graphics::render_task::set_display_taken();

        // The production x86_64 kernel has no render thread: its log sink stops
        // drawing log records on the framebuffer instead.
        #[cfg(all(target_arch = "x86_64", not(feature = "interactive")))]
        crate::logger::take_display();
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

/// getrlimit and setrlimit use the same atomic per-process operation as prlimit64.
pub fn sys_getrlimit(resource: u64, rlim_ptr: u64) -> SyscallResult {
    if rlim_ptr == 0 {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }
    sys_prlimit64(0, resource, 0, rlim_ptr)
}

pub fn sys_setrlimit(resource: u64, rlim_ptr: u64) -> SyscallResult {
    if rlim_ptr == 0 {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }
    sys_prlimit64(0, resource, rlim_ptr, 0)
}

pub fn sys_prlimit64(pid: u64, resource: u64, new_ptr: u64, old_ptr: u64) -> SyscallResult {
    use super::errno::{EFAULT, EINVAL, EPERM, ESRCH};
    use crate::process::limits::{Rlimit, COUNT, NOFILE};
    if resource >= COUNT as u64 {
        return SyscallResult::Err(EINVAL as u64);
    }
    let new = if new_ptr == 0 {
        None
    } else {
        match super::userptr::copy_from_user(new_ptr as *const Rlimit) {
            Ok(value) => Some(value),
            Err(_) => return SyscallResult::Err(EFAULT as u64),
        }
    };
    if new.is_some_and(|v| v.soft > v.hard) {
        return SyscallResult::Err(EINVAL as u64);
    }
    if new.is_some_and(|v| resource as usize == NOFILE && v.hard > crate::ipc::MAX_FDS as u64) {
        return SyscallResult::Err(EPERM as u64);
    }
    let Some(thread) = super::memory_common::get_current_thread_id() else {
        return SyscallResult::Err(ESRCH as u64);
    };
    // User copies may fault and acquire PM; never copy while holding it.
    let old = {
        let mut guard = crate::process::manager();
        let Some(manager) = guard.as_mut() else {
            return SyscallResult::Err(ESRCH as u64);
        };
        let Some((caller_pid, caller)) = manager.find_process_by_thread(thread) else {
            return SyscallResult::Err(ESRCH as u64);
        };
        let (euid, uid, gid) = (caller.cred.euid, caller.cred.uid, caller.cred.gid);
        let target = if pid == 0 {
            caller_pid
        } else {
            crate::process::ProcessId::new(pid)
        };
        let Some(process) = manager.get_process_mut(target) else {
            return SyscallResult::Err(ESRCH as u64);
        };
        if target != caller_pid
            && euid != 0
            && (uid != process.cred.uid
                || uid != process.cred.euid
                || uid != process.cred.suid
                || gid != process.cred.gid
                || gid != process.cred.egid
                || gid != process.cred.sgid)
        {
            return SyscallResult::Err(EPERM as u64);
        }
        let old = process.limits.get(resource as usize);
        if let Some(new) = new {
            if new.hard > old.hard && euid != 0 {
                return SyscallResult::Err(EPERM as u64);
            }
            process.limits.set(resource as usize, new);
            if resource as usize == NOFILE {
                let limits = process.limits.clone();
                manager.set_group_fd_limit(&limits, new.soft);
            }
        }
        old
    };
    if old_ptr != 0 && super::userptr::copy_to_user(old_ptr as *mut Rlimit, &old).is_err() {
        return SyscallResult::Err(EFAULT as u64);
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
                    return SyscallResult::Ok(process.cred.uid as u64);
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
                    return SyscallResult::Ok(process.cred.euid as u64);
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
                    return SyscallResult::Ok(process.cred.gid as u64);
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
                    return SyscallResult::Ok(process.cred.egid as u64);
                }
            }
        }
        SyscallResult::Ok(0)
    })
}

/// Apply `change` to the calling process's credentials under the process
/// manager lock; ESRCH when the caller has no process.
fn change_credentials(
    change: impl FnOnce(&mut crate::process::credentials::ProcessCredentials) -> Result<(), u64>,
) -> SyscallResult {
    crate::arch_without_interrupts(|| {
        let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
            return SyscallResult::Err(super::errno::ESRCH as u64);
        };
        let mut manager_guard = crate::process::manager();
        match manager_guard.as_mut().and_then(|m| m.find_process_by_thread_mut(thread_id)) {
            Some((_, process)) => match change(&mut process.cred) {
                Ok(()) => SyscallResult::Ok(0),
                Err(errno) => SyscallResult::Err(errno),
            },
            None => SyscallResult::Err(super::errno::ESRCH as u64),
        }
    })
}

/// setuid - set the user IDs (`ProcessCredentials::setuid`)
pub fn sys_setuid(uid: u32) -> SyscallResult {
    change_credentials(|cred| cred.setuid(uid))
}

/// setgid - set the group IDs (`ProcessCredentials::setgid`)
pub fn sys_setgid(gid: u32) -> SyscallResult {
    change_credentials(|cred| cred.setgid(gid))
}

/// setreuid - set the real and effective user IDs; -1 keeps one
pub fn sys_setreuid(real: u32, effective: u32) -> SyscallResult {
    change_credentials(|cred| cred.setreuid(real, effective))
}

/// setregid - set the real and effective group IDs; -1 keeps one
pub fn sys_setregid(real: u32, effective: u32) -> SyscallResult {
    change_credentials(|cred| cred.setregid(real, effective))
}

/// Report supplementary groups without holding PM across a faultable user copy.
pub fn sys_getgroups(size: i32, list: u64) -> SyscallResult {
    use super::errno::{EINVAL, ESRCH};
    if size < 0 { return SyscallResult::Err(EINVAL as u64); }
    let Some(tid) = crate::task::scheduler::current_thread_id() else { return SyscallResult::Err(ESRCH as u64); };
    let groups = {
        let guard = crate::process::manager();
        match guard.as_ref().and_then(|m| m.find_process_by_thread(tid)) {
            Some((_, process)) => process.cred.groups.clone(),
            None => return SyscallResult::Err(ESRCH as u64),
        }
    };
    if size == 0 { return SyscallResult::Ok(groups.len() as u64); }
    if (size as usize) < groups.len() { return SyscallResult::Err(EINVAL as u64); }
    match super::userptr::write_user_bytes(list, groups.as_ptr() as *const u8, groups.len() * core::mem::size_of::<u32>()) {
        Ok(()) => SyscallResult::Ok(groups.len() as u64),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// Replace supplementary groups atomically after copying the complete user list.
/// Only root may change the list, including clearing it with setgroups(0, NULL).
pub fn sys_setgroups(count: u64, list: u64) -> SyscallResult {
    use super::errno::{EINVAL, EPERM, ESRCH};
    if count > 65536 {
        return SyscallResult::Err(EINVAL as u64);
    }
    if crate::fs::permissions::Credentials::current(false).euid != 0 {
        return SyscallResult::Err(EPERM as u64);
    }
    let mut groups = alloc::vec![0u32; count as usize];
    if let Err(errno) = super::userptr::read_user_bytes(
        groups.as_mut_ptr() as *mut u8, list, groups.len() * core::mem::size_of::<u32>(),
    ) {
        return SyscallResult::Err(errno);
    }
    let groups = alloc::sync::Arc::new(groups);
    let Some(tid) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(ESRCH as u64);
    };
    let mut guard = crate::process::manager();
    let Some((_, process)) = guard.as_mut().and_then(|m| m.find_process_by_thread_mut(tid)) else {
        return SyscallResult::Err(ESRCH as u64);
    };
    if !process.cred.privileged() {
        return SyscallResult::Err(EPERM as u64);
    }
    process.cred.groups = groups;
    SyscallResult::Ok(0)
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
        SyscallResult::Err(super::errno::ESRCH as u64)
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
        if let Err(e) = validate_fd_for_degenerate_transfer(fd, false) {
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
    let fd_result: Result<crate::fs::ext2::live_inode::FileHandle, u64> = crate::arch_without_interrupts(|| {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                if let Some(fd_entry) = process.fd_table.get(fd) {
                    match &fd_entry.kind {
                        FdKind::RegularFile(_) if !fd_entry.readable() => {
                            return Err(super::errno::EBADF as u64);
                        }
                        FdKind::RegularFile(file_ref) => {
                            let file = file_ref.lock();
                            return Ok(file.handle.clone());
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

    let handle = match fd_result {
        Ok(handle) => handle,
        Err(e) => return SyscallResult::Err(e),
    };

    let inode_num = handle.object.key.inode;
    let mount_id = handle.object.mount.mount_id;
    let file_offset = offset as u64;

    // Read from ext2 at the given offset (no process lock held)
    use crate::fs::ext2;
    let mut update_atime = false;
    let mut read_fn = |fs: &ext2::Ext2Fs| -> SyscallResult {
        if handle.verify(fs).is_err() { return SyscallResult::Err(super::errno::EIO as u64); }
        let inode = match fs.read_inode(inode_num as u32) {
            Ok(i) => i,
            Err(_) => return SyscallResult::Err(super::errno::EIO as u64),
        };
        let file_size = inode.size();
        if file_offset >= file_size {
            return SyscallResult::Ok(0);
        }
        let to_read = core::cmp::min(count, file_size - file_offset) as usize;
        match fs.read_file_range_coherent(inode_num, &inode, file_offset, to_read) {
            Ok(data) => {
                let actual = core::cmp::min(data.len(), to_read);
                unsafe {
                    core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr as *mut u8, actual);
                }
                update_atime = inode.needs_atime_update();
                SyscallResult::Ok(actual as u64)
            }
            Err(_) => SyscallResult::Err(super::errno::EIO as u64),
        }
    };

    let is_home = ext2::home_mount_id().map_or(false, |id| id == mount_id);
    let result = if is_home {
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
    };
    if update_atime && matches!(result, SyscallResult::Ok(n) if n > 0) {
        let _ = super::fs::update_read_atime(inode_num as u32, mount_id);
    }
    result
}

/// pwrite64 - Write to file at given offset without changing file position
pub fn sys_pwrite64(fd: i32, buf_ptr: u64, count: u64, offset: i64) -> SyscallResult {
    use crate::ipc::FdKind;

    if buf_ptr == 0 || count == 0 {
        // Linux checks the descriptor before honouring a degenerate transfer
        // (#670): a zero-length operation on a bad descriptor is EBADF, not 0.
        if let Err(e) = validate_fd_for_degenerate_transfer(fd, true) {
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
    let fd_result: Result<(crate::fs::ext2::live_inode::FileHandle, bool), u64> = crate::arch_without_interrupts(|| {
        let manager_guard = crate::process::manager();
        if let Some(ref manager) = *manager_guard {
            if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
                if let Some(fd_entry) = process.fd_table.get(fd) {
                    match &fd_entry.kind {
                        FdKind::RegularFile(_) if !fd_entry.writable() => {
                            return Err(super::errno::EBADF as u64);
                        }
                        FdKind::RegularFile(file_ref) => {
                            let file = file_ref.lock();
                            return Ok((file.handle.clone(), !process.cred.privileged()));
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

    let (handle, unprivileged) = match fd_result {
        Ok(info) => info,
        Err(e) => return SyscallResult::Err(e),
    };

    let inode_num = handle.object.key.inode;
    let mount_id = handle.object.mount.mount_id;
    let file_offset = offset as u64;
    let count = match super::resource::write_length(
        file_offset,
        count as usize,
        super::resource::current_fsize(),
    ) {
        Ok(length) => length as u64,
        Err(errno) => {
            super::resource::signal_fsize();
            return SyscallResult::Err(errno);
        }
    };

    // Read user data (no process lock held)
    let data = match copy_from_user(buf_ptr, count as usize) {
        Ok(d) => d,
        Err(_) => return SyscallResult::Err(super::errno::EFAULT as u64),
    };

    use crate::fs::ext2;
    let write_fn = |fs: &mut ext2::Ext2Fs| -> SyscallResult {
        if handle.verify(fs).is_err() { return SyscallResult::Err(super::errno::EIO as u64); }
        match fs.write_file_range_as(inode_num as u32, file_offset, &data, unprivileged) {
            Ok(written) => SyscallResult::Ok(written as u64),
            Err(error) => SyscallResult::Err(crate::memory::file_map::mutation_errno(error)),
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
