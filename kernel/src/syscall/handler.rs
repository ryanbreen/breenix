// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║                         🚨 CRITICAL HOT PATH 🚨                               ║
// ║                                                                              ║
// ║  THIS FILE IS ON THE PROHIBITED MODIFICATIONS LIST.                          ║
// ║                                                                              ║
// ║  DO NOT ADD:                                                                 ║
// ║    - log::*, serial_println!, or ANY serial output                           ║
// ║    - Raw assembly that writes to port 0x3F8 (serial)                         ║
// ║    - Heap allocations (Box, Vec, String, format!)                            ║
// ║    - Locks that might contend (use try_lock with fallback only)              ║
// ║    - Page table walks or memory mapping operations                           ║
// ║    - Any code that takes more than ~100 cycles                               ║
// ║                                                                              ║
// ║  Timer interrupts fire every 1ms. Serial output takes 10,000+ cycles.        ║
// ║  Adding a single log statement here will cause:                              ║
// ║    - clock_gettime precision tests to fail (need sub-ms timing)              ║
// ║    - Userspace to never execute (timer fires before IRETQ completes)         ║
// ║    - Infinite kernel loops and stack overflows                               ║
// ║                                                                              ║
// ║  To debug syscalls, use GDB: See CLAUDE.md "GDB-Only Kernel Debugging"       ║
// ║                                                                              ║
// ║  If you believe you need to modify this file, you MUST:                      ║
// ║    1. Explain why GDB debugging is insufficient                              ║
// ║    2. Get explicit user approval                                             ║
// ║    3. Remove any added logging before committing                             ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

use super::{SyscallNumber, SyscallResult};
use core::sync::atomic::{AtomicBool, Ordering};
use x86_64::VirtAddr;

// Import tracing - these are inlined to ~5 instructions when disabled
use crate::tracing::providers::syscall::{trace_entry, trace_exit};

#[repr(C)]
#[derive(Debug)]
pub struct SyscallFrame {
    // General purpose registers (in memory order after all pushes)
    // Stack grows down, so last pushed is at lowest address (where RSP points)
    // Assembly pushes: rax first, then rcx, rdx, rbx, rbp, rsi, rdi, r8-r15
    // So r15 (pushed last) is at RSP+0, and rax (pushed first) is at RSP+112
    pub r15: u64, // pushed last, at RSP+0
    pub r14: u64, // at RSP+8
    pub r13: u64, // at RSP+16
    pub r12: u64, // at RSP+24
    pub r11: u64, // at RSP+32
    pub r10: u64, // at RSP+40
    pub r9: u64,  // at RSP+48
    pub r8: u64,  // at RSP+56
    pub rdi: u64, // at RSP+64
    pub rsi: u64, // at RSP+72
    pub rbp: u64, // at RSP+80
    pub rbx: u64, // at RSP+88
    pub rdx: u64, // at RSP+96
    pub rcx: u64, // at RSP+104
    pub rax: u64, // Syscall number - pushed first, at RSP+112

    // Interrupt frame (pushed by CPU before our code)
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl SyscallFrame {
    /// Check if this syscall came from userspace
    pub fn is_from_userspace(&self) -> bool {
        // Check CS register - if RPL (bits 0-1) is 3, it's from userspace
        (self.cs & 0x3) == 3
    }

    /// Get syscall number
    pub fn syscall_number(&self) -> u64 {
        self.rax
    }

    /// Get syscall arguments (following System V ABI)
    pub fn args(&self) -> (u64, u64, u64, u64, u64, u64) {
        (self.rdi, self.rsi, self.rdx, self.r10, self.r8, self.r9)
    }

    /// Set return value
    pub fn set_return_value(&mut self, value: u64) {
        self.rax = value;
    }
}

// Implement the HAL SyscallFrame trait
// This is compile-time trait glue with zero runtime overhead - all methods inline
impl crate::arch_impl::traits::SyscallFrame for SyscallFrame {
    #[inline(always)]
    fn syscall_number(&self) -> u64 {
        self.rax
    }

    #[inline(always)]
    fn arg1(&self) -> u64 {
        self.rdi
    }

    #[inline(always)]
    fn arg2(&self) -> u64 {
        self.rsi
    }

    #[inline(always)]
    fn arg3(&self) -> u64 {
        self.rdx
    }

    #[inline(always)]
    fn arg4(&self) -> u64 {
        self.r10
    }

    #[inline(always)]
    fn arg5(&self) -> u64 {
        self.r8
    }

    #[inline(always)]
    fn arg6(&self) -> u64 {
        self.r9
    }

    #[inline(always)]
    fn set_return_value(&mut self, value: u64) {
        self.rax = value;
    }

    #[inline(always)]
    fn return_value(&self) -> u64 {
        self.rax
    }
}

// Static flag to track first Ring 3 syscall
static RING3_CONFIRMED: AtomicBool = AtomicBool::new(false);

/// Returns true if userspace has started (first Ring 3 syscall received).
/// Used by scheduler to determine if idle thread should use idle_loop or
/// restore saved context from boot.
pub fn is_ring3_confirmed() -> bool {
    RING3_CONFIRMED.load(Ordering::Relaxed)
}

/// Raw serial string output - no locks, no allocations.
/// Used for boot markers where locking would deadlock.
#[inline(always)]
fn raw_serial_str_local(s: &str) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(0x3F8);
        for &byte in s.as_bytes() {
            port.write(byte);
        }
    }
}

/// Emit one-time marker when first syscall from Ring 3 (userspace) is received.
/// This is out-of-line to keep the hot path clean.
/// Also advances test framework to Userspace stage if boot_tests is enabled.
#[inline(never)]
#[cold]
fn emit_ring3_syscall_marker() {
    // Use raw serial output for the marker (no locks)
    raw_serial_str_local("RING3_SYSCALL: First syscall from userspace\n");
    raw_serial_str_local("[ OK ] syscall path verified\n");

    // Advance test framework to Userspace stage - we have confirmed Ring 3 execution
    // Note: We use advance_stage_marker_only() instead of advance_to_stage() because
    // we're in syscall context and cannot spawn kthreads or block on joins here.
    // The Userspace stage tests verify is_ring3_confirmed() which is already true.
    #[cfg(all(target_arch = "x86_64", feature = "boot_tests"))]
    {
        crate::test_framework::advance_stage_marker_only(
            crate::test_framework::TestStage::Userspace,
        );
    }
}

/// Main syscall handler called from assembly
///
/// CRITICAL: This is a hot path. NO logging, NO serial output, NO allocations.
/// See CLAUDE.md "Interrupt and Syscall Development - CRITICAL PATH REQUIREMENTS"
#[no_mangle]
pub extern "C" fn rust_syscall_handler(frame: &mut SyscallFrame) {
    // Increment preempt count FIRST (prevents scheduling during syscall)
    // CRITICAL: No logging before this point - timer interrupt + logger lock = deadlock
    crate::per_cpu::preempt_disable();

    // Verify this came from userspace (security check)
    if !frame.is_from_userspace() {
        // Don't log here - just return error
        frame.set_return_value(u64::MAX); // Error
        crate::per_cpu::preempt_enable();
        return;
    }

    // One-time marker for first syscall from Ring 3 (userspace confirmed)
    // This is called out-of-line only on the first syscall via swap
    if !RING3_CONFIRMED.swap(true, Ordering::Relaxed) {
        emit_ring3_syscall_marker();
    }

    let syscall_num = frame.syscall_number();
    let args = frame.args();

    // Trace syscall entry - compiles to ~5 instructions when tracing disabled
    // (2 atomic loads + branch, no function call on fast path)
    trace_entry(syscall_num);

    // A SIGKILL never takes this thread before the syscall's return path:
    // see `KillCustody::enter_syscall`.
    let custody = crate::task::thread::KillCustody::enter_syscall();

    // Dispatch to the appropriate syscall handler
    // NOTE: No logging here! This is the hot path.
    let result = match SyscallNumber::from_u64(syscall_num) {
        Some(SyscallNumber::Exit) => super::handlers::sys_exit(args.0 as i32),
        Some(SyscallNumber::Write) => super::handlers::sys_write(args.0, args.1, args.2),
        Some(SyscallNumber::Read) => super::handlers::sys_read(args.0, args.1, args.2),
        Some(SyscallNumber::Yield) => super::handlers::sys_yield(),
        Some(SyscallNumber::Fork) => super::handlers::sys_fork_with_frame(frame),
        Some(SyscallNumber::Mmap) => {
            let addr = args.0;
            let length = args.1;
            let prot = args.2 as u32;
            let flags = args.3 as u32;
            let fd = args.4 as i64;
            let offset = args.5;
            super::mmap::sys_mmap(addr, length, prot, flags, fd, offset)
        }
        Some(SyscallNumber::Mprotect) => {
            let addr = args.0;
            let length = args.1;
            let prot = args.2 as u32;
            super::mmap::sys_mprotect(addr, length, prot)
        }
        Some(SyscallNumber::Munmap) => {
            let addr = args.0;
            let length = args.1;
            super::mmap::sys_munmap(addr, length)
        }
        Some(SyscallNumber::Exec) => super::handlers::sys_execv_with_frame(frame, args.0, args.1, args.2),
        Some(SyscallNumber::GetPid) => super::handlers::sys_getpid(),
        Some(SyscallNumber::Getppid) => super::handlers::sys_getppid(),
        Some(SyscallNumber::GetTid) => super::handlers::sys_gettid(),
        Some(SyscallNumber::SetTidAddress) => super::handlers::sys_set_tid_address(args.0),
        Some(SyscallNumber::ExitGroup) => super::handlers::sys_exit_group(args.0 as i32),
        Some(SyscallNumber::ClockGetTime) => {
            // NOTE: No logging here! Serial I/O takes thousands of cycles
            // and would cause the sub-millisecond precision test to fail.
            let clock_id = args.0 as u32;
            let user_timespec_ptr = args.1 as *mut super::time::Timespec;
            super::time::sys_clock_gettime(clock_id, user_timespec_ptr)
        }
        Some(SyscallNumber::ClockSetTime) => {
            let clock_id = args.0 as u32;
            let user_timespec_ptr = args.1 as *const super::time::Timespec;
            super::time::sys_clock_settime(clock_id, user_timespec_ptr)
        }
        Some(SyscallNumber::Brk) => super::memory::sys_brk(args.0),
        Some(SyscallNumber::Kill) => super::signal::sys_kill(args.0 as i64, args.1 as i32),
        Some(SyscallNumber::Sigaction) => {
            super::signal::sys_sigaction(args.0 as i32, args.1, args.2, args.3)
        }
        Some(SyscallNumber::Sigprocmask) => {
            super::signal::sys_sigprocmask(args.0 as i32, args.1, args.2, args.3)
        }
        Some(SyscallNumber::Sigpending) => super::signal::sys_sigpending(args.0, args.1),
        Some(SyscallNumber::Sigsuspend) => {
            // sigsuspend(mask, sigsetsize) - atomically set mask and wait for signal
            // Needs frame access like pause() for saving userspace context
            super::signal::sys_sigsuspend_with_frame(args.0, args.1, frame)
        }
        Some(SyscallNumber::Sigaltstack) => super::signal::sys_sigaltstack(args.0, args.1),
        Some(SyscallNumber::Sigreturn) => {
            // CRITICAL: sigreturn restores ALL registers including RAX from the signal frame.
            // We must NOT overwrite RAX with the syscall return value after this call!
            // Return early to skip the set_return_value() call below.
            let result = super::signal::sys_sigreturn_with_frame(frame);
            if let SyscallResult::Err(errno) = result {
                // Only set return value on error - success case already has RAX set
                frame.set_return_value((-(errno as i64)) as u64);
            }
            // A signal that arrived during the call, a deferred SIGKILL
            // included, is delivered as on every other syscall return.
            drop(custody);
            check_and_deliver_signals_on_syscall_return(frame);
            // Perform cleanup that normally happens after result handling
            let kernel_stack_top = crate::per_cpu::kernel_stack_top();
            if kernel_stack_top != 0 {
                crate::gdt::set_tss_rsp0(VirtAddr::new(kernel_stack_top));
            }
            crate::irq_log::flush_local_try();
            crate::per_cpu::preempt_enable();
            return;
        }
        Some(SyscallNumber::Ioctl) => super::ioctl::sys_ioctl(args.0, args.1, args.2),
        Some(SyscallNumber::Socket) => super::socket::sys_socket(args.0, args.1, args.2),
        Some(SyscallNumber::Bind) => super::socket::sys_bind(args.0, args.1, args.2),
        Some(SyscallNumber::SendTo) => {
            super::socket::sys_sendto(args.0, args.1, args.2, args.3, args.4, args.5)
        }
        Some(SyscallNumber::RecvFrom) => {
            super::socket::sys_recvfrom(args.0, args.1, args.2, args.3, args.4, args.5)
        }
        Some(SyscallNumber::Connect) => super::socket::sys_connect(args.0, args.1, args.2),
        Some(SyscallNumber::Accept) => super::socket::sys_accept(args.0, args.1, args.2),
        Some(SyscallNumber::Listen) => super::socket::sys_listen(args.0, args.1),
        Some(SyscallNumber::Shutdown) => super::socket::sys_shutdown(args.0, args.1),
        Some(SyscallNumber::Getsockname) => super::socket::sys_getsockname(args.0, args.1, args.2),
        Some(SyscallNumber::Getpeername) => super::socket::sys_getpeername(args.0, args.1, args.2),
        Some(SyscallNumber::Socketpair) => {
            super::socket::sys_socketpair(args.0, args.1, args.2, args.3)
        }
        Some(SyscallNumber::Setsockopt) => {
            super::socket::sys_setsockopt(args.0, args.1, args.2, args.3, args.4)
        }
        Some(SyscallNumber::Getsockopt) => {
            super::socket::sys_getsockopt(args.0, args.1, args.2, args.3, args.4)
        }
        Some(SyscallNumber::Poll) => super::handlers::sys_poll(args.0, args.1, args.2 as i32),
        Some(SyscallNumber::Select) => {
            super::handlers::sys_select(args.0 as i32, args.1, args.2, args.3, args.4)
        }
        Some(SyscallNumber::Pipe) => super::pipe::sys_pipe(args.0),
        Some(SyscallNumber::Pipe2) => super::pipe::sys_pipe2(args.0, args.1),
        Some(SyscallNumber::Close) => super::pipe::sys_close(args.0 as i32),
        Some(SyscallNumber::Dup) => super::handlers::sys_dup(args.0),
        Some(SyscallNumber::Dup2) => super::handlers::sys_dup2(args.0, args.1),
        Some(SyscallNumber::Fcntl) => super::handlers::sys_fcntl(args.0, args.1, args.2),
        Some(SyscallNumber::Pause) => super::signal::sys_pause_with_frame(frame),
        Some(SyscallNumber::Nanosleep) => super::time::sys_nanosleep(args.0, args.1),
        Some(SyscallNumber::Getitimer) => super::signal::sys_getitimer(args.0 as i32, args.1),
        Some(SyscallNumber::Alarm) => super::signal::sys_alarm(args.0),
        Some(SyscallNumber::Setitimer) => {
            super::signal::sys_setitimer(args.0 as i32, args.1, args.2)
        }
        Some(SyscallNumber::Wait4) => {
            super::wait::sys_waitpid(args.0 as i64, args.1, args.2 as u32)
        }
        Some(SyscallNumber::Waitid) => {
            super::wait::sys_waitid(args.0 as u32, args.1, args.2, args.3 as u32)
        }
        Some(SyscallNumber::SetPgid) => super::session::sys_setpgid(args.0 as i32, args.1 as i32),
        Some(SyscallNumber::SetSid) => super::session::sys_setsid(),
        Some(SyscallNumber::GetPgid) => super::session::sys_getpgid(args.0 as i32),
        Some(SyscallNumber::GetSid) => super::session::sys_getsid(args.0 as i32),
        // Filesystem syscalls
        Some(SyscallNumber::Chmod) => super::metadata::sys_chmod(args.0, args.1 as u32),
        Some(SyscallNumber::Fchmod) => super::metadata::sys_fchmod(args.0 as i32, args.1 as u32),
        Some(SyscallNumber::Fchmodat) => super::metadata::sys_fchmodat(args.0 as i32, args.1, args.2 as u32),
        Some(SyscallNumber::Chown) => super::metadata::sys_chown(args.0, args.1 as u32, args.2 as u32),
        Some(SyscallNumber::Lchown) => super::metadata::sys_lchown(args.0, args.1 as u32, args.2 as u32),
        Some(SyscallNumber::Fchown) => super::metadata::sys_fchown(args.0 as i32, args.1 as u32, args.2 as u32),
        Some(SyscallNumber::Fchownat) => super::metadata::sys_fchownat(args.0 as i32, args.1, args.2 as u32, args.3 as u32, args.4 as u32),
        Some(SyscallNumber::Setgroups) => super::handlers::sys_setgroups(args.0, args.1),
        Some(SyscallNumber::Getgroups) => super::handlers::sys_getgroups(args.0 as i32, args.1),
        Some(SyscallNumber::Access) => super::fs::sys_access(args.0, args.1 as u32),
        Some(SyscallNumber::Getcwd) => super::fs::sys_getcwd(args.0, args.1),
        Some(SyscallNumber::Chdir) => super::fs::sys_chdir(args.0),
        Some(SyscallNumber::Fchdir) => super::fs::sys_fchdir(args.0),
        Some(SyscallNumber::Open) => super::fs::sys_open(args.0, args.1 as u32, args.2 as u32),
        Some(SyscallNumber::Lseek) => {
            super::fs::sys_lseek(args.0 as i32, args.1 as i64, args.2 as i32)
        }
        Some(SyscallNumber::Fstat) => super::fs::sys_fstat(args.0 as i32, args.1),
        Some(SyscallNumber::Sync) => super::fs::sys_sync(),
        Some(SyscallNumber::Statfs) => super::fs::sys_statfs(args.0, args.1),
        Some(SyscallNumber::Fstatfs) => super::fs::sys_fstatfs(args.0 as i32, args.1),
        Some(SyscallNumber::Fsync) => super::fs::sys_fsync(args.0 as i32),
        Some(SyscallNumber::Fdatasync) => super::fs::sys_fsync(args.0 as i32),
        Some(SyscallNumber::Truncate) => super::fs::sys_truncate(args.0, args.1 as i64),
        Some(SyscallNumber::Ftruncate) => {
            super::fs::sys_ftruncate(args.0 as i32, args.1 as i64)
        }
        Some(SyscallNumber::Getdents64) => super::fs::sys_getdents64(args.0 as i32, args.1, args.2),
        Some(SyscallNumber::Rename) => super::fs::sys_rename(args.0, args.1),
        Some(SyscallNumber::Mkdir) => super::fs::sys_mkdir(args.0, args.1 as u32),
        Some(SyscallNumber::Rmdir) => super::fs::sys_rmdir(args.0),
        Some(SyscallNumber::Link) => super::fs::sys_link(args.0, args.1),
        Some(SyscallNumber::Unlink) => super::fs::sys_unlink(args.0),
        Some(SyscallNumber::Symlink) => super::fs::sys_symlink(args.0, args.1),
        Some(SyscallNumber::Readlink) => super::fs::sys_readlink(args.0, args.1, args.2),
        Some(SyscallNumber::Mknod) => super::fifo::sys_mknod(args.0, args.1 as u32, args.2),
        // *at variants (ARM64 Linux has no legacy syscalls; x86_64 also supports these)
        Some(SyscallNumber::Openat) => {
            super::fs::sys_openat(args.0 as i32, args.1, args.2 as u32, args.3 as u32)
        }
        Some(SyscallNumber::Faccessat) => {
            super::fs::sys_faccessat(args.0 as i32, args.1, args.2 as u32, 0)
        }
        Some(SyscallNumber::Mkdirat) => {
            super::fs::sys_mkdirat(args.0 as i32, args.1, args.2 as u32)
        }
        Some(SyscallNumber::Mknodat) => {
            super::fs::sys_mknodat(args.0 as i32, args.1, args.2 as u32, args.3)
        }
        Some(SyscallNumber::Unlinkat) => {
            super::fs::sys_unlinkat(args.0 as i32, args.1, args.2 as i32)
        }
        Some(SyscallNumber::Symlinkat) => super::fs::sys_symlinkat(args.0, args.1 as i32, args.2),
        Some(SyscallNumber::Linkat) => {
            super::fs::sys_linkat(args.0 as i32, args.1, args.2 as i32, args.3, args.4 as i32)
        }
        Some(SyscallNumber::Renameat) => {
            super::fs::sys_renameat(args.0 as i32, args.1, args.2 as i32, args.3)
        }
        Some(SyscallNumber::Readlinkat) => {
            super::fs::sys_readlinkat(args.0 as i32, args.1, args.2, args.3)
        }
        Some(SyscallNumber::Dup3) => super::handlers::sys_dup3(args.0, args.1, args.2),
        Some(SyscallNumber::Pselect6) => {
            super::handlers::sys_pselect6(args.0 as i32, args.1, args.2, args.3, args.4, args.5)
        }
        Some(SyscallNumber::CowStats) => super::handlers::sys_cow_stats(args.0),
        Some(SyscallNumber::SimulateOom) => super::handlers::sys_simulate_oom(args.0),
        // PTY syscalls
        Some(SyscallNumber::PosixOpenpt) => super::pty::sys_posix_openpt(args.0),
        Some(SyscallNumber::Grantpt) => super::pty::sys_grantpt(args.0),
        Some(SyscallNumber::Unlockpt) => super::pty::sys_unlockpt(args.0),
        Some(SyscallNumber::Ptsname) => super::pty::sys_ptsname(args.0, args.1, args.2),
        Some(SyscallNumber::GetRandom) => {
            super::random::sys_getrandom(args.0, args.1, args.2 as u32)
        }
        Some(SyscallNumber::Clone) => {
            super::clone::sys_clone(args.0, args.1, args.2, args.3, args.4)
        }
        Some(SyscallNumber::Futex) => super::futex::sys_futex(
            args.0,
            args.1 as u32,
            args.2 as u32,
            args.3,
            args.4,
            args.5 as u32,
        ),
        // Vectored I/O
        Some(SyscallNumber::Readv) => super::iovec::sys_readv(args.0, args.1, args.2),
        Some(SyscallNumber::Writev) => super::iovec::sys_writev(args.0, args.1, args.2),
        // Stubs for musl libc compatibility
        Some(SyscallNumber::Mremap) => SyscallResult::Err(super::errno::ENOMEM as u64),
        Some(SyscallNumber::Madvise) => SyscallResult::Ok(0),
        Some(SyscallNumber::Ppoll) => {
            super::handlers::sys_ppoll(args.0, args.1, args.2, args.3, args.4)
        }
        Some(SyscallNumber::SetRobustList) => SyscallResult::Ok(0),
        // arch_prctl - x86_64 TLS setup
        Some(SyscallNumber::ArchPrctl) => {
            const ARCH_SET_FS: u64 = 0x1002;
            const ARCH_GET_FS: u64 = 0x1003;
            match args.0 {
                ARCH_SET_FS => crate::tls::set_user_fs_base(args.1),
                ARCH_GET_FS => {
                    let fs_base = x86_64::registers::model_specific::FsBase::read().as_u64();
                    match super::userptr::copy_to_user(args.1 as *mut u64, &fs_base) {
                        Ok(()) => SyscallResult::Ok(0),
                        Err(e) => SyscallResult::Err(e),
                    }
                }
                _ => SyscallResult::Err(super::errno::EINVAL as u64),
            }
        }
        // Filesystem: newfstatat
        Some(SyscallNumber::Newfstatat) => {
            super::fs::sys_newfstatat(args.0 as i32, args.1, args.2, args.3 as u32)
        }
        // GetTime is not mapped on x86_64 (use ClockGetTime instead)
        // It only exists in the enum for ARM64 compatibility
        Some(SyscallNumber::GetTime) => SyscallResult::Err(super::ErrorCode::NoSys as u64),
        // Graphics syscalls
        Some(SyscallNumber::FbInfo) => super::graphics::sys_fbinfo(args.0),
        Some(SyscallNumber::FbDraw) => super::graphics::sys_fbdraw(args.0),
        Some(SyscallNumber::FbMmap) => super::graphics::sys_fbmmap(args.0),
        Some(SyscallNumber::GetMousePos) => super::graphics::sys_get_mouse_pos(args.0),
        // Audio syscalls
        Some(SyscallNumber::AudioInit) => super::audio::sys_audio_init(),
        Some(SyscallNumber::AudioWrite) => super::audio::sys_audio_write(args.0, args.1),
        // Display takeover
        Some(SyscallNumber::TakeOverDisplay) => super::handlers::sys_take_over_display(),
        Some(SyscallNumber::GiveBackDisplay) => super::handlers::sys_give_back_display(),
        // Resource limits and system info
        Some(SyscallNumber::Getrlimit) => super::handlers::sys_getrlimit(args.0, args.1),
        Some(SyscallNumber::Prlimit64) => {
            super::handlers::sys_prlimit64(args.0, args.1, args.2, args.3)
        }
        Some(SyscallNumber::Uname) => super::handlers::sys_uname(args.0),
        // epoll
        Some(SyscallNumber::EpollCreate1) => super::epoll::sys_epoll_create1(args.0 as u32),
        Some(SyscallNumber::EpollCtl) => {
            super::epoll::sys_epoll_ctl(args.0 as i32, args.1 as i32, args.2 as i32, args.3)
        }
        Some(SyscallNumber::EpollWait) => {
            super::epoll::sys_epoll_pwait(args.0 as i32, args.1, args.2 as i32, args.3 as i32, 0, 0)
        }
        Some(SyscallNumber::EpollPwait) => super::epoll::sys_epoll_pwait(
            args.0 as i32,
            args.1,
            args.2 as i32,
            args.3 as i32,
            args.4,
            args.5,
        ),
        // Identity syscalls
        Some(SyscallNumber::Getuid) => super::handlers::sys_getuid(),
        Some(SyscallNumber::Geteuid) => super::handlers::sys_geteuid(),
        Some(SyscallNumber::Getgid) => super::handlers::sys_getgid(),
        Some(SyscallNumber::Getegid) => super::handlers::sys_getegid(),
        Some(SyscallNumber::Setuid) => super::handlers::sys_setuid(args.0 as u32),
        Some(SyscallNumber::Setgid) => super::handlers::sys_setgid(args.0 as u32),
        Some(SyscallNumber::Setreuid) => super::handlers::sys_setreuid(args.0 as u32, args.1 as u32),
        Some(SyscallNumber::Setregid) => super::handlers::sys_setregid(args.0 as u32, args.1 as u32),
        // Priorities and CPU usage
        Some(SyscallNumber::Getpriority) => super::priority::sys_getpriority(args.0, args.1),
        Some(SyscallNumber::Setpriority) => {
            super::priority::sys_setpriority(args.0, args.1, args.2)
        }
        Some(SyscallNumber::Getrusage) => super::rusage::sys_getrusage(args.0, args.1),
        Some(SyscallNumber::Times) => super::rusage::sys_times(args.0),
        // File creation mask
        Some(SyscallNumber::Umask) => super::handlers::sys_umask(args.0 as u32),
        // Timestamps
        Some(SyscallNumber::Utimensat) => {
            super::fs::sys_utimensat(args.0 as i32, args.1, args.2, args.3 as u32)
        }
        // Positional I/O
        Some(SyscallNumber::Pread64) => {
            super::handlers::sys_pread64(args.0 as i32, args.1, args.2, args.3 as i64)
        }
        Some(SyscallNumber::Pwrite64) => {
            super::handlers::sys_pwrite64(args.0 as i32, args.1, args.2, args.3 as i64)
        }
        Some(SyscallNumber::Spawn) => super::handlers::sys_spawn(args.0, args.1),
        // musl uses x86's legacy stat/lstat numbers, with the same Stat ABI.
        None if syscall_num == super::SETRLIMIT_SYSCALL_NUMBER => super::handlers::sys_setrlimit(args.0, args.1),
        None if syscall_num == 4 => super::fs::sys_newfstatat(-100, args.0, args.1, 0),
        None if syscall_num == 6 => super::fs::sys_newfstatat(-100, args.0, args.1, 0x100),
        None if syscall_num == super::MSYNC_SYSCALL_NUMBER => {
            super::mmap::sys_msync(args.0, args.1, args.2 as u32)
        }
        None => {
            log::warn!("Unknown syscall number: {} - returning ENOSYS", syscall_num);
            SyscallResult::Err(super::ErrorCode::NoSys as u64)
        }
    };
    drop(custody);

    // Set return value in RAX
    match result {
        SyscallResult::Ok(val) => {
            // Trace syscall exit with success value
            trace_exit(val as i64);
            frame.set_return_value(val);
        }
        SyscallResult::Err(errno) => {
            // Trace syscall exit with error (negative errno)
            trace_exit(-(errno as i64));
            // Return -errno in RAX for errors (Linux convention)
            frame.set_return_value((-(errno as i64)) as u64);
        }
    }

    // A wait a signal interrupted, to be resumed (SA_RESTART): return to the
    // 2-byte `int 0x80` with the syscall number back in RAX, so the syscall
    // runs again after any handler. The arguments are untouched.
    if matches!(result, SyscallResult::Err(e) if e == super::errno::ERESTARTSYS as u64) {
        frame.rax = syscall_num;
        frame.rip -= 2;
    }

    // CRITICAL: Check for pending signals before returning to userspace
    // This is required for POSIX compliance - signals must be delivered on syscall return.
    // Without this, a process that sends a signal to itself and then loops calling
    // yield() would never receive the signal (it would only get delivered on timer
    // interrupt, which might not fire for several milliseconds).
    check_and_deliver_signals_on_syscall_return(frame);

    // CRITICAL FIX: Update TSS.RSP0 before returning to userspace
    // When userspace triggers an interrupt (like int3), the CPU switches to kernel
    // mode and uses TSS.RSP0 as the kernel stack. This must be set correctly!
    let kernel_stack_top = crate::per_cpu::kernel_stack_top();
    if kernel_stack_top != 0 {
        crate::gdt::set_tss_rsp0(VirtAddr::new(kernel_stack_top));
    } else {
        log::error!("CRITICAL: Cannot set TSS.RSP0 - kernel_stack_top is 0!");
    }

    // Flush any pending IRQ logs before returning to userspace
    crate::irq_log::flush_local_try();

    // Decrement preempt count on syscall exit
    crate::per_cpu::preempt_enable();
}

// Assembly functions defined in entry.s
extern "C" {
    #[allow(dead_code)]
    pub fn syscall_entry();
    #[allow(dead_code)]
    pub fn syscall_return_to_userspace(user_rip: u64, user_rsp: u64, user_rflags: u64) -> !;
}

/// Trace function called before IRETQ to Ring 3
///
/// IMPORTANT: This function must be MINIMAL to avoid slowing down the iretq path.
/// Heavy diagnostics here cause the timer interrupt to fire before userspace
/// executes even a single instruction, creating an infinite loop.
///
/// The full page table verification code has been removed. If you need to debug
/// Ring 3 transition issues, temporarily re-enable diagnostics but be aware
/// this will prevent userspace from running.
#[no_mangle]
pub extern "C" fn trace_iretq_to_ring3(_frame_ptr: *const u64) {
    // Intentionally empty - diagnostics were causing timer to preempt before
    // userspace could execute. See commit history for the original diagnostic code.
}

/// Check for and deliver pending signals before returning from a syscall
///
/// This function is called on the syscall return path to check if the current
/// process has any deliverable signals. If so, it modifies the syscall frame
/// to jump to the signal handler instead of returning to the original code.
///
/// This is required for POSIX compliance - signals must be delivered on syscall
/// return, not just on interrupt return. Without this, a process that sends a
/// signal to itself and then busy-waits would never receive the signal until
/// a timer interrupt fires.
///
/// PERFORMANCE NOTE: This function uses try_manager() to avoid blocking if the
/// process manager lock is held. If the lock is unavailable, signals will be
/// delivered on the next timer interrupt instead.
fn check_and_deliver_signals_on_syscall_return(frame: &mut SyscallFrame) {
    // A stop holds the thread, outside PM, until SIGCONT; what is pending then
    // is checked again, in this loop rather than by recursion.
    while deliver_signals_on_syscall_return(frame) {
        crate::signal::delivery::hold_stopped_thread_on_syscall_return();
    }
}

/// One pass of `check_and_deliver_signals_on_syscall_return`: true when the
/// process is stopped, or has a stop to take, and nothing was delivered.
fn deliver_signals_on_syscall_return(frame: &mut SyscallFrame) -> bool {
    // Get current thread ID
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return false,
    };

    // Thread 0 is the idle thread - it doesn't have a process with signals
    if current_thread_id == 0 {
        return false;
    }

    // Try to acquire process manager lock (non-blocking)
    let mut manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => {
            // Lock held, skip signal check - will happen on next timer
            // interrupt. A deferred SIGKILL cannot wait for that.
            crate::signal::delivery::exit_if_killed_on_syscall_return();
            return false;
        }
    };

    if let Some(ref mut manager) = *manager_guard {
        // Find the process for this thread
        if let Some((_pid, process, shared_table)) =
            manager.find_process_and_shared_table_by_thread_mut(current_thread_id)
        {
            // Check interval timers
            crate::signal::delivery::check_and_fire_alarm(process);
            crate::signal::delivery::check_and_fire_itimer_real(process, 5000);

            // Check if there are any deliverable signals, or a stop in force
            if !crate::signal::delivery::needs_action_on_return_to_user(process) {
                return false;
            }

            // A stop is acted on before any other signal: the thread is held
            // until SIGCONT, and what is pending then is delivered after.
            if crate::signal::delivery::stop_pending_or_in_force(process) {
                return true;
            }

            // A default action that ends the process must take effect before
            // Ring 3 runs again. It is carried out without the lock held and
            // does not return.
            if let Some(sig) = crate::signal::delivery::take_fatal_default_signal(process) {
                drop(manager_guard);
                crate::signal::delivery::exit_by_signal_on_syscall_return(sig);
            }

            // We have deliverable signals - need to set up signal frame
            // First, switch to process's page table for signal delivery
            // (signal delivery writes to user stack memory)
            if let Some(ref page_table) = process.page_table {
                let page_table_frame = page_table.level_4_frame();
                let cr3_value = page_table_frame.start_address().as_u64();
                unsafe {
                    use x86_64::registers::control::{Cr3, Cr3Flags};
                    use x86_64::structures::paging::PhysFrame;
                    use x86_64::PhysAddr;
                    Cr3::write(
                        PhysFrame::containing_address(PhysAddr::new(cr3_value)),
                        Cr3Flags::empty(),
                    );
                }
            }

            let mut user_return = crate::signal::delivery::X86UserReturn {
                rip: frame.rip,
                rsp: frame.rsp,
                rflags: frame.rflags,
            };

            // Create saved registers from syscall frame
            let mut saved_regs = crate::task::process_context::SavedRegisters {
                rax: frame.rax,
                rbx: frame.rbx,
                rcx: frame.rcx,
                rdx: frame.rdx,
                rsi: frame.rsi,
                rdi: frame.rdi,
                rbp: frame.rbp,
                r8: frame.r8,
                r9: frame.r9,
                r10: frame.r10,
                r11: frame.r11,
                r12: frame.r12,
                r13: frame.r13,
                r14: frame.r14,
                r15: frame.r15,
            };

            // Deliver the signal
            let signal_result = crate::signal::delivery::deliver_caught_signal_on_syscall_return(
                process,
                shared_table,
                &mut user_return,
                &mut saved_regs,
            );

            // A frame that cannot be installed ends the process outside PM.
            if let crate::signal::delivery::SignalDeliveryResult::FrameFault = signal_result {
                drop(manager_guard);
                crate::signal::delivery::exit_frame_fault_on_syscall_return();
            }

            // Copy modified values back to syscall frame
            frame.rip = user_return.rip;
            frame.rsp = user_return.rsp;
            frame.rflags = user_return.rflags;
            frame.rax = saved_regs.rax;
            frame.rbx = saved_regs.rbx;
            frame.rcx = saved_regs.rcx;
            frame.rdx = saved_regs.rdx;
            frame.rsi = saved_regs.rsi;
            frame.rdi = saved_regs.rdi;
            frame.rbp = saved_regs.rbp;
            frame.r8 = saved_regs.r8;
            frame.r9 = saved_regs.r9;
            frame.r10 = saved_regs.r10;
            frame.r11 = saved_regs.r11;
            frame.r12 = saved_regs.r12;
            frame.r13 = saved_regs.r13;
            frame.r14 = saved_regs.r14;
            frame.r15 = saved_regs.r15;

            // Handle termination case
            if let crate::signal::delivery::SignalDeliveryResult::Terminated(_notification) =
                signal_result
            {
                // Process was terminated by signal - switch to idle
                crate::task::scheduler::set_need_resched();
                crate::task::scheduler::switch_to_idle();
                // Note: parent notification will happen through normal scheduler path
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that is_ring3_confirmed() returns false initially (before any Ring 3 syscalls)
    ///
    /// NOTE: This test can only verify the initial state. Once RING3_CONFIRMED is set
    /// to true by a real syscall, it cannot be reset (by design - it's a one-way flag).
    /// The actual state change from false->true is tested implicitly by the kernel
    /// boot process and verified by the RING3_CONFIRMED marker in serial output.
    #[test]
    fn test_is_ring3_confirmed_initial_state() {
        // In a test context, RING3_CONFIRMED starts as false.
        // NOTE: If other tests in this test run have already triggered a syscall,
        // this test may see true. The important behavior is the one-way transition.
        let initial = RING3_CONFIRMED.load(Ordering::Relaxed);

        // If it's false, verify is_ring3_confirmed() returns false
        if !initial {
            assert!(!is_ring3_confirmed());
        }
        // If it's already true (from another test), that's also valid - the flag
        // should never go back to false once set.
    }

    /// Test the atomic swap behavior of RING3_CONFIRMED
    ///
    /// The key property: swap(true) returns the previous value, allowing
    /// exactly-once detection of the first Ring 3 syscall.
    #[test]
    fn test_ring3_confirmed_swap_behavior() {
        // Get current state
        let was_confirmed = RING3_CONFIRMED.load(Ordering::Relaxed);

        // Swap to true
        let prev = RING3_CONFIRMED.swap(true, Ordering::SeqCst);

        // If it wasn't confirmed before, swap should return false
        // If it was confirmed, swap returns true
        assert_eq!(prev, was_confirmed);

        // After swap, should always be true
        assert!(is_ring3_confirmed());

        // Second swap should return true (idempotent)
        let second = RING3_CONFIRMED.swap(true, Ordering::SeqCst);
        assert!(second);
    }
}
