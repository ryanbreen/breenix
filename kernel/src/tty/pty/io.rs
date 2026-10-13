//! read(2) and write(2) on pseudo-terminal descriptors.

use super::PtyPair;
use crate::syscall::blocking_io::wait_prepared;
use crate::syscall::errno::{EAGAIN, EFAULT, EIO, ENOMEM};
use crate::syscall::SyscallResult;
use crate::task::thread::ThreadState;
use crate::tty::ctty::TtyRef;
use crate::tty::jobctl::{self, JobOp};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// A kernel buffer for a read of `count` bytes.
pub(crate) fn read_buffer(count: u64) -> Result<Vec<u8>, SyscallResult> {
    let mut buf = Vec::new();
    if buf.try_reserve_exact(count as usize).is_err() {
        return Err(SyscallResult::Err(ENOMEM as u64));
    }
    buf.resize(count as usize, 0);
    Ok(buf)
}

/// Copy `n` bytes read into `buf` out to the caller's `buf_ptr`.
pub(crate) fn copy_out(buf_ptr: u64, buf: &[u8], n: usize) -> SyscallResult {
    if n > 0 && crate::syscall::handlers::copy_to_user(buf_ptr, buf.as_ptr() as u64, n).is_err() {
        return SyscallResult::Err(EFAULT as u64);
    }
    SyscallResult::Ok(n as u64)
}

/// read(2) on a master: the slave's output.
pub fn read_master(pair: &Arc<PtyPair>, buf_ptr: u64, count: u64, nonblocking: bool) -> SyscallResult {
    let mut buf = match read_buffer(count) {
        Ok(buf) => buf,
        Err(result) => return result,
    };
    loop {
        let outcome = {
            let mut ring = pair.slave_to_master.lock();
            let n = ring.read(&mut buf);
            if n > 0 {
                drop(ring);
                pair.slave_writers.wake_up();
                return copy_out(buf_ptr, &buf, n);
            }
            if pair.has_slave_hung_up() {
                return SyscallResult::Ok(0);
            }
            if nonblocking {
                return SyscallResult::Err(EAGAIN as u64);
            }
            pair.master_readers
                .prepare_to_wait_checked(ThreadState::BlockedOnIO, None, || {
                    ring.is_empty() && !pair.has_slave_hung_up()
                })
        };
        if let Err(error) = wait_prepared(&pair.master_readers, outcome) {
            return SyscallResult::Err(error as u64);
        }
    }
}

/// write(2) on a master: input to the slave, through the line discipline.
pub fn write_master(pair: &Arc<PtyPair>, data: &[u8]) -> SyscallResult {
    match pair.master_write(data) {
        Ok(n) => SyscallResult::Ok(n as u64),
        Err(e) => SyscallResult::Err(e as u64),
    }
}

/// write(2) on a slave: output to the master.
pub fn write_slave(pair: &Arc<PtyPair>, data: &[u8], _nonblocking: bool) -> SyscallResult {
    // A hung-up slave fails writes with EIO before any job-control check.
    if pair.master_closed() {
        return SyscallResult::Err(EIO as u64);
    }
    if let Err(e) = jobctl::check(&TtyRef::Pty(pair.clone()), JobOp::Write) {
        return SyscallResult::Err(e as u64);
    }
    match pair.slave_write(data) {
        Ok(n) => SyscallResult::Ok(n as u64),
        Err(e) => SyscallResult::Err(e as u64),
    }
}
