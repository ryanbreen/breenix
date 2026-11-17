//! read(2) on a regular file through a bounded kernel buffer (#3040).
//!
//! The file's bytes pass through one kernel buffer of at most `cap` bytes,
//! filled from the backend and copied to the caller in a loop, so the kernel
//! allocation a read needs is bounded by `cap` rather than by the request.
//! Host checks: `tools/regular-read-host-tests`.

use alloc::vec::Vec;

/// The kernel buffer read(2) copies a regular file through.
pub const CHUNK: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Failure<E> {
    /// The bounded buffer itself could not be allocated (ENOMEM).
    NoMemory,
    /// The backend failed before any byte reached the caller.
    Backend(E),
    /// The caller's buffer could not be written (EFAULT). The caller leaves
    /// the file offset unchanged, as for a single failed copy.
    Fault,
}

/// Reads up to `length` bytes of a file starting at `position`.
///
/// `fill(offset, buf)` reads file bytes at `offset` into `buf` and returns how
/// many it read; fewer than `buf.len()` is end of file (or the backend's own
/// short read) and ends the call. `deliver(at, bytes)` copies `bytes` to the
/// caller's buffer `at` bytes from its start; it is not called for an empty
/// piece. The backend is asked at least once, also for `length == 0`.
///
/// Returns the bytes delivered. A backend error after some bytes were
/// delivered ends the call with that count; the next read reports the error
/// at the offset where it occurs.
pub fn read<E>(
    position: u64,
    length: usize,
    cap: usize,
    mut fill: impl FnMut(u64, &mut [u8]) -> Result<usize, E>,
    mut deliver: impl FnMut(usize, &[u8]) -> Result<(), ()>,
) -> Result<usize, Failure<E>> {
    let size = length.min(cap);
    let mut buffer = Vec::new();
    if buffer.try_reserve_exact(size).is_err() {
        return Err(Failure::NoMemory);
    }
    buffer.resize(size, 0);
    let mut done = 0usize;
    loop {
        let want = (length - done).min(cap);
        let read = match fill(position + done as u64, &mut buffer[..want]) {
            Ok(read) => read.min(want),
            Err(error) if done == 0 => return Err(Failure::Backend(error)),
            Err(_) => return Ok(done),
        };
        if read > 0 {
            deliver(done, &buffer[..read]).map_err(|()| Failure::Fault)?;
        }
        done += read;
        if read < want || done == length {
            return Ok(done);
        }
    }
}
