//! Scheduler-integrated wait queues.
//!
//! This is Breenix's equivalent of Linux waitqueues: callers enqueue the
//! current thread, mark it blocked in the scheduler, re-check their condition,
//! then schedule if the condition is still false. Task-context wakers remove
//! queued TIDs and wake them inline; interrupt-context wakers queue work to
//! the waiter CPU's TTWU wake list and send a function-call IPI.

use alloc::collections::VecDeque;

use super::thread::ThreadState;

/// A single waiter recorded by thread ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waiter {
    tid: u64,
    target_cpu: usize,
}

impl Waiter {
    pub const fn new(tid: u64, target_cpu: usize) -> Self {
        Self { tid, target_cpu }
    }

    pub const fn tid(&self) -> u64 {
        self.tid
    }

    pub const fn target_cpu(&self) -> usize {
        self.target_cpu
    }
}

/// Wait queue head for event-driven scheduler waits.
///
/// Linux stores caller-owned intrusive wait entries. Breenix's scheduler wake
/// API is TID-based, so the first implementation stores duplicate-free TIDs
/// behind a small spin mutex. The public semantics match Linux's core pattern:
/// prepare, check condition, schedule, finish.
pub struct WaitQueueHead {
    waiters: spin::Mutex<VecDeque<Waiter>>,
}

impl WaitQueueHead {
    /// Create an empty waitqueue.
    pub const fn new() -> Self {
        Self {
            waiters: spin::Mutex::new(VecDeque::new()),
        }
    }

    /// Enqueue the current thread and publish its scheduler wait state.
    ///
    /// F32 initially supports `BlockedOnIO` because that is the state wired to
    /// the scheduler's I/O wake path. Unsupported states return `None` rather
    /// than duplicating scheduler policy.
    pub fn prepare_to_wait(&self, state: ThreadState) -> Option<u64> {
        if state != ThreadState::BlockedOnIO {
            return None;
        }

        let tid = crate::task::scheduler::current_thread_id()?;
        let target_cpu = current_cpu_id();

        let published = self.with_waiters(|waiters| {
            if !waiters.iter().any(|waiter| waiter.tid == tid) {
                waiters.push_back(Waiter::new(tid, target_cpu));
            }

            let published = crate::task::scheduler::with_scheduler(|sched| {
                sched.publish_current_io_wait_state()
            })
            .unwrap_or(false);

            if !published {
                waiters.retain(|waiter| waiter.tid != tid);
            }

            published
        });

        if published {
            Some(tid)
        } else {
            None
        }
    }

    /// Remove the current thread from the waitqueue and normalize syscall state.
    ///
    /// This mirrors Linux `finish_wait`: a thread that was prepared but never
    /// actually slept must become runnable before returning from the syscall.
    pub fn finish_wait(&self) {
        let Some(tid) = crate::task::scheduler::current_thread_id() else {
            return;
        };

        self.remove_waiter(tid);

        crate::task::scheduler::with_scheduler(|sched| {
            if let Some(thread) = sched.current_thread_mut() {
                if thread.state == ThreadState::BlockedOnIO {
                    thread.set_ready();
                    thread.wake_time_ns = None;
                }
                thread.blocked_in_syscall = false;
            }
        });
    }

    /// Wake all waiters.
    pub fn wake_up(&self) {
        self.with_waiters(|waiters| {
            while let Some(waiter) = waiters.pop_front() {
                wake_waiter(waiter);
            }
        });
    }

    /// Wake the first waiter, if any.
    pub fn wake_up_one(&self) {
        self.with_waiters(|waiters| {
            if let Some(waiter) = waiters.pop_front() {
                wake_waiter(waiter);
            }
        });
    }

    /// Return whether the queue currently has waiters.
    #[allow(dead_code)]
    pub fn has_waiters(&self) -> bool {
        self.with_waiters(|waiters| !waiters.is_empty())
    }

    fn remove_waiter(&self, tid: u64) {
        self.with_waiters(|waiters| {
            waiters.retain(|waiter| waiter.tid != tid);
        });
    }

    #[cfg(test)]
    fn drain_waiters(&self) -> VecDeque<Waiter> {
        self.with_waiters(|waiters| waiters.drain(..).collect::<VecDeque<_>>())
    }

    #[cfg(test)]
    fn pop_one_waiter(&self) -> Option<Waiter> {
        self.with_waiters(|waiters| waiters.pop_front())
    }

    fn with_waiters<R>(&self, f: impl FnOnce(&mut VecDeque<Waiter>) -> R) -> R {
        crate::arch_without_interrupts(|| {
            let mut waiters = self.waiters.lock();
            f(&mut waiters)
        })
    }

    #[cfg(test)]
    fn push_waiter_for_test(&self, tid: u64) {
        self.with_waiters(|waiters| {
            if !waiters.iter().any(|waiter| waiter.tid == tid) {
                waiters.push_back(Waiter::new(tid, 0));
            }
        });
    }

    #[cfg(test)]
    fn waiter_count_for_test(&self) -> usize {
        self.with_waiters(|waiters| waiters.len())
    }

    #[cfg(test)]
    fn contains_waiter_for_test(&self, tid: u64) -> bool {
        self.with_waiters(|waiters| waiters.iter().any(|waiter| waiter.tid == tid))
    }
}

fn wake_waiter(waiter: Waiter) {
    if in_interrupt_context() {
        crate::task::scheduler::ttwu_queue_wakelist_for_io(waiter.tid, waiter.target_cpu);
    } else {
        crate::task::scheduler::wake_waitqueue_thread(waiter.tid);
    }
}

fn current_cpu_id() -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        0
    }
}

fn in_interrupt_context() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        crate::per_cpu_aarch64::in_interrupt()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        crate::per_cpu::in_interrupt()
    }
}

/// Sleep the current prepared waiter until the scheduler wake path makes it
/// runnable again.
///
/// Syscall handlers enter with preemption disabled. Waiting must enable
/// preemption before scheduling and restore the syscall entry invariant before
/// returning to the caller.
pub fn schedule_current_wait() {
    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_enable();
    #[cfg(not(target_arch = "aarch64"))]
    crate::per_cpu::preempt_enable();

    loop {
        let still_waiting = crate::task::scheduler::with_scheduler(|sched| {
            sched
                .current_thread_mut()
                .map(|thread| thread.state == ThreadState::BlockedOnIO)
                .unwrap_or(false)
        })
        .unwrap_or(false);

        if !still_waiting {
            break;
        }

        #[cfg(target_arch = "aarch64")]
        crate::arch_impl::aarch64::context_switch::schedule_from_kernel();
        #[cfg(not(target_arch = "aarch64"))]
        crate::arch_halt_with_interrupts();
    }

    #[cfg(target_arch = "aarch64")]
    crate::per_cpu_aarch64::preempt_disable();
    #[cfg(not(target_arch = "aarch64"))]
    crate::per_cpu::preempt_disable();
}

#[cfg(test)]
mod tests {
    use super::WaitQueueHead;

    #[test]
    fn duplicate_waiters_are_ignored() {
        let waitq = WaitQueueHead::new();

        waitq.push_waiter_for_test(42);
        waitq.push_waiter_for_test(42);

        assert_eq!(waitq.waiter_count_for_test(), 1);
    }

    #[test]
    fn wake_up_one_removes_single_waiter() {
        let waitq = WaitQueueHead::new();

        waitq.push_waiter_for_test(1);
        waitq.push_waiter_for_test(2);
        let waiter = waitq.pop_one_waiter();

        assert_eq!(waiter.map(|waiter| waiter.tid()), Some(1));
        assert_eq!(waitq.waiter_count_for_test(), 1);
        assert!(!waitq.contains_waiter_for_test(1));
        assert!(waitq.contains_waiter_for_test(2));
    }

    #[test]
    fn wake_up_drains_waiters() {
        let waitq = WaitQueueHead::new();

        waitq.push_waiter_for_test(1);
        waitq.push_waiter_for_test(2);
        let waiters = waitq.drain_waiters();

        assert_eq!(waiters.len(), 2);
        assert_eq!(waitq.waiter_count_for_test(), 0);
    }
}
