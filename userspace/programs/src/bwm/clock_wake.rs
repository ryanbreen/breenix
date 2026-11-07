//! Translate changes to the shared UI-clock futex into pollable pipe readiness.
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn fcntl(fd: i32, command: i32, ...) -> i32;
}

pub(super) trait Wait: Send + 'static {
    fn value(&self) -> u32;
    /// Wait while the word equals `expected`; false stops the worker.
    fn wait(&self, expected: u32) -> bool;
}

pub(super) struct ClockWake {
    read: File,
    pending: Arc<AtomicBool>,
}
impl ClockWake {
    pub(super) fn start<W: Wait>(source: Option<W>) -> io::Result<Option<Self>> {
        let Some(source) = source else { return Ok(None) };
        let mut fds = [-1; 2];
        if unsafe { pipe(fds.as_mut_ptr()) } != 0 { return Err(io::Error::last_os_error()); }
        // The two descriptors are newly owned by this process.
        let read = unsafe { File::from_raw_fd(fds[0]) };
        let mut write = unsafe { File::from_raw_fd(fds[1]) };
        for fd in fds {
            if unsafe { fcntl(fd, 2, 1) } == -1 { return Err(io::Error::last_os_error()); }
        }
        let pending = Arc::new(AtomicBool::new(false));
        let worker_pending = pending.clone();
        // Sample before spawning, so a step before the worker starts is observed.
        let mut seen = source.value();
        std::thread::Builder::new().name("bwm-clock-wake".into()).spawn(move || {
            loop {
                if !source.wait(seen) { break; }
                let current = source.value();
                if current != seen {
                    seen = current;
                    if !worker_pending.swap(true, Ordering::AcqRel) && write.write_all(&[1]).is_err() { break; }
                }
            }
        })?;
        Ok(Some(Self { read, pending }))
    }
    pub(super) fn fd(&self) -> i32 { self.read.as_raw_fd() }
    pub(super) fn drain(&self) -> io::Result<()> {
        // Clear before reading: a concurrent step can queue the next byte.
        self.pending.store(false, Ordering::Release);
        (&self.read).read_exact(&mut [0])
    }
}

#[cfg(not(test))]
mod page {
    use super::*;
    use libbreenix::{memory, syscall::{nr, raw}};
    use std::sync::atomic::AtomicU32;

    pub(super) struct Page(*mut u8);
    // The mapping stays alive in the worker and its futex word is atomic.
    unsafe impl Send for Page {}
    impl Drop for Page {
        fn drop(&mut self) { let _ = memory::munmap(self.0, 64); }
    }
    impl Wait for Page {
        fn value(&self) -> u32 {
            unsafe { (&*self.0.add(32).cast::<AtomicU32>()).load(Ordering::Acquire) }
        }
        fn wait(&self, expected: u32) -> bool {
            let result = unsafe { raw::syscall6(nr::FUTEX, self.0.add(32) as u64, 0, expected as u64, 0, 0, 0) } as i64;
            if result >= 0 || result == -11 || result == -4 { true } else {
                eprintln!("[bwm] UI clock futex wait failed: {}", -result);
                false
            }
        }
    }
    pub(super) fn open() -> io::Result<Option<Page>> {
        let Some(path) = std::env::var_os("BREENIX_UI_CLOCK") else { return Ok(None) };
        let file = File::open(path)?;
        if file.metadata()?.len() < 64 { return Err(io::Error::new(io::ErrorKind::InvalidData, "short UI clock page")); }
        let ptr = memory::mmap(std::ptr::null_mut(), 64, memory::PROT_READ, memory::MAP_SHARED, file.as_raw_fd(), 0)
            .map_err(|e| io::Error::other(format!("UI clock mmap: {e}")))?;
        let page = Page(ptr);
        if unsafe { std::slice::from_raw_parts(ptr, 8) } != b"BGCLK001" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid UI clock magic"));
        }
        Ok(Some(page))
    }
}
#[cfg(not(test))]
pub(super) fn open() -> io::Result<Option<ClockWake>> { ClockWake::start(page::open()?) }
