// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║                         🚨 CRITICAL HOT PATH 🚨                               ║
// ║                                                                              ║
// ║  THIS FILE IS ON THE PROHIBITED MODIFICATIONS LIST.                          ║
// ║  sys_clock_gettime is called repeatedly in tight loops for precision timing. ║
// ║                                                                              ║
// ║  DO NOT ADD ANY LOGGING. See kernel/src/syscall/handler.rs for full rules.   ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

use crate::arch_impl::traits::CpuOps;
use crate::syscall::{ErrorCode, SyscallResult};
use crate::time::{get_monotonic_time_ns, get_real_time_ns};

#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

#[cfg(target_arch = "x86_64")]
type Cpu = crate::arch_impl::x86_64::cpu::X86Cpu;

/// POSIX clock identifiers
pub const CLOCK_REALTIME: u32 = 0;
pub const CLOCK_MONOTONIC: u32 = 1;

/// Kernel‑internal representation of `struct timespec`
/// Matches the POSIX ABI layout exactly.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Timespec {
    pub tv_sec: i64,  // seconds since Unix epoch
    pub tv_nsec: i64, // nanoseconds [0, 999 999 999]
}

/// Internal clock_gettime implementation for kernel use.
///
/// Returns the current time for the specified clock without any
/// userspace memory operations. Use this from kernel code that
/// needs to read the system time directly.
///
/// Granularity: nanosecond precision via TSC (falls back to 1ms via PIT).
pub fn clock_gettime(clock_id: u32) -> Result<Timespec, ErrorCode> {
    match clock_id {
        CLOCK_REALTIME => {
            let (secs, nanos) = get_real_time_ns();
            Ok(Timespec {
                tv_sec: secs,
                tv_nsec: nanos,
            })
        }
        CLOCK_MONOTONIC => {
            let (secs, nanos) = get_monotonic_time_ns();
            Ok(Timespec {
                tv_sec: secs as i64,
                tv_nsec: nanos as i64,
            })
        }
        _ => Err(ErrorCode::InvalidArgument),
    }
}

/// Syscall #228 — clock_gettime(clock_id, *timespec)
///
/// This is the userspace syscall entry point. It gets the time via
/// `clock_gettime()` and copies the result to userspace memory.
///
/// For kernel code that needs to read the time, use `clock_gettime()`
/// directly instead of this syscall wrapper.
///
/// NOTE: No logging in this hot path! Serial I/O takes thousands of cycles
/// and would cause the sub-millisecond precision test to fail.
pub fn sys_clock_gettime(clock_id: u32, user_ptr: *mut Timespec) -> SyscallResult {
    // Get the time using internal implementation
    let ts = match clock_gettime(clock_id) {
        Ok(ts) => ts,
        Err(e) => {
            return SyscallResult::Err(e as u64);
        }
    };

    // Copy result to userspace using architecture-independent userptr module
    if let Err(_e) = crate::syscall::userptr::copy_to_user(user_ptr, &ts) {
        return SyscallResult::Err(ErrorCode::Fault as u64);
    }

    SyscallResult::Ok(0)
}

/// Restore TTBR0 to the current thread's process page tables.
///
/// After a blocking syscall resumes (nanosleep, waitpid, etc.), TTBR0 may
/// point to a different process's address space if the context switch's
/// try_manager() hit PM lock contention. This function uses the blocking
/// manager() to reliably restore the correct page tables before returning
/// to userspace.
#[cfg(target_arch = "aarch64")]
fn ensure_current_address_space() {
    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return,
    };

    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((_pid, process)) = manager.find_process_by_thread(thread_id) {
            if let Some(ref page_table) = process.page_table {
                let ttbr0_value = page_table.level_4_frame().start_address().as_u64();
                unsafe {
                    core::arch::asm!(
                        "dsb ishst",
                        "msr ttbr0_el1, {}",
                        "isb",
                        "tlbi vmalle1is",
                        "dsb ish",
                        "isb",
                        in(reg) ttbr0_value,
                        options(nomem, nostack)
                    );
                }
            }
        }
    }
}

/// Syscall #35 — nanosleep(req, rem)
///
/// Suspends the calling thread for the time specified in `req`.
/// If interrupted by a signal, writes the remaining time to `rem` (if non-null)
/// and returns -EINTR.
///
/// Note: This is NOT a hot path (called once per sleep), so brief logging is acceptable.
pub fn sys_nanosleep(req_ptr: u64, _rem_ptr: u64) -> SyscallResult {
    // Read the requested sleep duration from userspace
    let req: Timespec = match crate::syscall::userptr::copy_from_user(req_ptr as *const Timespec) {
        Ok(ts) => ts,
        Err(_) => return SyscallResult::Err(ErrorCode::Fault as u64),
    };

    // Validate the timespec
    if req.tv_nsec < 0 || req.tv_nsec >= 1_000_000_000 || req.tv_sec < 0 {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }

    // Calculate absolute wake time
    let (cur_secs, cur_nanos) = get_monotonic_time_ns();
    let now_ns = cur_secs as u64 * 1_000_000_000 + cur_nanos as u64;
    let sleep_ns = req.tv_sec as u64 * 1_000_000_000 + req.tv_nsec as u64;
    let wake_time_ns = now_ns.saturating_add(sleep_ns);

    // Zero-length sleep returns immediately
    if sleep_ns == 0 {
        return SyscallResult::Ok(0);
    }

    // Block the thread until wake_time_ns using the scheduler's timer infrastructure.
    // This follows the same pattern as waitpid/pause: block → preempt_enable → HLT loop.
    // The scheduler's wake_expired_timers() runs every scheduling decision and moves
    // BlockedOnTimer threads back to the ready queue when their wake time has passed.
    crate::task::scheduler::with_scheduler(|sched| {
        sched.block_current_for_timer(wake_time_ns);
    });

    // Enable preemption so timer interrupts can context-switch us out.
    // Syscall handlers run with preempt_count > 0; we must explicitly lower it
    // to allow the scheduler to switch to other threads while we sleep.
    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_enable();
    #[cfg(target_arch = "x86_64")]
    crate::per_cpu::preempt_enable();

    // HLT loop: sleep until the scheduler wakes us (timer expired).
    // Each WFI/HLT suspends the CPU until the next interrupt (timer at 1000Hz).
    // We call wake_expired_timers() directly to detect expiry immediately
    // rather than waiting for a scheduling decision on another CPU.
    loop {
        // Check for signals that should interrupt the sleep
        if let Some(e) = crate::syscall::check_signals_for_eintr() {
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
            ensure_current_address_space();
            return SyscallResult::Err(e as u64);
        }

        // Wake expired timers directly and check if our timer has expired.
        // This eliminates one tick of latency by not waiting for a scheduling
        // decision on another CPU to call wake_expired_timers().
        let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
            sched.wake_expired_timers();
            sched.current_thread_mut()
                .map(|t| t.state == crate::task::thread::ThreadState::BlockedOnTimer)
                .unwrap_or(false)
        });

        if !still_blocked.unwrap_or(false) {
            break;
        }

        crate::task::scheduler::yield_current();
        Cpu::halt_with_interrupts();
    }

    // Clear blocked_in_syscall flag and re-disable preemption before returning
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
        }
    });

    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_disable();
    #[cfg(target_arch = "x86_64")]
    crate::per_cpu::preempt_disable();

    // Restore TTBR0 to this process's page tables. After blocking and being
    // context-switched out/in, TTBR0 may point to a different process's address
    // space if the context switch's try_manager() hit PM lock contention.
    #[cfg(target_arch = "aarch64")]
    ensure_current_address_space();

    SyscallResult::Ok(0)
}
