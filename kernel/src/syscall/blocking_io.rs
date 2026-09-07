//! Interruptible prepared waits shared by blocking descriptor adapters.
//!
//! Publication happens under the caller's object lock. This module's sleep
//! lifecycle receives the queue and publication outcome rather than a guard.

use super::{errno, SyscallResult};
use crate::ipc::pipe::{PipeBuffer, WriteAttempt, PIPE_BUF};
use crate::task::thread::ThreadState;
use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
use alloc::sync::Arc;
use spin::Mutex;

/// Complete a checked publication after the caller has released object/PM locks.
/// Mismatch retries, publication failure is an internal thread error, and the
/// queued exit normalizes the scheduler's blocked-in-syscall flag.
pub(crate) fn wait_prepared(queue: &WaitQueueHead, outcome: PrepareOutcome) -> Result<(), i32> {
    match outcome {
        PrepareOutcome::Mismatch => {
            queue.finish_wait();
            return Ok(());
        }
        PrepareOutcome::PublishFailed => {
            queue.finish_wait();
            return Err(errno::ESRCH);
        }
        PrepareOutcome::Queued => {}
    }

    crate::per_cpu::preempt_enable();
    let interrupted = loop {
        if crate::syscall::check_signals_for_eintr().is_some() {
            break true;
        }
        let waiting = crate::task::scheduler::with_scheduler(|sched| {
            sched
                .current_thread_mut()
                .map(|thread| thread.state == ThreadState::BlockedOnIO)
                .unwrap_or(false)
        })
        .unwrap_or(false);
        if !waiting {
            break false;
        }
        crate::task::scheduler::yield_current();
        crate::arch_halt_with_interrupts();
    };
    crate::per_cpu::preempt_disable();

    if let Some(tid) = crate::task::scheduler::current_thread_id() {
        queue.take_waiter(tid);
    }
    queue.finish_wait();
    if interrupted {
        Err(errno::EINTR)
    } else {
        Ok(())
    }
}

/// PM-free adapter for an owned pipe/FIFO snapshot and its snapshotted mode.
/// Small writes stay atomic; large blocking writes retain their copied offset.
pub(crate) fn write_pipe(
    pipe_buffer: &Arc<Mutex<PipeBuffer>>,
    data: &[u8],
    is_nonblocking: bool,
) -> SyscallResult {
    let atomic_request = data.len() <= PIPE_BUF;
    let mut offset = 0;
    loop {
        let (queue, outcome) = {
            let mut pipe = pipe_buffer.lock();
            match pipe.try_write(&data[offset..], atomic_request) {
                Ok(written) => {
                    offset += written;
                    if is_nonblocking || offset == data.len() {
                        return SyscallResult::Ok(offset as u64);
                    }
                    continue;
                }
                Err(WriteAttempt::BrokenPipe) => {
                    return progress_or_error(offset, errno::EPIPE);
                }
                Err(WriteAttempt::WouldBlock) => {
                    if is_nonblocking {
                        return progress_or_error(offset, errno::EAGAIN);
                    }
                }
            }
            let required = if atomic_request { data.len() } else { 1 };
            let queue = pipe.write_waiters.clone();
            let outcome = queue.prepare_to_wait_checked(ThreadState::BlockedOnIO, None, || {
                pipe.has_readers() && !pipe.has_write_space(required)
            });
            (queue, outcome)
        };
        // The pipe guard is gone before preemption, signals, or sleeping.
        if let Err(error) = wait_prepared(&queue, outcome) {
            return progress_or_error(offset, error);
        }
    }
}

fn progress_or_error(offset: usize, error: i32) -> SyscallResult {
    if offset > 0 {
        SyscallResult::Ok(offset as u64)
    } else {
        SyscallResult::Err(error as u64)
    }
}
