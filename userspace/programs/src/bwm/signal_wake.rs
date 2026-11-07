//! Retain signal notifications across the unblock-to-poll gap.
use libbreenix::{errno::Errno, error::Error, io, types::Fd};
use std::sync::atomic::{AtomicI32, Ordering};

static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

pub(super) struct SignalWake { read: Fd, write: Fd }

impl SignalWake {
    // Set up before installing handlers or starting threads. Both signals stay
    // blocked outside the shutdown boundary; the descriptors live for main.
    pub(super) fn open() -> Result<Self, Error> {
        let (read, write) = io::pipe2(io::status_flags::O_NONBLOCK | io::status_flags::O_CLOEXEC)?;
        WRITE_FD.store(write.raw() as i32, Ordering::Release);
        Ok(Self { read, write })
    }
    pub(super) fn fd(&self) -> i32 { self.read.raw() as i32 }
    pub(super) fn drain(&self) {
        let mut bytes = [0; 64];
        loop {
            match io::read(self.read, &mut bytes) {
                Ok(0) => break,
                Ok(_) | Err(Error::Os(Errno::EINTR)) => continue,
                Err(_) => break,
            }
        }
    }
}
impl Drop for SignalWake {
    fn drop(&mut self) {
        WRITE_FD.store(-1, Ordering::Release);
        let _ = io::close(self.write);
        let _ = io::close(self.read);
    }
}

// Only an atomic load and a nonblocking raw-syscall wrapper in a signal handler.
// A full pipe is already readable, so coalescing that notification loses no wake.
pub(super) fn notify() {
    let fd = WRITE_FD.load(Ordering::Acquire);
    if fd < 0 { return; }
    while let Err(Error::Os(Errno::EINTR)) = io::write(Fd::from_raw(fd as u64), &[1]) {}
}
