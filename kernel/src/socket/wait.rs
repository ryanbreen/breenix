//! Blocking socket operations: one wait for every protocol.
//!
//! A protocol hands `wait_until` a `check` that takes its own locks and says
//! whether the operation can finish. Until it can, the thread sleeps on the
//! socket's wait queue until `SocketCommon::notify` moves the sequence number.
//! Every state change that can satisfy a check (data in, space freed, a peer's
//! FIN or RST, an error, a queued connection) calls `notify` after releasing
//! its protocol lock, so the queue lock is never held while a protocol lock is
//! taken.

use super::object::Socket;
use crate::syscall::errno::{EAGAIN, ENOMEM, ESRCH};
use crate::task::thread::ThreadState;
use crate::task::waitqueue::PrepareOutcome;

/// Monotonic time in nanoseconds, the clock the scheduler's timed waits use.
pub fn now_ns() -> u64 {
    let (seconds, nanos) = crate::time::get_monotonic_time_ns();
    (seconds as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nanos as u64)
}

/// Run `check` (it takes protocol locks itself and returns Some when
/// finished). Otherwise sleep on sk.common.wq until seq moves. nonblock, or a
/// passed deadline -> EAGAIN. A signal -> EINTR when a deadline is set, else
/// the restartable error from check_signals_for_restartable_wait().
///
/// `check` runs once more after the blocked state is published. TCP and Unix
/// streams still wake their readers by thread id rather than through
/// `notify`; that wake can land between the first check and the publication,
/// and the second check catches it.
pub fn wait_until<R>(
    sk: &Socket,
    nonblock: bool,
    deadline_ns: Option<u64>,
    mut check: impl FnMut() -> Option<Result<R, i32>>,
) -> Result<R, i32> {
    let queue = &sk.common.wq;
    loop {
        let seq = sk.common.seq();
        if let Some(result) = check() {
            return result;
        }
        if nonblock {
            return Err(EAGAIN);
        }
        if deadline_ns.is_some_and(|deadline| now_ns() >= deadline) {
            return Err(EAGAIN);
        }

        let outcome = queue.prepare_to_wait_checked(ThreadState::BlockedOnIO, deadline_ns, || {
            sk.common.seq() == seq
        });
        match outcome {
            PrepareOutcome::Mismatch => {
                queue.finish_wait();
                continue;
            }
            PrepareOutcome::PublishFailed => {
                queue.finish_wait();
                return Err(ESRCH);
            }
            PrepareOutcome::OutOfMemory => return Err(ENOMEM),
            PrepareOutcome::Queued => {}
        }

        let tid = crate::task::scheduler::current_thread_id()
            .expect("queued wait requires a current thread");
        // A socket whose last reference this check drops is released later,
        // not here, with the blocked state published.
        let second = {
            let _defer = super::object::DeferRelease::enter();
            check()
        };
        if let Some(result) = second {
            queue.take_waiter(tid);
            queue.finish_wait_for(tid);
            return result;
        }

        // Preemption stays disabled from the publication through the signal
        // check and is enabled for the halt only (#1230): see
        // `syscall::blocking_io::wait_prepared`.
        let interrupted = loop {
            let signal = if deadline_ns.is_some() {
                crate::syscall::check_signals_for_eintr()
            } else {
                crate::syscall::check_signals_for_restartable_wait()
            };
            if let Some(error) = signal {
                break Some(error);
            }
            let waiting = crate::task::scheduler::with_scheduler(|sched| {
                sched
                    .current_thread_mut()
                    .map(|thread| thread.state == ThreadState::BlockedOnIO)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
            // #772: a wasted turn (still blocked, sleep again) against one
            // that consumed a wake.
            if waiting {
                crate::trace_count!(
                    crate::tracing::providers::counters::RECV_WAIT_STILL_BLOCKED_TRUE
                );
            } else {
                crate::trace_count!(
                    crate::tracing::providers::counters::RECV_WAIT_STILL_BLOCKED_FALSE
                );
                break None;
            }
            crate::per_cpu::preempt_enable();
            crate::task::scheduler::yield_current();
            crate::arch_halt_while_blocked();
            crate::per_cpu::preempt_disable();
        };

        queue.take_waiter(tid);
        queue.finish_wait_for(tid);
        if let Some(error) = interrupted {
            return Err(error);
        }
    }
}
