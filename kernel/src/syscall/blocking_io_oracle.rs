//! Boot-only, read-only witnesses for #813. No query performs a wake or transfer.
use super::{errno, userptr, SyscallResult};
use crate::ipc::fd::FdKind;
use alloc::sync::Arc;

pub const QUERY: u64 = 0xB8130001;
pub const FIFO_FIXTURE: u64 = 0xB8130002;
pub const INPUT_QUERY: u64 = 0xB8130003;
pub const INPUT_INJECT: u64 = 0xB8130004;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Witness {
    tid: u64,
    target_fd: u64,
    kind: u64,
    identity: u64,
    queued: u64,
    blocked: u64,
    blocked_in_syscall: u64,
    occupancy: u64,
    capacity: u64,
    atomic_limit: u64,
}

pub fn dispatch(fd: u64, request: u64, arg: u64) -> SyscallResult {
    let result = if request == INPUT_QUERY || request == INPUT_INJECT {
        console(fd, request, arg)
    } else if request == FIFO_FIXTURE {
        fixture(fd, arg)
    } else {
        query(fd, arg)
    };
    match result {
        Ok(()) => SyscallResult::Ok(0),
        Err(error) => SyscallResult::Err(error),
    }
}

fn fixture(fd: u64, arg: u64) -> Result<(), u64> {
    // A valid caller-owned descriptor is required even for fixture setup.
    let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH as u64)?;
    let path = userptr::copy_cstr_from_user(arg)?;
    if !path.starts_with("/tmp/pipe_fifo_oracle_")
        || path.len() > 128
        || path.contains("..")
        || fd > i32::MAX as u64
    {
        return Err(errno::EINVAL as u64);
    }
    {
        let guard = crate::process::manager();
        let manager = guard.as_ref().ok_or(errno::ESRCH as u64)?;
        let (_, caller) = manager
            .find_process_by_thread(tid)
            .ok_or(errno::ESRCH as u64)?;
        caller.fd_table.get(fd as i32).ok_or(errno::EBADF as u64)?;
    }
    crate::ipc::fifo::FIFO_REGISTRY
        .create(&path, 0o600)
        .map_err(|e| e as u64)
}

fn query(fd: u64, arg: u64) -> Result<(), u64> {
    if arg % core::mem::align_of::<Witness>() as u64 != 0 {
        return Err(errno::EINVAL as u64);
    }
    let mut witness = userptr::copy_from_user(arg as *const Witness)?;
    if fd > i32::MAX as u64 || witness.target_fd > i32::MAX as u64 {
        return Err(errno::EBADF as u64);
    }
    let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH as u64)?;
    let (buffer, kind) = {
        let guard = crate::process::manager();
        let manager = guard.as_ref().ok_or(errno::ESRCH as u64)?;
        let (caller_pid, caller) = manager
            .find_process_by_thread(tid)
            .ok_or(errno::ESRCH as u64)?;
        let (target_pid, target) = manager
            .find_process_by_thread(witness.tid)
            .ok_or(errno::ESRCH as u64)?;
        if target_pid != caller_pid && target.parent != Some(caller_pid) {
            return Err(errno::EPERM as u64);
        }
        let own = caller.fd_table.get(fd as i32).ok_or(errno::EBADF as u64)?;
        let other = target
            .fd_table
            .get(witness.target_fd as i32)
            .ok_or(errno::EBADF as u64)?;
        (own.kind.clone(), other.kind.clone())
    };
    match (buffer, kind) {
        (FdKind::UnixStream(own), FdKind::UnixStream(other)) => {
            let own_pair = own.lock().pair.clone();
            let (other_pair, writer) = {
                let socket = other.lock();
                (socket.pair.clone(), socket.writer())
            };
            if !Arc::ptr_eq(&own_pair, &other_pair) {
                return Err(errno::EPERM as u64);
            }
            witness.kind = 3;
            witness.identity = Arc::as_ptr(&own_pair) as u64;
            let (queued, occupancy, capacity) = writer.witness(witness.tid);
            witness.queued = queued;
            witness.occupancy = occupancy;
            witness.capacity = capacity;
            witness.atomic_limit = 0;
        }
        (own, other) => {
            let (buffer, kind) = match own {
                FdKind::PipeWrite(buffer) => (buffer, 1),
                FdKind::FifoWrite(_, buffer) => (buffer, 2),
                _ => return Err(errno::EINVAL as u64),
            };
            let same = match &other {
                FdKind::PipeWrite(other) | FdKind::FifoWrite(_, other) => {
                    Arc::ptr_eq(&buffer, other)
                }
                _ => false,
            };
            if !same {
                return Err(errno::EPERM as u64);
            }
            witness.kind = kind;
            witness.identity = Arc::as_ptr(&buffer) as u64;
            let buffer = buffer.lock();
            witness.queued = buffer.write_waiters.contains_waiter(witness.tid) as u64;
            witness.occupancy = buffer.available() as u64;
            witness.capacity = crate::ipc::pipe::PIPE_BUF_SIZE as u64;
            witness.atomic_limit = crate::ipc::pipe::PIPE_BUF as u64;
        }
    }
    let (blocked, in_syscall) = crate::task::scheduler::with_thread_mut(witness.tid, |thread| {
        (
            thread.state == crate::task::thread::ThreadState::BlockedOnIO,
            thread.blocked_in_syscall,
        )
    })
    .ok_or(errno::ESRCH as u64)?;
    witness.blocked = blocked as u64;
    witness.blocked_in_syscall = in_syscall as u64;
    userptr::copy_to_user(arg as *mut Witness, &witness)
}

/// Boot-only thread-context input seam. Injection uses the real enqueue primitive
/// and rejects a remote target unless it is witnessed asleep on the empty ring.
fn console(fd: u64, request: u64, arg: u64) -> Result<(), u64> {
    use crate::fs::devfs::DeviceType;
    if arg % core::mem::align_of::<Witness>() as u64 != 0 {
        return Err(errno::EINVAL as u64);
    }
    let mut witness = userptr::copy_from_user(arg as *const Witness)?;
    if fd > i32::MAX as u64 || witness.target_fd > i32::MAX as u64 {
        return Err(errno::EBADF as u64);
    }
    let tid = crate::task::scheduler::current_thread_id().ok_or(errno::ESRCH as u64)?;
    let kind = {
        let guard = crate::process::manager();
        let manager = guard.as_ref().ok_or(errno::ESRCH as u64)?;
        let (caller_pid, caller) = manager
            .find_process_by_thread(tid)
            .ok_or(errno::ESRCH as u64)?;
        let (target_pid, target) = manager
            .find_process_by_thread(witness.tid)
            .ok_or(errno::ESRCH as u64)?;
        if target_pid != caller_pid && target.parent != Some(caller_pid) {
            return Err(errno::EPERM as u64);
        }
        let own = caller.fd_table.get(fd as i32).ok_or(errno::EBADF as u64)?;
        let other = target
            .fd_table
            .get(witness.target_fd as i32)
            .ok_or(errno::EBADF as u64)?;
        match (&own.kind, &other.kind) {
            (FdKind::Device(DeviceType::Console), FdKind::Device(DeviceType::Console)) => 3,
            (FdKind::Device(DeviceType::Tty), FdKind::Device(DeviceType::Tty)) => 4,
            _ => return Err(errno::EINVAL as u64),
        }
    };
    let (queued, occupancy) = crate::ipc::stdin::input_witness(witness.tid);
    let (blocked, in_syscall) = crate::task::scheduler::with_thread_mut(witness.tid, |thread| {
        (
            thread.state == crate::task::thread::ThreadState::BlockedOnIO,
            thread.blocked_in_syscall,
        )
    })
    .ok_or(errno::ESRCH as u64)?;
    if request == INPUT_INJECT {
        if witness.identity > 255
            || (witness.tid != tid && !(queued && blocked && in_syscall && occupancy == 0))
        {
            return Err(errno::EINVAL as u64);
        }
        if !crate::ipc::stdin::push_byte_from_irq(witness.identity as u8) {
            return Err(errno::EAGAIN as u64);
        }
        return Ok(());
    }
    witness.kind = kind;
    witness.identity = 1; // One global input ring; no kernel address disclosure.
    witness.queued = queued as u64;
    witness.occupancy = occupancy as u64;
    witness.blocked = blocked as u64;
    witness.blocked_in_syscall = in_syscall as u64;
    witness.capacity = crate::ipc::stdin::STDIN_BUF_SIZE as u64;
    witness.atomic_limit = 0;
    userptr::copy_to_user(arg as *mut Witness, &witness)
}
