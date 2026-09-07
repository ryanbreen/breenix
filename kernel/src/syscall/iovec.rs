//! Vectored I/O syscalls: readv and writev
//!
//! These syscalls read/write data from/to multiple buffers in a single call.
//! Required by musl libc for stdio operations.

use super::errno;
use super::handlers;
use super::userptr::copy_from_user;
use super::SyscallResult;

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

    // Snapshot the vector metadata once so the atomicity decision and copies
    // describe the same request even if another userspace thread edits it.
    let mut vectors = alloc::vec::Vec::with_capacity(iovcnt as usize);
    let mut length = 0u64;
    for i in 0..iovcnt {
        let Some(address) = iov_ptr.checked_add(i * core::mem::size_of::<IoVec>() as u64) else {
            return SyscallResult::Err(errno::EFAULT as u64);
        };
        let vector: IoVec = match copy_from_user(address as *const IoVec) {
            Ok(vector) => vector,
            Err(error) => return SyscallResult::Err(error),
        };
        length = match length.checked_add(vector.iov_len) {
            Some(length) if length <= isize::MAX as u64 => length,
            _ => return SyscallResult::Err(errno::EINVAL as u64),
        };
        vectors.push(vector);
    }
    if length > 0 && length <= crate::ipc::pipe::PIPE_BUF as u64 {
        // Hold only an owned endpoint snapshot across user copies and waiting.
        let endpoint = crate::task::scheduler::current_thread_id().and_then(|tid| {
            let manager = crate::process::manager();
            let (_, process) = manager.as_ref()?.find_process_by_thread(tid)?;
            let entry = process.fd_table.get(fd as i32)?;
            match &entry.kind {
                crate::ipc::FdKind::PipeWrite(buffer)
                | crate::ipc::FdKind::FifoWrite(_, buffer) => Some((
                    buffer.clone(),
                    entry.status_flags & crate::ipc::fd::status_flags::O_NONBLOCK != 0,
                )),
                _ => None,
            }
        });
        if let Some((buffer, nonblocking)) = endpoint {
            // Gather before taking the pipe lock. One adapter call preserves
            // aggregate PIPE_BUF atomicity without faulting on userspace under
            // the buffer lock or retaining a lock across a prepared wait.
            let mut gathered = alloc::vec::Vec::with_capacity(length as usize);
            for vector in &vectors {
                for offset in 0..vector.iov_len {
                    let Some(address) = vector.iov_base.checked_add(offset) else {
                        return SyscallResult::Err(errno::EFAULT as u64);
                    };
                    match copy_from_user(address as *const u8) {
                        Ok(byte) => gathered.push(byte),
                        Err(error) => return SyscallResult::Err(error),
                    }
                }
            }
            return super::blocking_io::write_pipe(&buffer, &gathered, nonblocking);
        }
    }

    let mut total: u64 = 0;
    for iov in vectors {
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
