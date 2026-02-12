//! Unified error type for libbreenix operations.
//!
//! All public functions in libbreenix return `Result<T, Error>` for consistent
//! error handling. Use the `?` operator freely across modules.

use crate::errno::Errno;

/// Unified error type for libbreenix operations.
///
/// All public functions in libbreenix return `Result<T, Error>` for consistent
/// error handling. Use the `?` operator freely across modules.
#[derive(Debug)]
pub enum Error {
    /// A POSIX errno from a failed syscall.
    Os(Errno),
}

impl Error {
    /// Convert a raw syscall return value to `Result`.
    ///
    /// Syscalls return negative values on failure (negated errno).
    /// Non-negative values indicate success.
    #[inline]
    pub fn from_syscall(ret: i64) -> Result<u64, Error> {
        if ret < 0 {
            Err(Error::Os(Errno::from_raw(-ret)))
        } else {
            Ok(ret as u64)
        }
    }
}

impl From<Errno> for Error {
    fn from(e: Errno) -> Self {
        Error::Os(e)
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Os(e) => write!(f, "{:?}", e),
        }
    }
}
