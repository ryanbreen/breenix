//! Common types used across libbreenix

/// Timespec structure for clock_gettime (matches kernel definition)
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

impl Timespec {
    pub const fn new() -> Self {
        Self {
            tv_sec: 0,
            tv_nsec: 0,
        }
    }

    /// Convert to total nanoseconds
    pub fn as_nanos(&self) -> i128 {
        (self.tv_sec as i128) * 1_000_000_000 + (self.tv_nsec as i128)
    }

    /// Convert to total microseconds
    pub fn as_micros(&self) -> i64 {
        self.tv_sec * 1_000_000 + self.tv_nsec / 1_000
    }

    /// Convert to total milliseconds
    pub fn as_millis(&self) -> i64 {
        self.tv_sec * 1_000 + self.tv_nsec / 1_000_000
    }
}

/// A file descriptor. This is a lightweight copyable handle.
/// For automatic close-on-drop, wrap in `OwnedFd`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Fd(u64);

impl Fd {
    pub const STDIN: Fd = Fd(0);
    pub const STDOUT: Fd = Fd(1);
    pub const STDERR: Fd = Fd(2);

    pub const fn from_raw(raw: u64) -> Self {
        Fd(raw)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Process ID type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Pid(u64);

impl Pid {
    pub const fn from_raw(raw: u64) -> Self {
        Pid(raw)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Thread ID type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Tid(u64);

impl Tid {
    pub const fn from_raw(raw: u64) -> Self {
        Tid(raw)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// A file descriptor with RAII close-on-drop semantics.
pub struct OwnedFd(Fd);

impl OwnedFd {
    pub fn new(fd: Fd) -> Self {
        OwnedFd(fd)
    }
    pub fn fd(&self) -> Fd {
        self.0
    }

    /// Consume self and return the raw Fd without closing.
    pub fn into_raw(self) -> Fd {
        let fd = self.0;
        core::mem::forget(self);
        fd
    }
}

impl Drop for OwnedFd {
    fn drop(&mut self) {
        unsafe {
            crate::syscall::raw::syscall1(crate::syscall::nr::CLOSE, self.0.raw());
        }
    }
}

/// Clock IDs for clock_gettime (Linux conventions)
pub mod clock {
    pub const REALTIME: u32 = 0;
    pub const MONOTONIC: u32 = 1;
}
