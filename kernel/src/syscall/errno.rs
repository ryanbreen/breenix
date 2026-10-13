//! POSIX errno values
//!
//! Standard error codes returned by system calls.

/// Operation not permitted
pub const EPERM: i32 = 1;

/// No such file or directory
pub const ENOENT: i32 = 2;

/// No such process
#[allow(dead_code)]
pub const ESRCH: i32 = 3;

/// Interrupted system call
pub const EINTR: i32 = 4;

/// Kernel-internal, never seen by userspace: a signal interrupted a wait that
/// is to be resumed (SA_RESTART, or no handler runs). The syscall return path
/// re-executes the syscall instead of returning this (Linux's ERESTARTSYS).
pub const ERESTARTSYS: i32 = 512;

/// I/O error
pub const EIO: i32 = 5;

/// Argument and environment list exceeds ARG_MAX.
pub const E2BIG: i32 = 7;

/// Executable image has an unrecognized or malformed format.
pub const ENOEXEC: i32 = 8;

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

/// Device or resource busy
pub const EBUSY: i32 = 16;

/// File exists
pub const EEXIST: i32 = 17;

/// Cross-device link (the two names are on different filesystems)
pub const EXDEV: i32 = 18;

/// No such device (a descriptor that cannot back a memory mapping)
pub const ENODEV: i32 = 19;

/// Not a directory
pub const ENOTDIR: i32 = 20;

/// Is a directory
pub const EISDIR: i32 = 21;

/// Invalid argument
pub const EINVAL: i32 = 22;

/// Too many open files
pub const EMFILE: i32 = 24;

/// Not a typewriter (inappropriate ioctl for device)
pub const ENOTTY: i32 = 25;

/// File too large
pub const EFBIG: i32 = 27;

/// No space left on device
pub const ENOSPC: i32 = 28;

/// Illegal seek (not a seekable fd)
pub const ESPIPE: i32 = 29;

/// Too many links
pub const EMLINK: i32 = 31;

/// Broken pipe
pub const EPIPE: i32 = 32;

/// Result too large / buffer too small
pub const ERANGE: i32 = 34;

/// Resource deadlock would occur (a blocking record lock would wait forever)
pub const EDEADLK: i32 = 35;

/// No locks available (the record-lock table is full)
pub const ENOLCK: i32 = 37;

/// Function not implemented (used by syscall dispatcher)
#[allow(dead_code)]
pub const ENOSYS: i32 = 38;

/// File name too long
pub const ENAMETOOLONG: i32 = 36;

/// Directory not empty
pub const ENOTEMPTY: i32 = 39;

/// Too many levels of symbolic links
pub const ELOOP: i32 = 40;

/// No message of the desired type (msgrcv with IPC_NOWAIT)
pub const ENOMSG: i32 = 42;

/// Identifier removed (a System V IPC object removed while a call waited on it)
pub const EIDRM: i32 = 43;

/// Value too large for its type (a record lock range past the largest offset)
pub const EOVERFLOW: i32 = 75;

/// Not a socket
pub const ENOTSOCK: i32 = 88;

/// Message too long (a message queue's message size)
pub const EMSGSIZE: i32 = 90;

/// Address family not supported
pub const EAFNOSUPPORT: i32 = 97;

/// Address already in use
pub const EADDRINUSE: i32 = 98;

/// Network is unreachable (part of network API)
#[allow(dead_code)]
pub const ENETUNREACH: i32 = 101;

/// Connection refused
pub const ECONNREFUSED: i32 = 111;

/// Operation now in progress (used for non-blocking connect)
pub const EINPROGRESS: i32 = 115;

/// Transport endpoint is already connected
pub const EISCONN: i32 = 106;

/// Transport endpoint is not connected (part of network API)
pub const ENOTCONN: i32 = 107;

/// Connection timed out (POSIX errno, used for network timeouts)
#[allow(dead_code)]
pub const ETIMEDOUT: i32 = 110;

/// Operation not supported
pub const EOPNOTSUPP: i32 = 95;

/// Numerical argument out of domain (a socket timeout's microseconds)
pub const EDOM: i32 = 33;

/// Destination address required
pub const EDESTADDRREQ: i32 = 89;

/// Protocol wrong type for socket
pub const EPROTOTYPE: i32 = 91;

/// Protocol not available (an unknown socket option)
pub const ENOPROTOOPT: i32 = 92;

/// Protocol not supported
pub const EPROTONOSUPPORT: i32 = 93;

/// Cannot assign requested address
pub const EADDRNOTAVAIL: i32 = 99;

/// Software caused connection abort
pub const ECONNABORTED: i32 = 103;

/// Connection reset by peer
pub const ECONNRESET: i32 = 104;

/// No buffer space available
pub const ENOBUFS: i32 = 105;

/// No route to host
pub const EHOSTUNREACH: i32 = 113;

/// Operation already in progress
pub const EALREADY: i32 = 114;
