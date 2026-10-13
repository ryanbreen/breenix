//! read(2) on a pseudo-terminal slave: input the line discipline has
//! processed.

use crate::syscall::blocking_io::wait_prepared;
use crate::syscall::errno::EAGAIN;
use crate::syscall::SyscallResult;
use crate::task::thread::ThreadState;
use crate::tty::ctty::TtyRef;
use crate::tty::jobctl::{self, JobOp};
use crate::tty::pty::io::{copy_out, read_buffer};
use crate::tty::pty::PtyPair;
use alloc::sync::Arc;

/// A read of a hung-up slave (its master has closed) returns end of file at
/// once, whatever input is still queued, as Linux's hung-up file does.
pub fn read(pair: &Arc<PtyPair>, buf_ptr: u64, count: u64, nonblocking: bool) -> SyscallResult {
    if pair.master_closed() {
        return SyscallResult::Ok(0);
    }
    if let Err(e) = jobctl::check(&TtyRef::Pty(pair.clone()), JobOp::Read) {
        return SyscallResult::Err(e as u64);
    }
    let mut buf = match read_buffer(count) {
        Ok(buf) => buf,
        Err(result) => return result,
    };
    loop {
        let outcome = {
            let mut ldisc = pair.ldisc.lock();
            if pair.master_closed() {
                return SyscallResult::Ok(0);
            }
            if let Ok(n) = ldisc.read(&mut buf) {
                if n > 0 {
                    drop(ldisc);
                    return copy_out(buf_ptr, &buf, n);
                }
            }
            if nonblocking {
                return SyscallResult::Err(EAGAIN as u64);
            }
            pair.slave_readers
                .prepare_to_wait_checked(ThreadState::BlockedOnIO, None, || {
                    !ldisc.has_data() && !pair.master_closed()
                })
        };
        if let Err(error) = wait_prepared(&pair.slave_readers, outcome) {
            return SyscallResult::Err(error as u64);
        }
    }
}
