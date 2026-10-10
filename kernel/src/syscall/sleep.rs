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

// Publication and clock-change wakeups share this lock. A clock change cannot
// spend its wake before an absolute realtime sleeper has published its wait.
static REALTIME_SLEEPERS: spin::Mutex<alloc::collections::BTreeSet<u64>> =
    spin::Mutex::new(alloc::collections::BTreeSet::new());

pub fn realtime_changed() {
    crate::arch_without_interrupts(|| {
        let sleepers = REALTIME_SLEEPERS.lock();
        crate::task::scheduler::with_scheduler(|sched| {
            for &tid in sleepers.iter() {
                sched.unblock(tid);
            }
        });
    });
}

pub fn nanosleep(req_ptr: u64, rem_ptr: u64) -> SyscallResult {
    clock_nanosleep(1, 0, req_ptr, rem_ptr)
}

/// Relative sleeps use monotonic elapsed time even for CLOCK_REALTIME.
/// Absolute realtime sleeps are re-evaluated when the wall clock is set.
pub fn clock_nanosleep(clock: u32, flags: u64, req_ptr: u64, rem_ptr: u64) -> SyscallResult {
    if !matches!(clock, 0 | 1 | 7) || flags & !1 != 0 {
        return SyscallResult::Err(22);
    }
    let req: Timespec = match userptr::copy_from_user(req_ptr as *const Timespec) {
        Ok(ts) => ts,
        Err(e) => return SyscallResult::Err(e),
    };
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= 1_000_000_000 {
        return SyscallResult::Err(22);
    }
    let requested = i128::from(req.tv_sec) * 1_000_000_000 + i128::from(req.tv_nsec);
    let absolute = flags == 1;
    let realtime = absolute && clock == 0;
    let deadline = if absolute {
        requested
    } else {
        i128::from(crate::signal::monotonic_nanos()) + requested
    };
    let now = if realtime {
        let (secs, nanos) = crate::time::get_real_time_ns();
        i128::from(secs) * 1_000_000_000 + i128::from(nanos)
    } else {
        i128::from(crate::signal::monotonic_nanos())
    };
    if now >= deadline {
        return SyscallResult::Ok(0);
    }
    let tid = crate::task::scheduler::current_thread_id().unwrap_or(0);
    if realtime {
        crate::arch_without_interrupts(|| {
            REALTIME_SLEEPERS.lock().insert(tid);
        });
    }
    let interrupted = loop {
        if super::check_signals_for_wait().is_some() {
            break true;
        }
        let expired = crate::arch_without_interrupts(|| {
            let sleepers = realtime.then(|| REALTIME_SLEEPERS.lock());
            // Sample realtime first: conversion to the later monotonic sample
            // may round the wake later, but never before the wall deadline.
            let real_now = if realtime {
                let (secs, nanos) = crate::time::get_real_time_ns();
                i128::from(secs) * 1_000_000_000 + i128::from(nanos)
            } else {
                0
            };
            let mono = crate::signal::monotonic_nanos();
            let now = if realtime { real_now } else { i128::from(mono) };
            if now >= deadline {
                return true;
            }
            let wake = (i128::from(mono) + deadline - now).min(i128::from(u64::MAX)) as u64;
            crate::task::scheduler::with_scheduler(|sched| sched.block_current_for_timer(wake));
            drop(sleepers);
            false
        });
        if expired {
            break false;
        }
        // A signal generated before publication has already spent its wake.
        if super::check_signals_for_wait().is_some() {
            break true;
        }
        crate::per_cpu::preempt_enable();
        crate::task::scheduler::yield_current();
        Cpu::halt_with_interrupts();
        crate::per_cpu::preempt_disable();
    };
    if realtime {
        crate::arch_without_interrupts(|| {
            REALTIME_SLEEPERS.lock().remove(&tid);
        });
    }
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
        if !absolute && rem_ptr != 0 {
            let left = (deadline - i128::from(crate::signal::monotonic_nanos())).max(0);
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
