//! POSIX errno values
//!
//! These match Linux errno values for compatibility.

/// Error numbers returned by syscalls
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum Errno {
    /// Operation not permitted
    EPERM = 1,
    /// No such file or directory
    ENOENT = 2,
    /// No such process
    ESRCH = 3,
    /// Interrupted system call
    EINTR = 4,
    /// I/O error
    EIO = 5,
    /// No such device or address
    ENXIO = 6,
    /// Argument list too long
    E2BIG = 7,
    /// Exec format error
    ENOEXEC = 8,
    /// Bad file descriptor
    EBADF = 9,
    /// No child processes
    ECHILD = 10,
    /// Resource temporarily unavailable
    EAGAIN = 11,
    /// Out of memory
    ENOMEM = 12,
    /// Permission denied
    EACCES = 13,
    /// Bad address
    EFAULT = 14,
    /// Block device required
    ENOTBLK = 15,
    /// Device or resource busy
    EBUSY = 16,
    /// File exists
    EEXIST = 17,
    /// Cross-device link
    EXDEV = 18,
    /// No such device
    ENODEV = 19,
    /// Not a directory
    ENOTDIR = 20,
    /// Is a directory
    EISDIR = 21,
    /// Invalid argument
    EINVAL = 22,
    /// File table overflow
    ENFILE = 23,
    /// Too many open files
    EMFILE = 24,
    /// Not a typewriter
    ENOTTY = 25,
    /// Text file busy
    ETXTBSY = 26,
    /// File too large
    EFBIG = 27,
    /// No space left on device
    ENOSPC = 28,
    /// Illegal seek
    ESPIPE = 29,
    /// Read-only file system
    EROFS = 30,
    /// Too many links
    EMLINK = 31,
    /// Broken pipe
    EPIPE = 32,
    /// Function not implemented
    ENOSYS = 38,
    /// Directory not empty
    ENOTEMPTY = 39,
    /// Address family not supported
    EAFNOSUPPORT = 97,
}

impl Errno {
    /// Convert a raw syscall return value to Result
    ///
    /// Syscalls return negative errno on error, non-negative on success.
    pub fn from_syscall(ret: i64) -> Result<u64, Errno> {
        if ret >= 0 {
            Ok(ret as u64)
        } else {
            Err(Errno::from_raw(-ret))
        }
    }

    /// Convert raw errno value to Errno enum
    pub fn from_raw(val: i64) -> Errno {
        match val {
            1 => Errno::EPERM,
            2 => Errno::ENOENT,
            3 => Errno::ESRCH,
            4 => Errno::EINTR,
            5 => Errno::EIO,
            6 => Errno::ENXIO,
            7 => Errno::E2BIG,
            8 => Errno::ENOEXEC,
            9 => Errno::EBADF,
            10 => Errno::ECHILD,
            11 => Errno::EAGAIN,
            12 => Errno::ENOMEM,
            13 => Errno::EACCES,
            14 => Errno::EFAULT,
            15 => Errno::ENOTBLK,
            16 => Errno::EBUSY,
            17 => Errno::EEXIST,
            18 => Errno::EXDEV,
            19 => Errno::ENODEV,
            20 => Errno::ENOTDIR,
            21 => Errno::EISDIR,
            22 => Errno::EINVAL,
            23 => Errno::ENFILE,
            24 => Errno::EMFILE,
            25 => Errno::ENOTTY,
            26 => Errno::ETXTBSY,
            27 => Errno::EFBIG,
            28 => Errno::ENOSPC,
            29 => Errno::ESPIPE,
            30 => Errno::EROFS,
            31 => Errno::EMLINK,
            32 => Errno::EPIPE,
            38 => Errno::ENOSYS,
            39 => Errno::ENOTEMPTY,
            97 => Errno::EAFNOSUPPORT,
            _ => Errno::EINVAL, // Unknown error
        }
    }
}
