//! Vectored I/O, including explicit-offset transfers on regular files.

use alloc::vec::Vec;
use super::errno;
use super::handlers;
use super::iovec_range::{positioned_offset, snapshot_vectors, vector_prefix, IoVec, UIO_MAXIOV};
use super::userptr::{copy_from_user, read_user_bytes, write_user_prefix};
use super::SyscallResult;
use crate::ipc::FdKind;

/// writev(fd, iov, iovcnt) - Write data from multiple buffers
///
/// Writes data described by the array of iovec structures to the file descriptor.
/// Returns total bytes written or negative errno on error.
pub fn sys_writev(fd: u64, iov_ptr: u64, iovcnt: u64) -> SyscallResult {
    if iovcnt == 0 {
        return SyscallResult::Ok(0);
    }
    // Snapshot the vector metadata once so the atomicity decision and copies
    // describe the same request even if another userspace thread edits it.
    let (vectors, length) = match user_vectors(iov_ptr, iovcnt) {
        Ok((vectors, length)) => (vectors, length as u64),
        Err(error) => return SyscallResult::Err(error),
    };
    if length > 0 && length <= crate::ipc::pipe::PIPE_BUF as u64 {
        // Hold only an owned endpoint snapshot across user copies and waiting.
        let endpoint = crate::task::scheduler::current_thread_id().and_then(|tid| {
            let manager = crate::process::manager();
            let (_, process) = manager.as_ref()?.find_process_by_thread(tid)?;
            let entry = process.fd_table.get(fd as i32)?;
            match &entry.kind {
                crate::ipc::FdKind::PipeWrite(buffer)
                | crate::ipc::FdKind::FifoWrite(_, buffer, _)
                | crate::ipc::FdKind::FifoReadWrite(_, buffer, _) => Some((
                    buffer.clone(),
                    entry.status_flags() & crate::ipc::fd::status_flags::O_NONBLOCK != 0,
                    match &entry.kind {
                        crate::ipc::FdKind::FifoWrite(_, _, fifo) | crate::ipc::FdKind::FifoReadWrite(_, _, fifo) => Some(fifo.clone()),
                        _ => None,
                    },
                )),
                _ => None,
            }
        });
        if let Some((buffer, nonblocking, fifo)) = endpoint {
            // Gather before taking the pipe lock. One adapter call preserves
            // aggregate PIPE_BUF atomicity without faulting on userspace under
            // the buffer lock or retaining a lock across a prepared wait.
            let mut gathered = alloc::vec::Vec::new();
            if gathered.try_reserve_exact(length as usize).is_err() { return SyscallResult::Err(errno::ENOMEM as u64); }
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
            let result = super::blocking_io::write_pipe(&buffer, &gathered, nonblocking);
            if matches!(result, SyscallResult::Ok(n) if n > 0) { if let Some(fifo) = fifo { super::fs::touch_fifo(&fifo, false); } }
            return result;
        }
    }

    let mut total: u64 = 0;
    for iov in vectors {
        if iov.iov_len == 0 {
            continue;
        }

        match handlers::write_vector(fd, iov.iov_base, iov.iov_len, total == 0) {
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

fn user_vectors(ptr: u64, count: u64) -> Result<(Vec<IoVec>, usize), u64> {
    snapshot_vectors(ptr, count, |addr| copy_from_user(addr as *const IoVec))
}

fn gather(vectors: &[IoVec], len: usize) -> Result<Vec<u8>, u64> {
    let mut data = Vec::new();
    data.try_reserve_exact(len).map_err(|_| errno::ENOMEM as u64)?;
    data.resize(len, 0);
    for (addr, span) in vector_prefix(vectors, len) {
        let output = &mut data[span];
        read_user_bytes(output.as_mut_ptr(), addr, output.len())?;
    }
    Ok(data)
}

fn scatter(vectors: &[IoVec], data: &[u8]) -> Result<u64, u64> {
    for (addr, span) in vector_prefix(vectors, data.len()) {
        let bytes = &data[span.clone()];
        let copied = write_user_prefix(addr, bytes.as_ptr(), bytes.len());
        if copied < bytes.len() {
            let total = span.start + copied;
            return if total == 0 { Err(errno::EFAULT as u64) } else { Ok(total as u64) };
        }
    }
    Ok(data.len() as u64)
}

pub fn sys_preadv(fd: i32, ptr: u64, count: i32, offset: i64) -> SyscallResult {
    positioned_result(transfer_at(fd, ptr, count, offset, false))
}

pub fn sys_pwritev(fd: i32, ptr: u64, count: i32, offset: i64) -> SyscallResult {
    positioned_result(transfer_at(fd, ptr, count, offset, true))
}

fn positioned_result(result: Result<u64, u64>) -> SyscallResult {
    match result {
        Ok(count) => SyscallResult::Ok(count),
        Err(error) => SyscallResult::Err(error),
    }
}

fn transfer_at(fd: i32, ptr: u64, count: i32, offset: i64, write: bool) -> Result<u64, u64> {
    let offset = positioned_offset(offset, count)?;
    let entry = super::metadata::descriptor_entry(fd)?;
    let open = match &entry.kind {
        FdKind::RegularFile(file) => Some((file.open.fs.clone(), file.open.node.clone())),
        _ => None,
    };
    let userspace = match &entry.kind {
        FdKind::Ipc(object) => object.as_any().downcast_ref::<crate::fs::ufs::file::UfsFile>(),
        _ => None,
    };
    if open.is_none() && userspace.is_none() {
        return Err(match entry.kind {
            FdKind::Directory(_) | FdKind::ProcfsDirectory { .. } => errno::EISDIR,
            _ => errno::ESPIPE,
        } as u64);
    }
    if (write && !entry.writable()) || (!write && !entry.readable()) {
        return Err(errno::EBADF as u64);
    }
    let (vectors, len) = user_vectors(ptr, count as u64)?;
    if len == 0 {
        return Ok(0);
    }
    if let Some(file) = userspace {
        return if write {
            file.pwritev_once(offset, len, |allowed| gather(&vectors, allowed))
        } else {
            scatter(&vectors, &file.preadv_once(offset, len)?)
        };
    }
    let (fs, node) = open.expect("regular file selected above");
    // The backend sees one contiguous explicit-offset operation, even when
    // userspace supplied several buffers or opened the file with O_APPEND.
    if write {
        let cred = crate::fs::permissions::Credentials::current(false)?;
        handlers::write_positional_file(
            fs.as_ref(),
            &node,
            offset,
            len,
            super::resource::current_fsize(),
            !cred.privileged(),
            |allowed| gather(&vectors, allowed),
        )
    } else {
        scatter(&vectors, &handlers::read_positional_file(fs.as_ref(), &node, offset, len)?)
    }
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
