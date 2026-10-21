//! Unix account formats and updates, written from POSIX.1-2024 and the
//! passwd(5), group(5), shadow(5) format descriptions. No implementation source
//! from another system is used. The allocation-free parsers also serve libc.
pub mod parse;

/// The shared POSIX record-lock ABI (x86-64 and AArch64).
#[repr(C)]
pub struct PasswordLock {
    pub kind: i16,
    pub whence: i16,
    pub start: i64,
    pub len: i64,
    pub pid: i32,
}
impl PasswordLock {
    /// A write lock from the start of the file through EOF, including later extensions.
    pub const fn write() -> Self {
        Self {
            kind: 1,
            whence: 0,
            start: 0,
            len: 0,
            pid: 0,
        }
    }
}

#[cfg(feature = "std")]
mod store;
#[cfg(feature = "std")]
pub use store::*;
