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

/// Process ID type
pub type Pid = u64;

/// Thread ID type
pub type Tid = u64;

/// File descriptor type
pub type Fd = u64;

/// Standard file descriptors
pub mod fd {
    use super::Fd;
    pub const STDIN: Fd = 0;
    pub const STDOUT: Fd = 1;
    pub const STDERR: Fd = 2;
}

/// Clock IDs for clock_gettime (Linux conventions)
pub mod clock {
    pub const REALTIME: u32 = 0;
    pub const MONOTONIC: u32 = 1;
}
