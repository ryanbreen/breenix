//! POSIX errno values
//!
//! Standard error codes returned by system calls.

/// Operation not permitted
pub const EPERM: i32 = 1;

/// No such file or directory
pub const ENOENT: i32 = 2;

/// No such process
#[allow(dead_code)] // Part of POSIX errno API
pub const ESRCH: i32 = 3;

/// I/O error
pub const EIO: i32 = 5;

/// Bad file descriptor
pub const EBADF: i32 = 9;

/// No child processes
pub const ECHILD: i32 = 10;

/// Resource temporarily unavailable (would block)
pub const EAGAIN: i32 = 11;

/// Cannot allocate memory (part of memory API)
#[allow(dead_code)]
pub const ENOMEM: i32 = 12;

/// Permission denied
pub const EACCES: i32 = 13;

/// Bad address
pub const EFAULT: i32 = 14;

/// Not a directory
#[allow(dead_code)] // Part of POSIX errno API
pub const ENOTDIR: i32 = 20;

/// Is a directory
pub const EISDIR: i32 = 21;

/// Invalid argument
pub const EINVAL: i32 = 22;

/// Too many open files
pub const EMFILE: i32 = 24;

/// Device or resource busy
pub const EBUSY: i32 = 16;

/// File exists (O_CREAT + O_EXCL)
pub const EEXIST: i32 = 17;

/// No space left on device
pub const ENOSPC: i32 = 28;

/// File name too long
#[allow(dead_code)] // Part of POSIX errno API
pub const ENAMETOOLONG: i32 = 36;

/// Directory not empty
pub const ENOTEMPTY: i32 = 39;

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
