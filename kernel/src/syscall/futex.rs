//! Futex (fast userspace mutex) syscall implementation
//!
//! Provides FUTEX_WAIT and FUTEX_WAKE operations for userspace synchronization.
//! Used by pthread_join, mutexes, condition variables, etc.

use super::SyscallResult;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

/// Futex operation codes (Linux-compatible)
const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
/// Mask to extract the operation (ignoring FUTEX_PRIVATE_FLAG etc.)
const FUTEX_CMD_MASK: u32 = 0x7f;

/// Key for futex wait queues: (thread_group_id, virtual_address)
/// Threads sharing an address space (CLONE_VM) use the same thread_group_id,
/// so a futex at the same virtual address maps to the same wait queue.
type FutexKey = (u64, u64);

/// Global futex wait queue registry
/// Maps (thread_group_id, vaddr) -> list of waiting thread IDs
static FUTEX_QUEUES: Mutex<BTreeMap<FutexKey, Vec<u64>>> = Mutex::new(BTreeMap::new());

/// Get the thread group ID for the current process.
/// For normal processes, this is the process ID.
/// For CLONE_VM threads, this is the parent's thread group ID.
fn current_thread_group_id() -> Option<u64> {
    let thread_id = crate::task::scheduler::current_thread_id()?;
    let manager_guard = crate::process::manager();
    if let Some(ref manager) = *manager_guard {
        if let Some((pid, process)) = manager.find_process_by_thread(thread_id) {
            // Use thread_group_id if set, otherwise use pid
            return Some(process.thread_group_id.unwrap_or(pid.as_u64()));
        }
    }
    None
}

/// sys_futex - futex system call
///
/// Arguments:
///   uaddr:   User-space address of the futex word (u32)
///   op:      Futex operation (FUTEX_WAIT=0, FUTEX_WAKE=1)
///   val:     Expected value (FUTEX_WAIT) or max threads to wake (FUTEX_WAKE)
///   timeout: Timeout pointer (currently ignored, 0 = no timeout)
///   uaddr2:  Second futex address (currently unused)
///   val3:    Third value (currently unused)
pub fn sys_futex(
    uaddr: u64,
    op: u32,
    val: u32,
    _timeout: u64,
    _uaddr2: u64,
    _val3: u32,
) -> SyscallResult {
    let cmd = op & FUTEX_CMD_MASK;

    match cmd {
        FUTEX_WAIT => futex_wait(uaddr, val, _val3),
        FUTEX_WAKE => futex_wake(uaddr, val, _val3),
        _ => {
            log::warn!("futex: unsupported operation {}", cmd);
            SyscallResult::Err(super::errno::ENOSYS as u64)
        }
    }
}

/// FUTEX_WAIT: Atomically check *uaddr == val, and if so, block.
///
/// Returns 0 on success (woken by FUTEX_WAKE).
/// Returns -EAGAIN if *uaddr != val.
/// Returns -EINTR if interrupted by a signal.
fn futex_wait(uaddr: u64, expected_val: u32, _val3: u32) -> SyscallResult {
    // Validate user pointer
    if uaddr == 0 || uaddr % 4 != 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }

    let thread_id = match crate::task::scheduler::current_thread_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };

    let tg_id = match current_thread_group_id() {
        Some(id) => id,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };

    #[cfg(feature = "boot_tests")]
    let oracle_stage = crate::syscall::futex_oracle::arm_from_val3(_val3);
    #[cfg(feature = "boot_tests")]
    let oracle_deadline = oracle_stage
        .map(|stage| crate::syscall::futex_oracle::record_arm(stage, tg_id, uaddr));
    #[cfg(feature = "boot_tests")]
    let mut oracle_enqueued = false;

    // CRITICAL: The check-and-block must be atomic with respect to FUTEX_WAKE.
    // On single-core Breenix, disabling interrupts is sufficient.
    // We read the user value and add to the wait queue under the futex lock.
    {
        // Read the current value at uaddr
        let current_val = match unsafe { read_user_u32(uaddr) } {
            Some(v) => v,
            None => {
                #[cfg(feature = "boot_tests")]
                oracle_finish(
                    oracle_stage,
                    oracle_enqueued,
                    tg_id,
                    uaddr,
                    thread_id,
                    crate::syscall::futex_oracle::OracleRet::Efault,
                );
                return SyscallResult::Err(super::errno::EFAULT as u64);
            }
        };

        // If value doesn't match expected, return EAGAIN (spurious wakeup semantics)
        if current_val != expected_val {
            #[cfg(feature = "boot_tests")]
            oracle_finish(
                oracle_stage,
                oracle_enqueued,
                tg_id,
                uaddr,
                thread_id,
                crate::syscall::futex_oracle::OracleRet::Eagain,
            );
            return SyscallResult::Err(super::errno::EAGAIN as u64);
        }

        // Value matches - add to wait queue and block
        let key = (tg_id, uaddr);

        #[cfg(feature = "boot_tests")]
        if oracle_stage == Some(crate::syscall::futex_oracle::Stage::S1) {
            // Oracle seam: immediately before the enqueue.
            crate::syscall::futex_oracle::stage1_drive(tg_id, uaddr, expected_val);
        }

        let mut queues = FUTEX_QUEUES.lock();
        queues.entry(key).or_insert_with(Vec::new).push(thread_id);
    }

    #[cfg(feature = "boot_tests")]
    {
        oracle_enqueued = true;
        if let Some(stage) = oracle_stage {
            crate::syscall::futex_oracle::record_enqueued(stage);
        }
    }

    #[cfg(feature = "boot_tests")]
    if oracle_stage == Some(crate::syscall::futex_oracle::Stage::S2) {
        crate::syscall::futex_oracle::stage2_drive(tg_id, uaddr);
    }

    // Block the current thread
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.state = crate::task::thread::ThreadState::Blocked;
            thread.blocked_in_syscall = true;
        }
        if let Some(current_id) = sched.current_thread_id_inner() {
            sched.remove_from_ready_queue(current_id);
        }
    });

    // Yield and wait for wake
    loop {
        crate::task::scheduler::yield_current();

        // Check if we've been woken (state changed from Blocked to Ready/Running)
        let still_blocked = crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.state == crate::task::thread::ThreadState::Blocked
            } else {
                false
            }
        })
        .unwrap_or(false);

        if !still_blocked {
            break;
        }

        // Check for signal interruption
        if let Some(_eintr) = crate::syscall::check_signals_for_eintr() {
            // Remove from wait queue
            let key = (tg_id, uaddr);
            let mut queues = FUTEX_QUEUES.lock();
            if let Some(waiters) = queues.get_mut(&key) {
                waiters.retain(|&id| id != thread_id);
                if waiters.is_empty() {
                    queues.remove(&key);
                }
            }

            // Unblock thread
            crate::task::scheduler::with_scheduler(|sched| {
                if let Some(thread) = sched.current_thread_mut() {
                    thread.blocked_in_syscall = false;
                    thread.set_ready();
                }
            });

            #[cfg(feature = "boot_tests")]
            oracle_finish(
                oracle_stage,
                oracle_enqueued,
                tg_id,
                uaddr,
                thread_id,
                crate::syscall::futex_oracle::OracleRet::Eintr,
            );
            return SyscallResult::Err(super::errno::EINTR as u64);
        }

        #[cfg(feature = "boot_tests")]
        if let Some(stage) = oracle_stage {
            if crate::syscall::futex_oracle::deadline_passed(oracle_deadline.unwrap()) {
                crate::syscall::futex_oracle::record_parked(stage);

                let key = (tg_id, uaddr);
                {
                    let mut queues = FUTEX_QUEUES.lock();
                    if let Some(waiters) = queues.get_mut(&key) {
                        waiters.retain(|&id| id != thread_id);
                        if waiters.is_empty() {
                            queues.remove(&key);
                        }
                    }
                }

                crate::task::scheduler::with_scheduler(|sched| {
                    if let Some(thread) = sched.current_thread_mut() {
                        thread.blocked_in_syscall = false;
                        thread.set_ready();
                    }
                });

                oracle_finish(
                    oracle_stage,
                    oracle_enqueued,
                    tg_id,
                    uaddr,
                    thread_id,
                    crate::syscall::futex_oracle::OracleRet::Rescued,
                );
                return SyscallResult::Ok(0);
            }
        }

        // Power-efficient wait
        #[cfg(target_arch = "x86_64")]
        x86_64::instructions::interrupts::enable_and_hlt();
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("wfe");
        }
    }

    // Clear blocked_in_syscall
    crate::task::scheduler::with_scheduler(|sched| {
        if let Some(thread) = sched.current_thread_mut() {
            thread.blocked_in_syscall = false;
        }
    });

    #[cfg(feature = "boot_tests")]
    oracle_finish(
        oracle_stage,
        oracle_enqueued,
        tg_id,
        uaddr,
        thread_id,
        crate::syscall::futex_oracle::OracleRet::Zero,
    );

    SyscallResult::Ok(0)
}

/// FUTEX_WAKE: Wake up to `val` threads waiting on the futex at uaddr.
///
/// Returns the number of threads woken.
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
    let mut woken = 0u32;

    // Remove threads from wait queue and unblock them
    let threads_to_wake: Vec<u64> = {
        let mut queues = FUTEX_QUEUES.lock();
        if let Some(waiters) = queues.get_mut(&key) {
            let wake_count = core::cmp::min(max_wake as usize, waiters.len());
            let to_wake: Vec<u64> = waiters.drain(..wake_count).collect();
            if waiters.is_empty() {
                queues.remove(&key);
            }
            to_wake
        } else {
            Vec::new()
        }
    };

    // Unblock each thread
    for tid in threads_to_wake {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.unblock(tid);
        });
        woken += 1;
    }

    SyscallResult::Ok(woken as u64)
}

/// Perform a FUTEX_WAKE on a specific address for a specific thread group.
/// Used by thread exit to notify joiners via clear_child_tid.
pub fn futex_wake_for_thread_group(tg_id: u64, uaddr: u64, max_wake: u32) -> u32 {
    let key = (tg_id, uaddr);
    let mut woken = 0u32;

    let threads_to_wake: Vec<u64> = {
        let mut queues = FUTEX_QUEUES.lock();
        if let Some(waiters) = queues.get_mut(&key) {
            let wake_count = core::cmp::min(max_wake as usize, waiters.len());
            let to_wake: Vec<u64> = waiters.drain(..wake_count).collect();
            if waiters.is_empty() {
                queues.remove(&key);
            }
            to_wake
        } else {
            Vec::new()
        }
    };

    for tid in threads_to_wake {
        crate::task::scheduler::with_scheduler(|sched| {
            sched.unblock(tid);
        });
        woken += 1;
    }

    woken
}

#[cfg(feature = "boot_tests")]
pub(crate) fn oracle_queue_residual(keys: [(u64, u64); 3]) -> u64 {
    let queues = FUTEX_QUEUES.lock();
    keys.iter()
        .filter_map(|key| queues.get(key))
        .map(|waiters| waiters.len() as u64)
        .sum()
}

#[cfg(feature = "boot_tests")]
pub(crate) fn oracle_waiter_present(tg_id: u64, uaddr: u64, thread_id: u64) -> bool {
    let queues = FUTEX_QUEUES.lock();
    queues
        .get(&(tg_id, uaddr))
        .is_some_and(|waiters| waiters.contains(&thread_id))
}

#[cfg(feature = "boot_tests")]
fn oracle_finish(
    stage: Option<crate::syscall::futex_oracle::Stage>,
    enqueued: bool,
    tg_id: u64,
    uaddr: u64,
    thread_id: u64,
    ret: crate::syscall::futex_oracle::OracleRet,
) {
    if let Some(stage) = stage {
        crate::syscall::futex_oracle::record_return(
            stage,
            ret,
            crate::syscall::futex_oracle::elapsed_since_arm(stage),
        );
        if enqueued && !oracle_waiter_present(tg_id, uaddr, thread_id) {
            crate::syscall::futex_oracle::record_left(stage);
        }
    }
}

/// Read a u32 from user-space memory. Returns None if the address is invalid.
unsafe fn read_user_u32(addr: u64) -> Option<u32> {
    // Basic validation: address must be in user-space range
    if addr == 0 || addr > 0x7FFF_FFFF_FFFF {
        return None;
    }

    // Read the value - we're in the process's address space during syscall
    let ptr = addr as *const u32;
    Some(core::ptr::read_volatile(ptr))
}
