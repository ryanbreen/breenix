//! Vectored I/O syscalls: readv and writev
//!
//! These syscalls read/write data from/to multiple buffers in a single call.
//! Required by musl libc for stdio operations.

use super::handlers;
use super::SyscallResult;
use super::errno;
use super::userptr::copy_from_user;

/// Maximum number of iovec entries per call (matches Linux UIO_MAXIOV)
const UIO_MAXIOV: u64 = 1024;

/// iovec structure matching Linux ABI
#[repr(C)]
#[derive(Copy, Clone)]
struct IoVec {
    iov_base: u64,
    iov_len: u64,
}

/// writev(fd, iov, iovcnt) - Write data from multiple buffers
///
/// Writes data described by the array of iovec structures to the file descriptor.
/// Returns total bytes written or negative errno on error.
pub fn sys_writev(fd: u64, iov_ptr: u64, iovcnt: u64) -> SyscallResult {
    if iovcnt == 0 {
        return SyscallResult::Ok(0);
    }
    if iovcnt > UIO_MAXIOV {
        return SyscallResult::Err(errno::EINVAL as u64);
    }
    if iov_ptr == 0 {
        return SyscallResult::Err(errno::EFAULT as u64);
    }

    let mut total: u64 = 0;
    for i in 0..iovcnt {
        let iov_addr = iov_ptr + i * core::mem::size_of::<IoVec>() as u64;
        let iov: IoVec = match copy_from_user(iov_addr as *const IoVec) {
            Ok(v) => v,
            Err(e) => return SyscallResult::Err(e as u64),
        };

        if iov.iov_len == 0 {
            continue;
        }

        match handlers::sys_write(fd, iov.iov_base, iov.iov_len) {
            SyscallResult::Ok(n) => {
                total += n;
                // Short write: stop early (like Linux)
                if n < iov.iov_len {
                    break;
                }
            }
            SyscallResult::Err(e) => {
                // If we've already written some data, return partial count
                if total > 0 {
                    break;
                }
                return SyscallResult::Err(e);
            }
        }
    }
    SyscallResult::Ok(total)
}

/// readv(fd, iov, iovcnt) - Read data into multiple buffers
///
/// Reads data from the file descriptor into the buffers described by the
/// array of iovec structures. Returns total bytes read or negative errno.
pub fn sys_readv(fd: u64, iov_ptr: u64, iovcnt: u64) -> SyscallResult {
    if iovcnt == 0 {
        return SyscallResult::Ok(0);
    }
    if iovcnt > UIO_MAXIOV {
        return SyscallResult::Err(errno::EINVAL as u64);
    }
    if iov_ptr == 0 {
        return SyscallResult::Err(errno::EFAULT as u64);
    }

    let mut total: u64 = 0;
    for i in 0..iovcnt {
        let iov_addr = iov_ptr + i * core::mem::size_of::<IoVec>() as u64;
        let iov: IoVec = match copy_from_user(iov_addr as *const IoVec) {
            Ok(v) => v,
            Err(e) => return SyscallResult::Err(e as u64),
        };

        if iov.iov_len == 0 {
            continue;
        }

        match handlers::sys_read(fd, iov.iov_base, iov.iov_len) {
            SyscallResult::Ok(n) => {
                total += n;
                // Short read or EOF: stop early (like Linux)
                if n < iov.iov_len {
                    break;
                }
            }
            SyscallResult::Err(e) => {
                // If we've already read some data, return partial count
                if total > 0 {
                    break;
                }
                return SyscallResult::Err(e);
            }
        }
    }
    SyscallResult::Ok(total)
}
