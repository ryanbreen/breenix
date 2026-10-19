//! getrandom syscall implementation
//!
//! Draws from the kernel pool in `crate::random`, which keeps its state across
//! calls and CPUs and is seeded at boot and by device events.

use super::{ErrorCode, SyscallResult};

/// Return without waiting when the pool is not initialized (EAGAIN).
const GRND_NONBLOCK: u32 = 0x0001;
/// Draw from the "random" source. It is the same pool, and like the default
/// source it waits until the pool is initialized.
const GRND_RANDOM: u32 = 0x0002;
/// Return at once, even before the pool is initialized.
const GRND_INSECURE: u32 = 0x0004;

/// Most bytes one call returns, as getrandom(2) documents for the urandom source.
const MAX_BYTES: u64 = 33_554_431;

/// sys_getrandom - fill a userspace buffer with random bytes
///
/// Arguments:
///   buf_ptr: userspace buffer address
///   buflen:  number of bytes to fill
///   flags:   GRND_NONBLOCK, GRND_RANDOM, GRND_INSECURE
///
/// Returns: number of bytes written (at most `MAX_BYTES`; fewer if the buffer
/// faults part way through), or negative errno: EINVAL for unknown flags or
/// GRND_INSECURE with GRND_RANDOM, EAGAIN with GRND_NONBLOCK before the pool
/// is initialized, EINTR if a signal interrupts initialization, EFAULT if no
/// byte could be written.
pub fn sys_getrandom(buf_ptr: u64, buflen: u64, flags: u32) -> SyscallResult {
    if flags & !(GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE) != 0
        || flags & (GRND_INSECURE | GRND_RANDOM) == (GRND_INSECURE | GRND_RANDOM)
    {
        return SyscallResult::Err(ErrorCode::InvalidArgument as u64);
    }
    if flags & GRND_INSECURE == 0 && !crate::random::is_ready() {
        if flags & GRND_NONBLOCK != 0 {
            return SyscallResult::Err(ErrorCode::TryAgain as u64);
        }
        if let Err(error) = wait_ready() {
            return SyscallResult::Err(error as u64);
        }
    }
    if buflen == 0 {
        return SyscallResult::Ok(0);
    }

    let len = buflen.min(MAX_BYTES) as usize;
    let mut chunk = [0u8; crate::random::CHUNK];
    let mut written = 0usize;
    while written < len {
        let take = chunk.len().min(len - written);
        // fill_chunk releases the pool's lock before this copy, which may fault.
        crate::random::fill_chunk(&mut chunk[..take], written == 0);
        let copied = crate::syscall::userptr::write_user_prefix(
            buf_ptr.wrapping_add(written as u64),
            chunk.as_ptr(),
            take,
        );
        written += copied;
        if copied < take {
            break;
        }
    }
    crate::random::erase::wipe(&mut chunk);

    if written == 0 {
        SyscallResult::Err(ErrorCode::Fault as u64)
    } else {
        SyscallResult::Ok(written as u64)
    }
}

/// Sleep between bounded hardware retries. Timing and device input cannot
/// establish readiness, so a machine without a CPU RNG may wait indefinitely,
/// but it must leave the CPU available and let caught/fatal signals end the call.
fn wait_ready() -> Result<(), i32> {
    use crate::task::thread::ThreadState;
    use crate::task::waitqueue::{PrepareOutcome, WaitQueueHead};
    static WAITERS: WaitQueueHead = WaitQueueHead::new();

    while !crate::random::is_ready() {
        if let Some(error) = super::check_signals_for_wait() {
            return Err(error);
        }
        crate::random::reseed_hardware();
        let deadline = crate::signal::monotonic_nanos().saturating_add(10_000_000);
        let outcome =
            WAITERS.prepare_to_wait_checked(ThreadState::BlockedOnIO, Some(deadline), || {
                !crate::random::is_ready()
            });
        match outcome {
            PrepareOutcome::Mismatch => {
                WAITERS.finish_wait();
            }
            PrepareOutcome::PublishFailed => {
                WAITERS.finish_wait();
                return Err(super::errno::ESRCH);
            }
            PrepareOutcome::OutOfMemory => return Err(super::errno::ENOMEM),
            PrepareOutcome::Queued => {
                // Recheck after publication: a signal that arrived before it
                // already spent its wake and must not be slept through.
                let interrupted = super::check_signals_for_wait();
                if interrupted.is_none() {
                    crate::task::waitqueue::schedule_current_wait();
                }
                WAITERS.finish_wait();
                if let Some(error) = interrupted.or_else(super::check_signals_for_wait) {
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}
