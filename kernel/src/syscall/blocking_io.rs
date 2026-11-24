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
        PrepareOutcome::OutOfMemory => return Err(errno::ENOMEM),
        PrepareOutcome::Queued => {}
    }

    // Keep the queued thread's identity through the wait and cleanup.
    let tid =
        crate::task::scheduler::current_thread_id().expect("queued wait requires a current thread");
    // The wait returns with the preemption level its caller entered with
    // (E24 ledger 703; tests/canvas_wait_test.py). A syscall caller holds its
    // entry level through the signal check and releases it for the halt only.
    // A kernel thread can wait already preemptible, at level 0: releasing a
    // level it did not take would underflow, and restoring one afterwards would
    // leave it held, so a level is released and restored only when the caller
    // holds one.
    let release_preempt =
        crate::per_cpu::preempt_count() & crate::arch_impl::current::constants::PREEMPT_MASK != 0;
    // A thread switched out in its published blocked state runs again only
    // when something wakes it, and a signal that was already pending wakes
    // nothing: its wake may have ended an earlier sleep of this wait. Preempted
    // before the check, the thread would sleep with the signal pending (#1230).
    // Once the check has run, a new signal finds the thread blocked and wakes
    // it. Every syscall wait loop that publishes a blocked state follows this
    // order. claim-lint:ok: #1230 records the lost-signal ordering.
    let interrupted = loop {
        if let Some(error) = crate::syscall::check_signals_for_restartable_wait() {
            break Some(error);
        }
        let waiting = crate::task::scheduler::with_current(|sched| {
            sched
                .current_thread_mut()
                .map(|thread| thread.state == ThreadState::BlockedOnIO)
                .unwrap_or(false)
        })
        .unwrap_or(false);
        if !waiting {
            break None;
        }
        if release_preempt {
            crate::per_cpu::preempt_enable();
        }
        crate::task::scheduler::yield_current();
        crate::arch_halt_while_blocked();
        if release_preempt {
            crate::per_cpu::preempt_disable();
        }
    };

    queue.take_waiter(tid);
    queue.finish_wait_for(tid);
    match interrupted {
        Some(error) => Err(error),
        None => Ok(()),
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
                    // SIGPIPE accompanies EPIPE even after partial progress.
                    drop(pipe);
                    super::signal::raise_sigpipe();
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

/// Generic Console/Tty reads use the live input ring and descriptor status flags.
/// Once `live` turns false (the terminal was revoked from the descriptor), the
/// read takes no more input and returns end of file.
pub(crate) fn read_console(buf: &mut [u8], is_nonblocking: bool, live: &dyn Fn() -> bool) -> Result<usize, i32> {
    loop {
        let outcome = match crate::ipc::stdin::read_or_prepare(buf, is_nonblocking, live) {
            Ok(n) => return Ok(n),
            Err(outcome) => outcome,
        };
        if is_nonblocking {
            return Err(errno::EAGAIN);
        }
        wait_prepared(&crate::ipc::stdin::INPUT_READERS, outcome)?;
    }
}

/// Reads of a virtual terminal (`/dev/ttyN`, `/dev/tty0` at the time of the
/// read): its own input queue, as `read_console`; tty1's is the console's.
pub(crate) fn read_terminal(
    device: crate::fs::devfs::DeviceType,
    buf: &mut [u8],
    is_nonblocking: bool,
    live: &dyn Fn() -> bool,
) -> Result<usize, i32> {
    let input = match device.terminal() {
        Some(1) => return read_console(buf, is_nonblocking, live),
        Some(number) => crate::ipc::stdin::vt_input(number - 1).ok_or(errno::ENXIO)?,
        None => return Err(errno::ENXIO),
    };
    loop {
        let outcome = match input.read_or_prepare(buf, is_nonblocking, live) {
            Ok(n) => return Ok(n),
            Err(outcome) => outcome,
        };
        if is_nonblocking {
            return Err(errno::EAGAIN);
        }
        wait_prepared(&input.readers, outcome)?;
    }
}

/// Complete a checked publication for a short exclusion wait that only a
/// fatal signal ends early: exec and mmap waiting for a file's binding gate
/// (`MapState::take_binding_gate`), which must not fail for a handled signal.
/// The sleep is the wait queue's own `schedule_current_wait`, which releases
/// and restores the system call's preemption level. A wake for a handled
/// signal returns `Ok`, and the caller retries before sleeping again. A
/// caller holding no preemption level only retries.
pub(crate) fn wait_prepared_killable(queue: &WaitQueueHead, outcome: PrepareOutcome) -> Result<(), i32> {
    match outcome {
        PrepareOutcome::Mismatch => {
            queue.finish_wait();
            return Ok(());
        }
        PrepareOutcome::PublishFailed => {
            queue.finish_wait();
            return Err(if crate::syscall::fatal_signal_pending() { errno::EINTR } else { errno::ESRCH });
        }
        PrepareOutcome::OutOfMemory => return Err(errno::ENOMEM),
        PrepareOutcome::Queued => {}
    }
    let syscall_level =
        crate::per_cpu::preempt_count() & crate::arch_impl::current::constants::PREEMPT_MASK != 0;
    if syscall_level && !crate::syscall::fatal_signal_pending() {
        crate::task::waitqueue::schedule_current_wait();
    }
    queue.finish_wait();
    if crate::syscall::fatal_signal_pending() {
        return Err(errno::EINTR);
    }
    Ok(())
}
