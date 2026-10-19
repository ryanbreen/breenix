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
/// is initialized, EFAULT if no byte could be written.
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
        crate::random::wait_ready();
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
