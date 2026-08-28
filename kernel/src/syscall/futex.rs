//! Futex (fast userspace mutex) syscall implementation
//!
//! Provides FUTEX_WAIT and FUTEX_WAKE operations for userspace synchronization.
//! Used by pthread_join, mutexes, condition variables, and similar primitives.

use super::SyscallResult;
use alloc::collections::BTreeMap;
use spin::Mutex;

use crate::arch_impl::traits::CpuOps;
use crate::task::thread::ThreadState;
use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};

#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

#[cfg(target_arch = "x86_64")]
type Cpu = crate::arch_impl::x86_64::cpu::X86Cpu;

/// Futex operation codes (Linux-compatible).
const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
/// Mask to extract the operation (ignoring FUTEX_PRIVATE_FLAG etc.).
const FUTEX_CMD_MASK: u32 = 0x7f;

/// Key for futex wait queues: (thread_group_id, virtual_address).
/// Threads sharing an address space (CLONE_VM) use the same thread_group_id,
/// so a futex at the same virtual address maps to the same wait queue.
type FutexKey = (u64, u64);

/// Global futex wait queue registry.
/// Maps (thread_group_id, vaddr) to a scheduler-integrated wait queue.
static FUTEX_QUEUES: Mutex<BTreeMap<FutexKey, WaitQueueHead>> = Mutex::new(BTreeMap::new());

/// Get the thread group ID for the current process.
///
/// The process-manager guard is dropped when this function returns. Callers
/// must resolve this before taking FUTEX_QUEUES so the lock order remains
/// process manager -> futex map -> waitqueue -> scheduler.
fn current_thread_group_id() -> Option<u64> {
    let thread_id = crate::task::scheduler::current_thread_id()?;
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((pid, process)) = manager.find_process_by_thread(thread_id) {
            return Some(process.thread_group_id.unwrap_or(pid.as_u64()));
        }
    }
    None
}

/// sys_futex - futex system call.
pub fn sys_futex(
    uaddr: u64,
    op: u32,
    val: u32,
    timeout: u64,
    _uaddr2: u64,
    val3: u32,
) -> SyscallResult {
    let cmd = op & FUTEX_CMD_MASK;

    match cmd {
        FUTEX_WAIT => futex_wait(uaddr, val, timeout, val3),
        FUTEX_WAKE => futex_wake(uaddr, val, val3),
        _ => SyscallResult::Err(super::errno::ENOSYS as u64),
    }
}

/// FUTEX_WAIT: atomically check *uaddr == expected_val and enqueue the
/// current thread if it matches.
fn futex_wait(uaddr: u64, expected_val: u32, timeout_ptr: u64, _val3: u32) -> SyscallResult {
    // Arming handshake for the #584 oracle driver. This arm is compiled in only
    // where the oracle seam itself is; a production kernel ignores val3, honours
    // the probe's timeout and returns ETIMEDOUT, which is how the driver learns
    // the seam is absent and skips instead of blocking init forever.
    #[cfg(feature = "boot_tests")]
    if crate::syscall::futex_oracle::is_probe(_val3) {
        return SyscallResult::Ok(crate::syscall::futex_oracle::PROBE_ACK);
    }

    if uaddr == 0 || uaddr % 4 != 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    if crate::syscall::userptr::validate_user_ptr_read::<u32>(uaddr as *const u32).is_err() {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }

    let (user_wake_time_ns, zero_timeout) = if timeout_ptr != 0 {
        let timeout = match crate::syscall::userptr::copy_from_user::<crate::syscall::time::Timespec>(
            timeout_ptr as *const crate::syscall::time::Timespec,
        ) {
            Ok(timeout) => timeout,
            Err(_) => return SyscallResult::Err(super::errno::EFAULT as u64),
        };

        if timeout.tv_nsec < 0 || timeout.tv_nsec >= 1_000_000_000 || timeout.tv_sec < 0 {
            return SyscallResult::Err(super::errno::EINVAL as u64);
        }

        let (cur_secs, cur_nanos) = crate::time::get_monotonic_time_ns();
        let now_ns = cur_secs as u64 * 1_000_000_000 + cur_nanos as u64;
        let relative_ns = timeout.tv_sec as u64 * 1_000_000_000 + timeout.tv_nsec as u64;
        (Some(now_ns.saturating_add(relative_ns)), relative_ns == 0)
    } else {
        (None, false)
    };

    // Pre-touch the word before taking any lock. The in-lock read below is
    // intentionally a single volatile read; this tree has no user-copy fault
    // fixup table to recover if the mapping disappears after this touch.
    if crate::syscall::userptr::copy_from_user::<u32>(uaddr as *const u32).is_err() {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };

    // Resolve the process-manager state before taking FUTEX_QUEUES. The
    // manager guard is fully released by current_thread_group_id's return.
    let tg_id = match current_thread_group_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };

    #[cfg(feature = "boot_tests")]
    let oracle_stage = crate::syscall::futex_oracle::arm_from_val3(_val3);
    #[cfg(feature = "boot_tests")]
    let oracle_deadline =
        oracle_stage.map(|stage| crate::syscall::futex_oracle::record_arm(stage, tg_id, uaddr));

    #[cfg(feature = "boot_tests")]
    if oracle_stage == Some(crate::syscall::futex_oracle::Stage::S1) {
        // This seam must remain immediately before the check+enqueue section.
        // It drives the race while the map lock is still available to the wake.
        crate::syscall::futex_oracle::stage1_drive(tg_id, uaddr, expected_val);
    }

    let key = (tg_id, uaddr);
    let effective_wake_time_ns = {
        #[cfg(feature = "boot_tests")]
        {
            match (user_wake_time_ns, oracle_deadline) {
                (Some(user_deadline), Some(oracle_deadline)) => {
                    Some(core::cmp::min(user_deadline, oracle_deadline))
                }
                (Some(user_deadline), None) => Some(user_deadline),
                (None, Some(oracle_deadline)) => Some(oracle_deadline),
                (None, None) => None,
            }
        }
        #[cfg(not(feature = "boot_tests"))]
        {
            user_wake_time_ns
        }
    };

    crate::proof_cover!(FutexSection);

    #[cfg(not(feature = "coreproof_mut_futex_section"))]
    let mut value_matches = false;

    // CORE-PROOF MUTATION LEG `coreproof_mut_futex_section` (#584, fixed by PR
    // #604): the value check runs in its OWN critical section, which is then
    // dropped before the section below re-takes the map lock to enqueue and
    // publish the waiter. A wake landing in that gap is lost. PR #604 made
    // check, enqueue and publication one section, which is the unmutated form
    // immediately below. Test profiles only.
    #[cfg(feature = "coreproof_mut_futex_section")]
    let value_matches = {
        let _queues = FUTEX_QUEUES.lock();
        // SAFETY: The address was validated and pre-touched above.
        let current_val = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
        current_val == expected_val
    };
    #[cfg(feature = "coreproof_mut_futex_section")]
    let split_precheck = value_matches && !zero_timeout;

    let prepare_outcome = {
        let mut queues = FUTEX_QUEUES.lock();
        let waitqueue = queues.entry(key).or_insert_with(WaitQueueHead::new);
        let outcome = waitqueue.prepare_to_wait_checked(
            ThreadState::BlockedOnIO,
            effective_wake_time_ns,
            || {
                #[cfg(feature = "coreproof_mut_futex_section")]
                {
                    split_precheck
                }
                #[cfg(not(feature = "coreproof_mut_futex_section"))]
                {
                    // SAFETY: The address was validated and pre-touched above.
                    // A concurrent unmap remains a documented residual risk.
                    let current_val =
                        unsafe { core::ptr::read_volatile(uaddr as *const u32) };
                    value_matches = current_val == expected_val;
                    value_matches && !zero_timeout
                }
            },
        );

        if outcome != PrepareOutcome::Queued && !waitqueue.has_waiters() {
            queues.remove(&key);
        }
        outcome
    };

    match prepare_outcome {
        PrepareOutcome::Mismatch => {
            #[cfg(feature = "boot_tests")]
            oracle_finish(
                oracle_stage,
                false,
                if zero_timeout && value_matches {
                    crate::syscall::futex_oracle::OracleRet::Etimedout
                } else {
                    crate::syscall::futex_oracle::OracleRet::Eagain
                },
            );
            if zero_timeout && value_matches {
                SyscallResult::Err(super::errno::ETIMEDOUT as u64)
            } else {
                SyscallResult::Err(super::errno::EAGAIN as u64)
            }
        }
        PrepareOutcome::PublishFailed => {
            #[cfg(feature = "boot_tests")]
            oracle_finish(
                oracle_stage,
                false,
                crate::syscall::futex_oracle::OracleRet::Esrch,
            );
            SyscallResult::Err(super::errno::ESRCH as u64)
        }
        PrepareOutcome::Queued => {
            #[cfg(feature = "boot_tests")]
            {
                if let Some(stage) = oracle_stage {
                    crate::syscall::futex_oracle::record_enqueued(stage);
                }
            }

            #[cfg(feature = "boot_tests")]
            if oracle_stage == Some(crate::syscall::futex_oracle::Stage::S2) {
                // First point after the critical section at which a waker can
                // observe the waiter. The map lock is dropped above.
                crate::syscall::futex_oracle::stage2_drive(tg_id, uaddr);
            }

            #[cfg(target_arch = "aarch64")]
            crate::per_cpu_aarch64::preempt_enable();
            #[cfg(target_arch = "x86_64")]
            crate::per_cpu::preempt_enable();

            #[cfg(feature = "boot_tests")]
            let mut oracle_parked = false;
            let mut signal_pending = false;
            // What the timer heap's pop of this wait saw, carried out of the
            // loop for the record below (#608 F4). It is a plain field read
            // taken inside the scheduler access this loop already performs.
            let mut timer_pop_wake_time_set: Option<bool> = None;
            loop {
                if crate::syscall::check_signals_for_eintr().is_some() {
                    signal_pending = true;
                    break;
                }

                let (still_waiting, pop_observation) =
                    crate::task::scheduler::with_scheduler(|sched| {
                        sched.wake_expired_timers();
                        sched
                            .current_thread_mut()
                            .map(|thread| {
                                (
                                    thread.state == ThreadState::BlockedOnIO,
                                    thread.timer_pop_wake_time_set,
                                )
                            })
                            .unwrap_or((false, None))
                    })
                    .unwrap_or((false, None));

                if pop_observation.is_some() {
                    timer_pop_wake_time_set = pop_observation;
                }

                if !still_waiting {
                    break;
                }

                #[cfg(feature = "boot_tests")]
                if let Some(deadline) = oracle_deadline {
                    if crate::syscall::futex_oracle::deadline_passed(deadline) {
                        crate::syscall::futex_oracle::record_parked(
                            oracle_stage.expect("oracle deadline requires an armed stage"),
                        );
                        oracle_parked = true;
                        break;
                    }
                }

                crate::task::scheduler::yield_current();
                Cpu::halt_with_interrupts();
            }

            #[cfg(target_arch = "aarch64")]
            crate::per_cpu_aarch64::preempt_disable();
            #[cfg(target_arch = "x86_64")]
            crate::per_cpu::preempt_disable();

            let removed_by_me = {
                let mut queues = FUTEX_QUEUES.lock();
                let waitqueue = queues.entry(key).or_insert_with(WaitQueueHead::new);
                let removed_by_me = waitqueue.take_waiter(thread_id);
                waitqueue.finish_wait();
                if !waitqueue.has_waiters() {
                    queues.remove(&key);
                }
                removed_by_me
            };

            #[cfg(target_arch = "aarch64")]
            ensure_current_address_space();

            #[cfg(feature = "boot_tests")]
            if removed_by_me && !oracle_parked {
                if let (Some(stage), Some(deadline)) = (oracle_stage, oracle_deadline) {
                    let user_timeout_expired = user_wake_time_ns.is_some_and(|deadline| {
                        let (seconds, nanos) = crate::time::get_monotonic_time_ns();
                        let now_ns = seconds as u64 * 1_000_000_000 + nanos as u64;
                        now_ns >= deadline
                    });

                    if crate::syscall::futex_oracle::deadline_passed(deadline)
                        && !user_timeout_expired
                    {
                        crate::syscall::futex_oracle::record_parked(stage);
                        oracle_parked = true;
                    }
                }
            }

            // One clock read, taken once and used by both the arbitration and
            // the record, so the record reports the value the decision was
            // actually made on rather than a second, later sample.
            let arbitration_now_ns = user_wake_time_ns.map(|_| {
                let (seconds, nanos) = crate::time::get_monotonic_time_ns();
                seconds as u64 * 1_000_000_000 + nanos as u64
            });
            let deadline_reached = match (user_wake_time_ns, arbitration_now_ns) {
                (Some(deadline), Some(now_ns)) => now_ns >= deadline,
                _ => false,
            };

            let result = if removed_by_me {
                if signal_pending {
                    SyscallResult::Err(super::errno::EINTR as u64)
                } else if deadline_reached {
                    SyscallResult::Err(super::errno::ETIMEDOUT as u64)
                } else {
                    SyscallResult::Ok(0)
                }
            } else {
                // The waker won the queue arbitration, even if the deadline
                // or a signal became observable at the same time.
                SyscallResult::Ok(0)
            };

            // The failure arbitration of a timed wait: the caller asked for a
            // deadline and is about to be told something other than
            // ETIMEDOUT, having either come out of the wait with nobody
            // dequeuing it or come out after its deadline had already passed.
            // A wait a real waker satisfied before its deadline is the one
            // shape excluded here, because that is the shape that is correct.
            if let (Some(deadline), Some(now_ns)) = (user_wake_time_ns, arbitration_now_ns) {
                let timed_out = matches!(
                    result,
                    SyscallResult::Err(errno) if errno == super::errno::ETIMEDOUT as u64
                );
                if !timed_out && (removed_by_me || deadline_reached) {
                    crate::syscall::futex_timeout_record::record(
                        &crate::syscall::futex_timeout_record::TimedWaitRecord {
                            thread_id,
                            removed_by_me,
                            signal_pending,
                            user_deadline_ns: deadline,
                            now_ns,
                            timer_pop_wake_time_set,
                            errno: match result {
                                SyscallResult::Err(errno) => errno,
                                SyscallResult::Ok(_) => 0,
                            },
                        },
                    );
                }
            }

            #[cfg(feature = "boot_tests")]
            oracle_finish(
                oracle_stage,
                true,
                if oracle_parked {
                    crate::syscall::futex_oracle::OracleRet::Rescued
                } else {
                    match result {
                        SyscallResult::Ok(_) => crate::syscall::futex_oracle::OracleRet::Zero,
                        SyscallResult::Err(errno) if errno == super::errno::EINTR as u64 => {
                            crate::syscall::futex_oracle::OracleRet::Eintr
                        }
                        SyscallResult::Err(errno) if errno == super::errno::ETIMEDOUT as u64 => {
                            crate::syscall::futex_oracle::OracleRet::Etimedout
                        }
                        _ => crate::syscall::futex_oracle::OracleRet::Other,
                    }
                },
            );

            result
        }
    }
}

/// FUTEX_WAKE: wake up to `max_wake` threads waiting on the futex at `uaddr`.
fn futex_wake(uaddr: u64, max_wake: u32, _val3: u32) -> SyscallResult {
    #[cfg(feature = "boot_tests")]
    if crate::syscall::futex_oracle::is_report(_val3) {
        crate::syscall::futex_oracle::report();
        return SyscallResult::Ok(0);
    }

    if uaddr == 0 || uaddr % 4 != 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    let tg_id = match current_thread_group_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };

    let key = (tg_id, uaddr);
    let mut queues = FUTEX_QUEUES.lock();
    let Some(waitqueue) = queues.get_mut(&key) else {
        return SyscallResult::Ok(0);
    };

    let woken = waitqueue.wake_up_n(max_wake);
    if !waitqueue.has_waiters() {
        queues.remove(&key);
    }

    SyscallResult::Ok(woken as u64)
}

/// Perform a FUTEX_WAKE on a specific address for a specific thread group.
/// Used by thread exit to notify joiners via clear_child_tid.
pub fn futex_wake_for_thread_group(tg_id: u64, uaddr: u64, max_wake: u32) -> u32 {
    let key = (tg_id, uaddr);
    let mut queues = FUTEX_QUEUES.lock();
    let Some(waitqueue) = queues.get_mut(&key) else {
        return 0;
    };

    let woken = waitqueue.wake_up_n(max_wake);
    if !waitqueue.has_waiters() {
        queues.remove(&key);
    }
    woken
}

#[cfg(target_arch = "aarch64")]
/// Duplicate of time.rs's private helper because time.rs is prohibited from
/// modification.
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

#[cfg(feature = "boot_tests")]
pub(crate) fn oracle_queue_residual(keys: [FutexKey; 3]) -> u64 {
    let queues = FUTEX_QUEUES.lock();
    keys.iter()
        .filter_map(|key| queues.get(key))
        .map(|waitqueue| waitqueue.waiter_count() as u64)
        .sum()
}

#[cfg(feature = "boot_tests")]
fn oracle_finish(
    stage: Option<crate::syscall::futex_oracle::Stage>,
    enqueued: bool,
    ret: crate::syscall::futex_oracle::OracleRet,
) {
    if let Some(stage) = stage {
        crate::syscall::futex_oracle::record_return(
            stage,
            ret,
            crate::syscall::futex_oracle::elapsed_since_arm(stage),
        );
        if enqueued {
            crate::syscall::futex_oracle::record_left(stage);
        }
    }
}
