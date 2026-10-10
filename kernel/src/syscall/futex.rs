//! Futex (fast userspace mutex) syscall implementation
//!
//! FUTEX_WAIT, FUTEX_WAKE, FUTEX_WAIT_BITSET, FUTEX_WAKE_BITSET, FUTEX_REQUEUE
//! and FUTEX_CMP_REQUEUE, private and shared (#1311). Used by pthread_join,
//! mutexes, condition variables, and similar primitives.

use super::SyscallResult;
use alloc::collections::{BTreeMap, VecDeque};
use spin::Mutex;

use crate::arch_impl::traits::CpuOps;
use crate::task::thread::ThreadState;

#[cfg(target_arch = "aarch64")]
type Cpu = crate::arch_impl::aarch64::Aarch64Cpu;

#[cfg(target_arch = "x86_64")]
type Cpu = crate::arch_impl::x86_64::cpu::X86Cpu;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

/// Futex operation codes (Linux-compatible).
const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;
const FUTEX_REQUEUE: u32 = 3;
const FUTEX_CMP_REQUEUE: u32 = 4;
const FUTEX_WAIT_BITSET: u32 = 9;
const FUTEX_WAKE_BITSET: u32 = 10;
/// The futex is private to the calling process: no other process maps it.
const FUTEX_PRIVATE_FLAG: u32 = 128;
/// FUTEX_WAIT_BITSET's absolute timeout is on CLOCK_REALTIME, not
/// CLOCK_MONOTONIC.
const FUTEX_CLOCK_REALTIME: u32 = 256;
/// Mask to extract the operation from the flags.
const FUTEX_CMD_MASK: u32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
/// The bitset FUTEX_WAIT and FUTEX_WAKE use: every waiter matches.
const FUTEX_BITSET_MATCH_ANY: u32 = u32::MAX;

/// Key for futex wait queues. A private futex, and any futex outside a shared
/// mapping, is (thread_group_id, virtual address): threads sharing an address
/// space (CLONE_VM) share the thread group id. A futex in a shared mapping is
/// (`SHARED_FUTEX`, physical address), which every process mapping the page
/// reaches.
type FutexKey = (u64, u64);
const SHARED_FUTEX: u64 = u64::MAX;

/// A thread waiting on a futex, with the bitset its wait matches.
struct Waiter {
    tid: u64,
    bitset: u32,
}

/// Every futex wait queue, and the queue each waiting thread is on (a requeue
/// moves a waiter from one to another).
struct FutexTable {
    queues: BTreeMap<FutexKey, VecDeque<Waiter>>,
    queued_on: BTreeMap<u64, FutexKey>,
}

impl FutexTable {
    const fn new() -> Self {
        Self {
            queues: BTreeMap::new(),
            queued_on: BTreeMap::new(),
        }
    }

    fn enqueue(&mut self, key: FutexKey, waiter: Waiter) {
        self.queued_on.insert(waiter.tid, key);
        self.queues.entry(key).or_default().push_back(waiter);
    }

    /// Take `tid` off whichever queue it is on. Returns whether it was queued.
    fn remove(&mut self, tid: u64) -> bool {
        let Some(key) = self.queued_on.remove(&tid) else {
            return false;
        };
        if let Some(queue) = self.queues.get_mut(&key) {
            queue.retain(|waiter| waiter.tid != tid);
            if queue.is_empty() {
                self.queues.remove(&key);
            }
        }
        true
    }

    /// Take up to `max` waiters on `key` whose bitset meets `bitset` off the
    /// queue, oldest first, and wake each.
    fn wake(&mut self, key: FutexKey, max: u32, bitset: u32) -> u32 {
        let Some(queue) = self.queues.get_mut(&key) else {
            return 0;
        };
        let mut woken = 0;
        let mut index = 0;
        while woken < max && index < queue.len() {
            if queue[index].bitset & bitset == 0 {
                index += 1;
                continue;
            }
            let Some(waiter) = queue.remove(index) else {
                break;
            };
            self.queued_on.remove(&waiter.tid);
            crate::task::scheduler::wake_waitqueue_thread(waiter.tid);
            woken += 1;
        }
        if queue.is_empty() {
            self.queues.remove(&key);
        }
        woken
    }

    /// Move up to `max` waiters from `from` to the end of `to`, oldest first,
    /// without waking them.
    fn requeue(&mut self, from: FutexKey, to: FutexKey, max: u32) -> u32 {
        if from == to {
            return self.queues.get(&from).map_or(0, |queue| queue.len().min(max as usize) as u32);
        }
        let mut moved = 0;
        while moved < max {
            let Some(waiter) = self.queues.get_mut(&from).and_then(|queue| queue.pop_front()) else {
                break;
            };
            self.queued_on.insert(waiter.tid, to);
            self.queues.entry(to).or_default().push_back(waiter);
            moved += 1;
        }
        if self.queues.get(&from).is_some_and(|queue| queue.is_empty()) {
            self.queues.remove(&from);
        }
        moved
    }

    #[cfg(feature = "boot_tests")]
    fn count(&self, key: &FutexKey) -> usize {
        self.queues.get(key).map_or(0, |queue| queue.len())
    }
}

/// How a wait's check-and-enqueue section under the futex table ended.
#[derive(PartialEq)]
enum Prepared {
    /// The word could not be read (EFAULT).
    Fault,
    /// The word did not hold the expected value.
    Mismatch,
    /// The absolute deadline had already passed.
    Expired,
    /// The waiter was enqueued and its blocked state was published.
    Queued,
    /// The waiter could not be published and was removed again.
    PublishFailed,
}

/// Global futex wait-queue registry, only taken with interrupts masked and
/// never with PROCESS_MANAGER held: the user-word read under it may fault,
/// and resolving the fault can take PROCESS_MANAGER. The scheduler lock is
/// taken under it.
static FUTEX_QUEUES: Mutex<FutexTable> = Mutex::new(FutexTable::new());

fn with_table<R>(f: impl FnOnce(&mut FutexTable) -> R) -> R {
    crate::arch_without_interrupts(|| f(&mut FUTEX_QUEUES.lock()))
}

fn monotonic_ns() -> u64 {
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    secs * 1_000_000_000 + nanos
}

fn realtime_ns() -> i128 {
    let (secs, nanos) = crate::time::get_real_time_ns();
    i128::from(secs) * 1_000_000_000 + i128::from(nanos)
}

/// When a futex wait times out.
#[derive(Clone, Copy)]
enum Deadline {
    Never,
    /// CLOCK_MONOTONIC nanoseconds.
    Monotonic(u64),
    /// CLOCK_REALTIME nanoseconds: setting the clock moves it.
    Realtime(i128),
}

impl Deadline {
    /// The CLOCK_MONOTONIC time the scheduler wakes the waiter at, read now.
    fn wake_ns(&self) -> Option<u64> {
        match *self {
            Deadline::Never => None,
            Deadline::Monotonic(at) => Some(at),
            Deadline::Realtime(at) => {
                let now_real = realtime_ns();
                let now_mono = monotonic_ns();
                let left = (at - now_real).max(0);
                Some((i128::from(now_mono) + left).min(i128::from(u64::MAX)) as u64)
            }
        }
    }

    fn reached(&self) -> bool {
        match *self {
            Deadline::Never => false,
            Deadline::Monotonic(at) => monotonic_ns() >= at,
            Deadline::Realtime(at) => realtime_ns() >= at,
        }
    }
}

/// The key the calling thread's `uaddr` names. A word in a shared mapping is
/// keyed by its physical address unless the operation is private; the caller
/// has touched the word, so its page is present.
fn futex_key(uaddr: u64, private: bool) -> Option<FutexKey> {
    let thread_id = crate::task::scheduler::current_thread_id()?;
    let mut manager_guard = crate::process::manager();
    let manager = manager_guard.as_mut()?;
    let group = {
        let (pid, process) = manager.find_process_by_thread(thread_id)?;
        process.thread_group_id.unwrap_or(pid.as_u64())
    };
    if private {
        return Some((group, uaddr));
    }
    let physical = manager
        .find_address_space_by_thread_mut(thread_id)
        .and_then(|(_, owner)| {
            let addr = VirtAddr::new(uaddr);
            let shared = owner.vmas.iter().any(|vma| {
                vma.contains(addr) && vma.flags.contains(crate::memory::vma::MmapFlags::SHARED)
            });
            if !shared {
                return None;
            }
            owner.page_table.as_ref()?.translate(addr).map(|phys| phys.as_u64())
        });
    Some(physical.map_or((group, uaddr), |phys| (SHARED_FUTEX, phys)))
}

/// Read the futex word through the user-copy routine, faulting its page in.
/// EFAULT if it cannot be read: another thread of the address space may have
/// unmapped or protected the word since it was last touched, so this is also
/// the read taken under the futex table's lock, where a raw load would fault
/// in the kernel with the table held.
fn read_word(uaddr: u64) -> Result<u32, u64> {
    crate::syscall::userptr::copy_from_user::<u32>(uaddr as *const u32)
        .map_err(|_| super::errno::EFAULT as u64)
}

/// Read a timespec argument, EINVAL if it is not a valid one.
fn read_timespec(ptr: u64) -> Result<crate::syscall::time::Timespec, u64> {
    let timeout = crate::syscall::userptr::copy_from_user::<crate::syscall::time::Timespec>(
        ptr as *const crate::syscall::time::Timespec,
    )
    .map_err(|_| super::errno::EFAULT as u64)?;
    if timeout.tv_nsec < 0 || timeout.tv_nsec >= 1_000_000_000 || timeout.tv_sec < 0 {
        return Err(super::errno::EINVAL as u64);
    }
    Ok(timeout)
}

/// sys_futex - futex system call.
pub fn sys_futex(
    uaddr: u64,
    op: u32,
    val: u32,
    timeout: u64,
    uaddr2: u64,
    val3: u32,
) -> SyscallResult {
    let cmd = op & FUTEX_CMD_MASK;
    let private = op & FUTEX_PRIVATE_FLAG != 0;
    // Only FUTEX_WAIT_BITSET takes a CLOCK_REALTIME deadline (Linux).
    if op & FUTEX_CLOCK_REALTIME != 0 && cmd != FUTEX_WAIT_BITSET {
        return SyscallResult::Err(super::errno::ENOSYS as u64);
    }

    match cmd {
        FUTEX_WAIT => futex_wait(uaddr, val, timeout, val3, private),
        FUTEX_WAIT_BITSET => {
            if val3 == 0 {
                return SyscallResult::Err(super::errno::EINVAL as u64);
            }
            let deadline = if timeout == 0 {
                Deadline::Never
            } else {
                match read_timespec(timeout) {
                    Ok(ts) => {
                        let at = i128::from(ts.tv_sec) * 1_000_000_000 + i128::from(ts.tv_nsec);
                        if op & FUTEX_CLOCK_REALTIME != 0 {
                            Deadline::Realtime(at)
                        } else {
                            Deadline::Monotonic(at.min(i128::from(u64::MAX)) as u64)
                        }
                    }
                    Err(e) => return SyscallResult::Err(e),
                }
            };
            futex_wait_until(uaddr, val, deadline, val3, private)
        }
        FUTEX_WAKE => futex_wake(uaddr, val, val3, private, FUTEX_BITSET_MATCH_ANY),
        FUTEX_WAKE_BITSET => {
            if val3 == 0 {
                return SyscallResult::Err(super::errno::EINVAL as u64);
            }
            futex_wake(uaddr, val, 0, private, val3)
        }
        // The second count, nr_requeue, travels in the timeout argument.
        FUTEX_REQUEUE => futex_requeue(uaddr, val, timeout as u32, uaddr2, None, private),
        FUTEX_CMP_REQUEUE => futex_requeue(uaddr, val, timeout as u32, uaddr2, Some(val3), private),
        _ => SyscallResult::Err(super::errno::ENOSYS as u64),
    }
}

/// FUTEX_WAIT_BITSET: wait on `uaddr` while it holds `expected_val`, until a
/// wake whose bitset meets `bitset`, a signal, or the absolute `deadline`.
fn futex_wait_until(
    uaddr: u64,
    expected_val: u32,
    deadline: Deadline,
    bitset: u32,
    private: bool,
) -> SyscallResult {
    if uaddr == 0 || uaddr % 4 != 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    if let Err(e) = read_word(uaddr) {
        return SyscallResult::Err(e);
    }
    let Some(thread_id) = crate::task::scheduler::current_thread_id() else {
        return SyscallResult::Err(super::errno::ESRCH as u64);
    };
    let Some(key) = futex_key(uaddr, private) else {
        return SyscallResult::Err(super::errno::ESRCH as u64);
    };
    let realtime = matches!(deadline, Deadline::Realtime(_));

    let prepared = with_table(|table| {
        let Ok(current) = read_word(uaddr) else {
            return Prepared::Fault;
        };
        if current != expected_val {
            return Prepared::Mismatch;
        }
        if deadline.reached() {
            return Prepared::Expired;
        }
        table.enqueue(key, Waiter { tid: thread_id, bitset });
        let published = crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                thread.realtime_sleep = realtime;
            }
            sched.block_current_for_io_with_timeout(deadline.wake_ns())
        })
        .unwrap_or(false);
        if published {
            Prepared::Queued
        } else {
            table.remove(thread_id);
            Prepared::PublishFailed
        }
    });
    match prepared {
        Prepared::Fault => return SyscallResult::Err(super::errno::EFAULT as u64),
        Prepared::Mismatch => return SyscallResult::Err(super::errno::EAGAIN as u64),
        Prepared::Expired => return SyscallResult::Err(super::errno::ETIMEDOUT as u64),
        Prepared::PublishFailed => {
            finish_wait(thread_id);
            return SyscallResult::Err(super::errno::ESRCH as u64);
        }
        Prepared::Queued => {}
    }

    // Preemption stays disabled until the signal check below has run (#1230):
    // see `blocking_io::wait_prepared`.
    let result = loop {
        if crate::syscall::check_signals_for_eintr().is_some() {
            break if with_table(|table| table.remove(thread_id)) {
                SyscallResult::Err(super::errno::EINTR as u64)
            } else {
                SyscallResult::Ok(0)
            };
        }
        let still_waiting = crate::task::scheduler::with_scheduler(|sched| {
            sched.wake_expired_timers();
            sched
                .current_thread_mut()
                .is_some_and(|thread| thread.state == ThreadState::BlockedOnIO)
        })
        .unwrap_or(false);
        if !still_waiting {
            // Woken: by a waker, which took this thread off its queue, by the
            // deadline, or by CLOCK_REALTIME being set.
            let outcome = with_table(|table| {
                if !table.queued_on.contains_key(&thread_id) {
                    return Some(SyscallResult::Ok(0));
                }
                if deadline.reached() {
                    table.remove(thread_id);
                    return Some(SyscallResult::Err(super::errno::ETIMEDOUT as u64));
                }
                // Not yet: wait again, to the deadline as it now stands.
                let published = crate::task::scheduler::with_scheduler(|sched| {
                    sched.block_current_for_io_with_timeout(deadline.wake_ns())
                })
                .unwrap_or(false);
                if published {
                    None
                } else {
                    table.remove(thread_id);
                    Some(SyscallResult::Err(super::errno::EINTR as u64))
                }
            });
            if let Some(result) = outcome {
                break result;
            }
        }
        crate::per_cpu::preempt_enable();
        crate::task::scheduler::yield_current();
        Cpu::halt_with_interrupts();
        crate::per_cpu::preempt_disable();
    };
    finish_wait(thread_id);
    result
}

/// Leave a futex wait: the thread runs on in its syscall.
fn finish_wait(thread_id: u64) {
    crate::task::scheduler::with_thread_mut(thread_id, |thread| {
        if thread.state == ThreadState::BlockedOnIO {
            thread.set_ready();
        }
        thread.wake_time_ns = None;
        thread.realtime_sleep = false;
        thread.blocked_in_syscall = false;
    });
    #[cfg(target_arch = "aarch64")]
    ensure_current_address_space();
}

/// FUTEX_REQUEUE and FUTEX_CMP_REQUEUE: wake up to `nr_wake` waiters on
/// `uaddr` and move up to `nr_requeue` of the rest to `uaddr2`. With
/// `expected`, only while `uaddr` holds it (EAGAIN otherwise). Returns how
/// many were woken or moved.
fn futex_requeue(
    uaddr: u64,
    nr_wake: u32,
    nr_requeue: u32,
    uaddr2: u64,
    expected: Option<u32>,
    private: bool,
) -> SyscallResult {
    if uaddr == 0 || uaddr % 4 != 0 || uaddr2 == 0 || uaddr2 % 4 != 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    if (nr_wake as i32) < 0 || (nr_requeue as i32) < 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    if let Err(e) = read_word(uaddr).and_then(|_| read_word(uaddr2)) {
        return SyscallResult::Err(e);
    }
    let (Some(from), Some(to)) = (futex_key(uaddr, private), futex_key(uaddr2, private)) else {
        return SyscallResult::Err(super::errno::ESRCH as u64);
    };
    with_table(|table| {
        if let Some(expected) = expected {
            match read_word(uaddr) {
                Ok(current) if current == expected => {}
                Ok(_) => return SyscallResult::Err(super::errno::EAGAIN as u64),
                Err(e) => return SyscallResult::Err(e),
            }
        }
        let woken = table.wake(from, nr_wake, FUTEX_BITSET_MATCH_ANY);
        let moved = table.requeue(from, to, nr_requeue);
        SyscallResult::Ok(u64::from(woken) + u64::from(moved))
    })
}

/// FUTEX_WAIT: atomically check *uaddr == expected_val and enqueue the
/// current thread if it matches.
fn futex_wait(
    uaddr: u64,
    expected_val: u32,
    timeout_ptr: u64,
    _val3: u32,
    private: bool,
) -> SyscallResult {
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

    // #627: the oracle's stage3_elapsed_ok bit must be anchored to the same
    // clock read the deadline below is computed from, not to a later read
    // taken after process-manager lookups. Captured here, unconditionally,
    // so the boot_tests build can hand it to the oracle as its measurement
    // origin (see the record_arm call site below).
    #[cfg(feature = "boot_tests")]
    let mut deadline_base_ns: Option<u64> = None;

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
        #[cfg(feature = "boot_tests")]
        {
            deadline_base_ns = Some(now_ns);
        }
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
    // manager guard is fully released by futex_key's return.
    let key = match futex_key(uaddr, private) {
        Some(key) => key,
        None => return SyscallResult::Err(super::errno::ESRCH as u64),
    };
    #[cfg(feature = "boot_tests")]
    let tg_id = key.0;

    #[cfg(feature = "boot_tests")]
    let oracle_stage = crate::syscall::futex_oracle::arm_from_val3(_val3);
    #[cfg(feature = "boot_tests")]
    let oracle_deadline = oracle_stage.map(|stage| {
        // Anchor the oracle's elapsed measurement to deadline_base_ns (the
        // same read the deadline above used), not to a fresh read taken here
        // -- that later origin was #627 (this call site sat strictly after
        // the process-manager lookup above, understating elapsed by exactly
        // that lookup's cost). No timeout means no deadline to anchor to, so
        // record_arm falls back to reading the clock itself -- the pre-#627
        // fallback shape, kept only for that untimed case.
        // claim-lint:ok: #627 -- the fallback path is unreachable for every
        // oracle stage on this branch (stage1/2/3 all pass a nonzero timeout
        // in userspace/programs/src/futex_handoff_oracle.rs), kept only as a
        // defensive default; see validate_futex_oracle_record_arm_anchor in
        // tests/teardown_structure.rs.
        let base_ns = deadline_base_ns.unwrap_or_else(|| {
            let (seconds, nanos) = crate::time::get_monotonic_time_ns();
            seconds as u64 * 1_000_000_000 + nanos as u64
        });
        crate::syscall::futex_oracle::record_arm(stage, tg_id, uaddr, base_ns)
    });

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
        read_word(uaddr).is_ok_and(|current_val| current_val == expected_val)
    };
    #[cfg(feature = "coreproof_mut_futex_section")]
    let split_precheck = value_matches && !zero_timeout;

    // STAGE 1's seam. It must remain immediately before the section that
    // ENQUEUES and PUBLISHES the waiter, with the map lock free, so the wake it
    // drives is one a correct implementation cannot lose: #584 is lost exactly
    // when a wake lands after the value check and before the publication.
    //
    // In a correct build there is nothing between the seam and the check — they
    // are one section — so the drive changes the value, the in-lock check sees
    // the change, and the wait returns EAGAIN having never enqueued. Split that
    // section and the same seam lands INSIDE the split: the check has already
    // decided on a stale value, the wake finds nothing queued, and the waiter
    // publishes itself into a queue no wake will visit again. The oracle then
    // reports the loss through its own backstop as `stage1_ret=RESCUED`,
    // `stage1_parked=1` and `rescues=1`.
    //
    // The seam moved here from above the value check in round 3 of the
    // core-proof pilot. In a build without the split the two positions are the
    // same position — only a `let` binding and compiled-out code sit between
    // them — so this is a no-op for every shipping profile and a real race for
    // the one that reopens the window.
    #[cfg(feature = "boot_tests")]
    if oracle_stage == Some(crate::syscall::futex_oracle::Stage::S1) {
        crate::syscall::futex_oracle::stage1_drive(tg_id, uaddr, expected_val);
    }

    let prepare_outcome = with_table(|table| {
        let proceed = {
            #[cfg(feature = "coreproof_mut_futex_section")]
            {
                split_precheck
            }
            #[cfg(not(feature = "coreproof_mut_futex_section"))]
            {
                // A word unmapped or protected since the touch above is
                // EFAULT, not a kernel fault with the table held.
                let Ok(current_val) = read_word(uaddr) else {
                    return Prepared::Fault;
                };
                value_matches = current_val == expected_val;
                value_matches && !zero_timeout
            }
        };
        if !proceed {
            return Prepared::Mismatch;
        }
        table.enqueue(
            key,
            Waiter {
                tid: thread_id,
                bitset: FUTEX_BITSET_MATCH_ANY,
            },
        );
        let published = crate::task::scheduler::with_scheduler(|sched| {
            sched.block_current_for_io_with_timeout(effective_wake_time_ns)
        })
        .unwrap_or(false);
        if published {
            Prepared::Queued
        } else {
            table.remove(thread_id);
            Prepared::PublishFailed
        }
    });

    match prepare_outcome {
        // Not produced here: FUTEX_WAIT's deadline is relative, and a zero
        // timeout is a Mismatch with the value matching.
        Prepared::Expired => SyscallResult::Err(super::errno::ETIMEDOUT as u64),
        Prepared::Fault => {
            #[cfg(feature = "boot_tests")]
            oracle_finish(
                oracle_stage,
                false,
                crate::syscall::futex_oracle::OracleRet::Other,
            );
            SyscallResult::Err(super::errno::EFAULT as u64)
        }
        Prepared::Mismatch => {
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
        Prepared::PublishFailed => {
            #[cfg(feature = "boot_tests")]
            oracle_finish(
                oracle_stage,
                false,
                crate::syscall::futex_oracle::OracleRet::Esrch,
            );
            SyscallResult::Err(super::errno::ESRCH as u64)
        }
        Prepared::Queued => {
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

            // Preemption stays disabled until the signal check below has run (#1230):
            // see `blocking_io::wait_prepared`.

            #[cfg(feature = "boot_tests")]
            let mut oracle_parked = false;
            let mut signal_pending = false;
            // What the timer heap's pops of this wait saw, carried out of the
            // loop for the record below (#608 F4). It is a plain field read
            // taken inside the scheduler access this loop already performs.
            let mut timer_pop: Option<crate::task::thread::TimerPopRecord> = None;
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
                                    thread.timer_pop,
                                )
                            })
                            .unwrap_or((false, None))
                    })
                    .unwrap_or((false, None));

                if pop_observation.is_some() {
                    timer_pop = pop_observation;
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

                crate::per_cpu::preempt_enable();
                crate::task::scheduler::yield_current();
                Cpu::halt_with_interrupts();
                crate::per_cpu::preempt_disable();
            }

            let removed_by_me = with_table(|table| table.remove(thread_id));
            finish_wait(thread_id);

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
                            timer_pop,
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

/// FUTEX_WAKE and FUTEX_WAKE_BITSET: wake up to `max_wake` threads waiting on
/// the futex at `uaddr` whose wait's bitset meets `bitset`.
fn futex_wake(uaddr: u64, max_wake: u32, _val3: u32, private: bool, bitset: u32) -> SyscallResult {
    #[cfg(feature = "boot_tests")]
    if bitset == FUTEX_BITSET_MATCH_ANY && crate::syscall::futex_oracle::is_report(_val3) {
        crate::syscall::futex_oracle::report();
        return SyscallResult::Ok(0);
    }

    if uaddr == 0 || uaddr % 4 != 0 {
        return SyscallResult::Err(super::errno::EINVAL as u64);
    }
    // A shared futex is keyed by its page, which must be present.
    if !private {
        if let Err(e) = read_word(uaddr) {
            return SyscallResult::Err(e);
        }
    }
    let Some(key) = futex_key(uaddr, private) else {
        return SyscallResult::Err(super::errno::ESRCH as u64);
    };
    SyscallResult::Ok(u64::from(with_table(|table| table.wake(key, max_wake, bitset))))
}

/// Perform a FUTEX_WAKE on a specific address for a specific thread group.
pub fn futex_wake_for_thread_group(tg_id: u64, uaddr: u64, max_wake: u32) -> u32 {
    with_table(|table| table.wake((tg_id, uaddr), max_wake, FUTEX_BITSET_MATCH_ANY))
}

/// The calling thread is exiting and has cleared its CLONE_CHILD_CLEARTID word
/// at `uaddr`: wake every waiter on it, as a FUTEX_WAKE that is not private
/// would (a word in a shared mapping is keyed by its page). `tg_id` keys the
/// word if it cannot be resolved.
pub fn futex_wake_cleared_tid(tg_id: u64, uaddr: u64) {
    let key = futex_key(uaddr, false).unwrap_or((tg_id, uaddr));
    with_table(|table| table.wake(key, u32::MAX, FUTEX_BITSET_MATCH_ANY));
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
            // A thread's own table, or the one its CLONE_VM group shares.
            let ttbr0_value = process.cr3_value();
            if let Some(ttbr0_value) = ttbr0_value {
                crate::arch_impl::aarch64::ttbr0::restore_process_ttbr0(ttbr0_value);
            }
        }
    }
}

#[cfg(feature = "boot_tests")]
pub(crate) fn oracle_queue_residual(keys: [FutexKey; 3]) -> u64 {
    with_table(|table| keys.iter().map(|key| table.count(key) as u64).sum())
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
