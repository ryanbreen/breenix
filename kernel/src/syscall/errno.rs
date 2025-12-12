//! POSIX errno values
//!
//! Standard error codes returned by system calls.

/// Bad file descriptor
pub const EBADF: i32 = 9;

/// Resource temporarily unavailable (would block)
pub const EAGAIN: i32 = 11;

/// Cannot allocate memory
pub const ENOMEM: i32 = 12;

/// Bad address
pub const EFAULT: i32 = 14;

/// Invalid argument
pub const EINVAL: i32 = 22;

/// Function not implemented (used by syscall dispatcher)
#[allow(dead_code)]
pub const ENOSYS: i32 = 38;

/// Not a socket
pub const ENOTSOCK: i32 = 88;

/// Address family not supported
pub const EAFNOSUPPORT: i32 = 97;

/// Address already in use
pub const EADDRINUSE: i32 = 98;

/// Network is unreachable (part of network API)
#[allow(dead_code)]
pub const ENETUNREACH: i32 = 101;

/// Transport endpoint is not connected (part of network API)
#[allow(dead_code)]
pub const ENOTCONN: i32 = 107;
