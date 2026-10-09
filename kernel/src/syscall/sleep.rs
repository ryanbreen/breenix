//! Interruptible sleep, separate from the precision clock syscall path.
use super::{time::Timespec, userptr, SyscallResult};
use crate::arch_impl::traits::CpuOps;
#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;
#[cfg(target_arch = "x86_64")]
type Cpu = crate::arch_impl::x86_64::cpu::X86Cpu;

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
                crate::arch_impl::aarch64::ttbr0::restore_process_ttbr0(ttbr0_value);
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
pub fn nanosleep(req_ptr: u64, rem_ptr: u64) -> SyscallResult {
    let req: Timespec = match userptr::copy_from_user(req_ptr as *const Timespec) {
        Ok(ts) => ts,
        Err(e) => return SyscallResult::Err(e),
    };
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= 1_000_000_000 {
        return SyscallResult::Err(22);
    }
    let duration = (req.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(req.tv_nsec as u64);
    if duration == 0 {
        return SyscallResult::Ok(0);
    }
    let deadline = crate::signal::monotonic_micros()
        .saturating_mul(1000)
        .saturating_add(duration);
    let interrupted = loop {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current_for_timer(deadline);
        });
        // Keep preemption disabled until the pending check is complete. A
        // signal generated before publication has already spent its wake.
        if super::check_signals_for_eintr().is_some() {
            break true;
        }
        if crate::signal::monotonic_micros().saturating_mul(1000) >= deadline {
            break false;
        }
        crate::per_cpu::preempt_enable();
        crate::task::scheduler::yield_current();
        Cpu::halt_with_interrupts();
        crate::per_cpu::preempt_disable();
    };
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
            thread.wake_time_ns = None;
            thread.set_running();
        }
    });
    #[cfg(target_arch = "aarch64")]
    ensure_current_address_space();
    if interrupted {
        if rem_ptr != 0 {
            let left =
                deadline.saturating_sub(crate::signal::monotonic_micros().saturating_mul(1000));
            let rem = Timespec {
                tv_sec: (left / 1_000_000_000) as i64,
                tv_nsec: (left % 1_000_000_000) as i64,
            };
            if let Err(e) = userptr::copy_to_user(rem_ptr as *mut Timespec, &rem) {
                return SyscallResult::Err(e);
            }
        }
        SyscallResult::Err(4)
    } else {
        SyscallResult::Ok(0)
    }
}
