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
            crate::test_framework::TestStage::Userspace
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

    // Dispatch to the appropriate syscall handler
    // NOTE: No logging here! This is the hot path.
    let result = match SyscallNumber::from_u64(syscall_num) {
        Some(SyscallNumber::Exit) => super::handlers::sys_exit(args.0 as i32),
        Some(SyscallNumber::Write) => super::handlers::sys_write(args.0, args.1, args.2),
        Some(SyscallNumber::Read) => super::handlers::sys_read(args.0, args.1, args.2),
        Some(SyscallNumber::Yield) => super::handlers::sys_yield(),
        Some(SyscallNumber::GetTime) => super::handlers::sys_get_time(),
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
        Some(SyscallNumber::Exec) => super::handlers::sys_execv_with_frame(frame, args.0, args.1),
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
        Some(SyscallNumber::Brk) => super::memory::sys_brk(args.0),
        Some(SyscallNumber::Kill) => super::signal::sys_kill(args.0 as i64, args.1 as i32),
        Some(SyscallNumber::Sigaction) => {
            super::signal::sys_sigaction(args.0 as i32, args.1, args.2, args.3)
        }
        Some(SyscallNumber::Sigprocmask) => {
            super::signal::sys_sigprocmask(args.0 as i32, args.1, args.2, args.3)
        }
        Some(SyscallNumber::Sigpending) => {
            super::signal::sys_sigpending(args.0, args.1)
        }
        Some(SyscallNumber::Sigsuspend) => {
            // sigsuspend(mask, sigsetsize) - atomically set mask and wait for signal
            // Needs frame access like pause() for saving userspace context
            super::signal::sys_sigsuspend_with_frame(args.0, args.1, frame)
        }
        Some(SyscallNumber::Sigaltstack) => {
            super::signal::sys_sigaltstack(args.0, args.1)
        }
        Some(SyscallNumber::Sigreturn) => {
            // CRITICAL: sigreturn restores ALL registers including RAX from the signal frame.
            // We must NOT overwrite RAX with the syscall return value after this call!
            // Return early to skip the set_return_value() call below.
            let result = super::signal::sys_sigreturn_with_frame(frame);
            if let SyscallResult::Err(errno) = result {
                // Only set return value on error - success case already has RAX set
                frame.set_return_value((-(errno as i64)) as u64);
            }
            // Perform cleanup that normally happens after result handling
            let kernel_stack_top = crate::per_cpu::kernel_stack_top();
            if kernel_stack_top != 0 {
                crate::gdt::set_tss_rsp0(VirtAddr::new(kernel_stack_top));
            }
            crate::irq_log::flush_local_try();
            crate::per_cpu::preempt_enable();
            return;
        }
        Some(SyscallNumber::Ioctl) => {
            super::ioctl::sys_ioctl(args.0, args.1, args.2)
        }
        Some(SyscallNumber::Socket) => {
            super::socket::sys_socket(args.0, args.1, args.2)
        }
        Some(SyscallNumber::Bind) => {
            super::socket::sys_bind(args.0, args.1, args.2)
        }
        Some(SyscallNumber::SendTo) => {
            super::socket::sys_sendto(args.0, args.1, args.2, args.3, args.4, args.5)
        }
        Some(SyscallNumber::RecvFrom) => {
            super::socket::sys_recvfrom(args.0, args.1, args.2, args.3, args.4, args.5)
        }
        Some(SyscallNumber::Connect) => {
            super::socket::sys_connect(args.0, args.1, args.2)
        }
        Some(SyscallNumber::Accept) => {
            super::socket::sys_accept(args.0, args.1, args.2)
        }
        Some(SyscallNumber::Listen) => {
            super::socket::sys_listen(args.0, args.1)
        }
        Some(SyscallNumber::Shutdown) => {
            super::socket::sys_shutdown(args.0, args.1)
        }
        Some(SyscallNumber::Getsockname) => {
            super::socket::sys_getsockname(args.0, args.1, args.2)
        }
        Some(SyscallNumber::Getpeername) => {
            super::socket::sys_getpeername(args.0, args.1, args.2)
        }
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
        Some(SyscallNumber::Setitimer) => super::signal::sys_setitimer(args.0 as i32, args.1, args.2),
        Some(SyscallNumber::Wait4) => {
            super::handlers::sys_waitpid(args.0 as i64, args.1, args.2 as u32)
        }
        Some(SyscallNumber::SetPgid) => {
            super::session::sys_setpgid(args.0 as i32, args.1 as i32)
        }
        Some(SyscallNumber::SetSid) => super::session::sys_setsid(),
        Some(SyscallNumber::GetPgid) => super::session::sys_getpgid(args.0 as i32),
        Some(SyscallNumber::GetSid) => super::session::sys_getsid(args.0 as i32),
        // Filesystem syscalls
        Some(SyscallNumber::Access) => super::fs::sys_access(args.0, args.1 as u32),
        Some(SyscallNumber::Getcwd) => super::fs::sys_getcwd(args.0, args.1),
        Some(SyscallNumber::Chdir) => super::fs::sys_chdir(args.0),
        Some(SyscallNumber::Open) => super::fs::sys_open(args.0, args.1 as u32, args.2 as u32),
        Some(SyscallNumber::Lseek) => super::fs::sys_lseek(args.0 as i32, args.1 as i64, args.2 as i32),
        Some(SyscallNumber::Fstat) => super::fs::sys_fstat(args.0 as i32, args.1),
        Some(SyscallNumber::Getdents64) => super::fs::sys_getdents64(args.0 as i32, args.1, args.2),
        Some(SyscallNumber::Rename) => super::fs::sys_rename(args.0, args.1),
        Some(SyscallNumber::Mkdir) => super::fs::sys_mkdir(args.0, args.1 as u32),
        Some(SyscallNumber::Rmdir) => super::fs::sys_rmdir(args.0),
        Some(SyscallNumber::Link) => super::fs::sys_link(args.0, args.1),
        Some(SyscallNumber::Unlink) => super::fs::sys_unlink(args.0),
        Some(SyscallNumber::Symlink) => super::fs::sys_symlink(args.0, args.1),
        Some(SyscallNumber::Readlink) => super::fs::sys_readlink(args.0, args.1, args.2),
        Some(SyscallNumber::Mknod) => super::fifo::sys_mknod(args.0, args.1 as u32, args.2),
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
        Some(SyscallNumber::Futex) => {
            super::futex::sys_futex(args.0, args.1 as u32, args.2 as u32, args.3, args.4, args.5 as u32)
        }
        // Graphics syscalls
        Some(SyscallNumber::FbInfo) => super::graphics::sys_fbinfo(args.0),
        Some(SyscallNumber::FbDraw) => super::graphics::sys_fbdraw(args.0),
        Some(SyscallNumber::FbMmap) => super::graphics::sys_fbmmap(),
        Some(SyscallNumber::GetMousePos) => super::graphics::sys_get_mouse_pos(args.0),
        // Audio syscalls
        Some(SyscallNumber::AudioInit) => super::audio::sys_audio_init(),
        Some(SyscallNumber::AudioWrite) => super::audio::sys_audio_write(args.0, args.1),
        // Display takeover
        Some(SyscallNumber::TakeOverDisplay) => super::handlers::sys_take_over_display(),
        Some(SyscallNumber::GiveBackDisplay) => super::handlers::sys_give_back_display(),
        None => {
            log::warn!("Unknown syscall number: {} - returning ENOSYS", syscall_num);
            SyscallResult::Err(super::ErrorCode::NoSys as u64)
        }
    };

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
    // Get current thread ID
    let current_thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };

    // Thread 0 is the idle thread - it doesn't have a process with signals
    if current_thread_id == 0 {
        return;
    }

    // Try to acquire process manager lock (non-blocking)
    let mut manager_guard = match crate::process::try_manager() {
        Some(guard) => guard,
        None => return, // Lock held, skip signal check - will happen on next timer interrupt
    };

    if let Some(ref mut manager) = *manager_guard {
        // Find the process for this thread
        if let Some((_pid, process)) = manager.find_process_by_thread_mut(current_thread_id) {
            // Check interval timers
            crate::signal::delivery::check_and_fire_alarm(process);
            crate::signal::delivery::check_and_fire_itimer_real(process, 5000);

            // Check if there are any deliverable signals
            if !crate::signal::delivery::has_deliverable_signals(process) {
                return;
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

            // Create an interrupt frame wrapper for the signal delivery code
            // We need to convert SyscallFrame to InterruptStackFrame equivalent
            let mut interrupt_frame = SyscallInterruptFrameWrapper {
                rip: frame.rip,
                cs: frame.cs,
                rflags: frame.rflags,
                rsp: frame.rsp,
                ss: frame.ss,
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
            let signal_result = deliver_pending_signals_syscall(
                process,
                &mut interrupt_frame,
                &mut saved_regs,
            );

            // Copy modified values back to syscall frame
            frame.rip = interrupt_frame.rip;
            frame.rsp = interrupt_frame.rsp;
            frame.rflags = interrupt_frame.rflags;
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
            if let crate::signal::delivery::SignalDeliveryResult::Terminated(_notification) = signal_result {
                // Process was terminated by signal - switch to idle
                crate::task::scheduler::set_need_resched();
                crate::task::scheduler::switch_to_idle();
                // Note: parent notification will happen through normal scheduler path
            }
        }
    }
}

/// Wrapper to make SyscallFrame work with signal delivery code
/// This mimics the InterruptStackFrame structure that signal delivery expects
#[allow(dead_code)]
struct SyscallInterruptFrameWrapper {
    rip: u64,
    cs: u64, // Used for consistency with interrupt frame layout
    rflags: u64,
    rsp: u64,
    ss: u64, // Used for consistency with interrupt frame layout
}

/// Deliver pending signals during syscall return
/// This is similar to deliver_pending_signals but works with our wrapper type
fn deliver_pending_signals_syscall(
    process: &mut crate::process::Process,
    frame: &mut SyscallInterruptFrameWrapper,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
) -> crate::signal::delivery::SignalDeliveryResult {
    use crate::signal::constants::*;

    // Process all deliverable signals in a loop
    loop {
        // Get next deliverable signal
        let sig = match process.signals.next_deliverable_signal() {
            Some(s) => s,
            None => return crate::signal::delivery::SignalDeliveryResult::NoAction,
        };

        // Clear pending flag for this signal
        process.signals.clear_pending(sig);

        // Get the handler for this signal
        let action = *process.signals.get_handler(sig);

        match action.handler {
            SIG_DFL => {
                // Default action - delegate to main delivery code
                // For simplicity, return NoAction and let timer interrupt handle it
                // This avoids duplicating termination logic here
                process.signals.set_pending(sig); // Re-queue for timer interrupt
                return crate::signal::delivery::SignalDeliveryResult::NoAction;
            }
            SIG_IGN => {
                // Signal ignored - continue to check for more signals
            }
            handler_addr => {
                // User-defined handler - set up signal frame
                deliver_to_user_handler_syscall(process, frame, saved_regs, sig, handler_addr, &action);
                return crate::signal::delivery::SignalDeliveryResult::Delivered;
            }
        }
    }
}

/// Set up user stack and registers to call a user-defined signal handler (syscall version)
fn deliver_to_user_handler_syscall(
    process: &mut crate::process::Process,
    frame: &mut SyscallInterruptFrameWrapper,
    saved_regs: &mut crate::task::process_context::SavedRegisters,
    sig: u32,
    handler_addr: u64,
    action: &crate::signal::types::SignalAction,
) {
    use crate::signal::constants::*;
    use crate::signal::types::*;

    // Get current user stack pointer
    let current_rsp = frame.rsp;
    let original_rsp = current_rsp;

    // Check if we should use the alternate signal stack
    let use_alt_stack = (action.flags & SA_ONSTACK) != 0
        && (process.signals.alt_stack.flags & SS_DISABLE) == 0
        && process.signals.alt_stack.size > 0
        && !process.signals.alt_stack.on_stack;

    let user_rsp = if use_alt_stack {
        // Use alternate stack - stack grows down, so start at top (base + size)
        let alt_top = process.signals.alt_stack.base + process.signals.alt_stack.size as u64;
        // Mark that we're now on the alternate stack
        process.signals.alt_stack.on_stack = true;
        alt_top
    } else {
        current_rsp
    };

    // Calculate space needed for signal frame (and optionally trampoline)
    let frame_size = SignalFrame::SIZE as u64;

    // Check if the handler provides a restorer function (SA_RESTORER flag)
    // If so, use it instead of writing trampoline to the stack.
    // This is essential for signals delivered on alternate stacks where the
    // stack may not be executable (NX bit set).
    let use_restorer = (action.flags & SA_RESTORER) != 0 && action.restorer != 0;

    let (frame_rsp, return_addr) = if use_restorer {
        // Use the restorer function provided by the application/libc
        let frame_rsp = (user_rsp - frame_size) & !0xF;
        (frame_rsp, action.restorer)
    } else {
        // Fall back to writing trampoline on the stack
        let trampoline_size = crate::signal::trampoline::SIGNAL_TRAMPOLINE_SIZE as u64;
        let total_size = frame_size + trampoline_size;
        let frame_rsp = (user_rsp - total_size) & !0xF;
        let trampoline_rsp = frame_rsp + frame_size;

        // Write trampoline code to user stack
        unsafe {
            let trampoline_ptr = trampoline_rsp as *mut u8;
            core::ptr::copy_nonoverlapping(
                crate::signal::trampoline::SIGNAL_TRAMPOLINE.as_ptr(),
                trampoline_ptr,
                crate::signal::trampoline::SIGNAL_TRAMPOLINE_SIZE,
            );
        }

        (frame_rsp, trampoline_rsp)
    };

    // Build signal frame with saved context
    let signal_frame = SignalFrame {
        trampoline_addr: return_addr,
        magic: SignalFrame::MAGIC,
        signal: sig as u64,
        siginfo_ptr: 0,
        ucontext_ptr: 0,
        saved_rip: frame.rip,
        saved_rsp: original_rsp,
        saved_rflags: frame.rflags,
        saved_rax: saved_regs.rax,
        saved_rbx: saved_regs.rbx,
        saved_rcx: saved_regs.rcx,
        saved_rdx: saved_regs.rdx,
        saved_rdi: saved_regs.rdi,
        saved_rsi: saved_regs.rsi,
        saved_rbp: saved_regs.rbp,
        saved_r8: saved_regs.r8,
        saved_r9: saved_regs.r9,
        saved_r10: saved_regs.r10,
        saved_r11: saved_regs.r11,
        saved_r12: saved_regs.r12,
        saved_r13: saved_regs.r13,
        saved_r14: saved_regs.r14,
        saved_r15: saved_regs.r15,
        saved_blocked: process.signals.blocked,
    };

    // Write signal frame to user stack
    unsafe {
        let frame_ptr = frame_rsp as *mut SignalFrame;
        core::ptr::write_volatile(frame_ptr, signal_frame);
    }

    // Block signals during handler execution
    if (action.flags & SA_NODEFER) == 0 {
        process.signals.block_signals(sig_mask(sig));
    }
    process.signals.block_signals(action.mask);

    // Modify frame to jump to signal handler
    frame.rip = handler_addr;
    frame.rsp = frame_rsp;

    // Set up arguments for signal handler: void handler(int signum)
    saved_regs.rdi = sig as u64;
    saved_regs.rsi = 0;
    saved_regs.rdx = 0;
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
