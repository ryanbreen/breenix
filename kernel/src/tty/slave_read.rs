//! Slave-side terminal reads with POSIX canonical and MIN/TIME semantics.
use super::ctty::TtyRef;
use super::jobctl::{self, JobOp};
use super::line_discipline::{EOF_MARKER, MAX_CANON};
use super::pty::PtyPair;
use crate::ipc::wait::{wait_on, IpcDeadline, WaitEnd};
use crate::syscall::{errno, SyscallResult};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

fn now_ns() -> u64 {
    let (s, ns) = crate::time::get_monotonic_time_ns();
    s.saturating_mul(1_000_000_000).saturating_add(ns)
}

pub fn read(pair: &Arc<PtyPair>, buf_ptr: u64, count: u64, nonblocking: bool) -> SyscallResult {
    if count == 0 {
        return SyscallResult::Ok(0);
    }
    if pair.master_closed() { return SyscallResult::Ok(0); }
    let terminal = TtyRef::Pty(pair.clone());
    if let Err(e) = jobctl::check(&terminal, JobOp::Read) {
        return SyscallResult::Err(e as u64);
    }
    let count = count.min(MAX_CANON as u64) as usize;
    let mut buf = Vec::new();
    if buf.try_reserve_exact(count).is_err() {
        return SyscallResult::Err(errno::ENOMEM as u64);
    }
    buf.resize(count, 0);
    let input_epoch = pair.ldisc.lock().input_epoch();
    let mut configuration = None;
    let mut seen = 0;
    let mut deadline = IpcDeadline::Never;
    let mut expired = false;
    loop {
        if pair.master_closed() { return SyscallResult::Ok(0); }
        if let Err(e) = jobctl::check(&terminal, JobOp::Read) {
            return SyscallResult::Err(e as u64);
        }
        let mut ldisc = pair.ldisc.lock();
        let attrs = ldisc.termios();
        let canonical = attrs.is_canonical();
        let minimum = usize::from(attrs.vmin()).min(buf.len());
        let timeout = u64::from(attrs.vtime()) * 100_000_000;
        let current = (canonical, minimum, timeout);
        if configuration != Some(current) {
            configuration = Some(current);
            seen = 0;
            expired = false;
            deadline = if !canonical && minimum == 0 && timeout != 0 {
                IpcDeadline::Monotonic(now_ns().saturating_add(timeout))
            } else {
                IpcDeadline::Never
            };
        }
        let available = ldisc.bytes_available();
        if !canonical && minimum > 0 && timeout > 0 {
            if available == 0 {
                // A flush or another reader can remove the first byte while
                // waiting. An inter-byte timeout must never return zero bytes.
                deadline = IpcDeadline::Never;
                expired = false;
            } else if available > seen {
                deadline = IpcDeadline::Monotonic(now_ns().saturating_add(timeout));
            }
        }
        seen = available;
        if pair.master_closed() {
            drop(ldisc);
            return SyscallResult::Ok(0);
        }
        let ready = if canonical {
            ldisc.has_data()
        } else {
            available >= minimum && (minimum > 0 || timeout == 0)
                || (minimum == 0 && available > 0)
                || expired
        };
        if ready || (nonblocking && available > 0) {
            let result = ldisc.read_user(buf_ptr, &mut buf);
            drop(ldisc);
            if result.is_ok() || result == Err(EOF_MARKER) {
                pair.master_writers.wake_up();
            }
            return match result {
                Ok(n) => SyscallResult::Ok(n as u64),
                Err(EOF_MARKER) => SyscallResult::Ok(0),
                Err(e) => SyscallResult::Err(e as u64),
            };
        }
        let observed = input_epoch.load(Ordering::Acquire);
        drop(ldisc);
        if nonblocking {
            return SyscallResult::Err(errno::EAGAIN as u64);
        }
        // Capture the sequence under ldisc, release it, then register under
        // the waitqueue lock. An intervening input/settings change prevents
        // sleep; producers release their object lock before waking this queue.
        match wait_on(&pair.slave_readers, &deadline, || {
            !pair.master_closed() && input_epoch.load(Ordering::Acquire) == observed
        }) {
            WaitEnd::Woken => {}
            WaitEnd::TimedOut => expired = true,
            WaitEnd::Interrupted(e) => return SyscallResult::Err(e),
        }
    }
}
