//! read(2) and write(2) on pseudo-terminal descriptors.

use super::PtyPair;
use crate::syscall::blocking_io::wait_prepared;
use crate::syscall::errno::{EAGAIN, EIO};
use crate::syscall::SyscallResult;
use crate::task::thread::ThreadState;
use crate::tty::ctty::TtyRef;
use crate::tty::jobctl::{self, JobOp};
use alloc::sync::Arc;

/// The most a master read returns: more than its ring (4095 bytes) ever holds.
const READ_MAX: u64 = super::pair::PTY_BUFFER_SIZE as u64;

/// read(2) on a master: the slave's output.
pub fn read_master(pair: &Arc<PtyPair>, buf_ptr: u64, count: u64, nonblocking: bool) -> SyscallResult {
    let count = count.min(READ_MAX) as usize;
    loop {
        let outcome = {
            let mut ring = pair.slave_to_master.lock();
            let (taken, result) = ring.read_user(buf_ptr, count);
            // Taken bytes made room even when their copy faulted.
            if taken > 0 {
                drop(ring);
                pair.drain_echo();
                pair.slave_writers.wake_up();
                return match result {
                    Ok(n) => SyscallResult::Ok(n as u64),
                    Err(e) => SyscallResult::Err(e as u64),
                };
            }
            if let Err(e) = result {
                return SyscallResult::Err(e as u64);
            }
            // Linux: once the slave has been opened and every slave fd is
            // closed, the master reads the buffered output and then EIO.
            if pair.has_slave_hung_up() {
                return SyscallResult::Err(EIO as u64);
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

/// A write that made progress reports it; otherwise the error.
fn progress_or(done: usize, error: i32) -> SyscallResult {
    if done > 0 {
        SyscallResult::Ok(done as u64)
    } else {
        SyscallResult::Err(error as u64)
    }
}

/// write(2) on a master: input to the slave, through the line discipline.
/// A blocking write waits for room in the slave's input queue, and while
/// master output is stopped (TCXONC on the master), until all of
/// `data` is consumed. Once the slave has hung up nothing will make room, so
/// a write that finds no room then fails with EIO, as on Linux.
pub fn write_master(pair: &Arc<PtyPair>, data: &[u8], nonblocking: bool) -> SyscallResult {
    let mut done = 0;
    while done < data.len() {
        let n = pair.master_write(&data[done..]);
        if n > 0 {
            done += n;
            continue;
        }
        if pair.has_slave_hung_up() {
            return progress_or(done, EIO);
        }
        if nonblocking {
            return progress_or(done, EAGAIN);
        }
        let outcome = {
            let ldisc = pair.ldisc.lock();
            pair.master_writers
                .prepare_to_wait_checked(ThreadState::BlockedOnIO, None, || {
                    (!ldisc.input_room() || pair.master_output_stopped()) && !pair.has_slave_hung_up()
                })
        };
        if let Err(error) = wait_prepared(&pair.master_writers, outcome) {
            return progress_or(done, error);
        }
    }
    SyscallResult::Ok(done as u64)
}

/// write(2) on a slave: output to the master. A blocking write waits for
/// room in the master's buffer, and while output is stopped (TCXONC), until
/// all of `data` is written.
pub fn write_slave(pair: &Arc<PtyPair>, data: &[u8], nonblocking: bool) -> SyscallResult {
    // A hung-up slave fails writes with EIO before any job-control check.
    if pair.master_closed() {
        return SyscallResult::Err(EIO as u64);
    }
    if let Err(e) = jobctl::check(&TtyRef::Pty(pair.clone()), JobOp::Write) {
        return SyscallResult::Err(e as u64);
    }
    pair.drain_echo();
    let mut done = 0;
    while done < data.len() {
        if pair.master_closed() {
            return progress_or(done, EIO);
        }
        // The output settings are read on every pass, so a writer that
        // waited writes with the settings in force when it resumes.
        let termios = pair.termios_snapshot();
        let onlcr = termios.is_opost() && termios.is_onlcr();
        let outcome = {
            let mut ring = pair.slave_to_master.lock();
            if !pair.output_stopped() {
                let n = PtyPair::put_output(&mut ring, &data[done..], onlcr);
                if n > 0 {
                    drop(ring);
                    done += n;
                    pair.master_readers.wake_up();
                    continue;
                }
            }
            if nonblocking {
                return progress_or(done, EAGAIN);
            }
            // Nothing fit: the ring has less room than the next byte needs
            // (two for a translated newline), or output is stopped.
            pair.slave_writers
                .prepare_to_wait_checked(ThreadState::BlockedOnIO, None, || {
                    (pair.output_stopped() || ring.space() < 2) && !pair.master_closed()
                })
        };
        if let Err(error) = wait_prepared(&pair.slave_writers, outcome) {
            return progress_or(done, error);
        }
    }
    SyscallResult::Ok(done as u64)
}
