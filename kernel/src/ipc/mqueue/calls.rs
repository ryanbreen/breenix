//! The message queue system calls: mq_open, mq_unlink, mq_timedsend,
//! mq_timedreceive, mq_notify and mq_getsetattr, with Linux's structures.
//!
//! Each call copies its arguments in from user memory, looks the descriptor
//! up under PROCESS_MANAGER, then works on the queue with PM released, and
//! copies results out with no queue lock held.

use super::store::{self, MqAttr, MQ_PRIO_MAX};
use super::{MessageQueue, MqFile, MqNotify, NotifyKind};
use crate::fs::permissions::Credentials;
use crate::ipc::fd::{status_flags, FdKind, FileDescriptor};
use crate::ipc::object::IpcFile;
use crate::ipc::wait;
use crate::signal::timers::{SIGEV_NONE, SIGEV_SIGNAL};
use crate::syscall::errno::{EBADF, EFAULT, EINVAL, EMFILE, ENOMEM, ESRCH};
use crate::syscall::userptr::{copy_cstr_from_user, copy_from_user, copy_to_user, read_user_bytes, write_user_bytes};
use crate::syscall::SyscallResult;
use alloc::sync::Arc;
use alloc::vec::Vec;

const O_CREAT: u32 = 0o100;

/// struct sigevent as the Linux ABI lays it out: sigev_value, sigev_signo,
/// sigev_notify, then a union padding it to 64 bytes.
#[repr(C)]
#[derive(Clone, Copy)]
struct SigEvent {
    value: u64,
    signo: i32,
    notify: i32,
    pad: [u64; 6],
}

const _: () = assert!(core::mem::size_of::<SigEvent>() == 64);

fn result(r: Result<u64, u64>) -> SyscallResult {
    match r {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// mq_open(name, oflag, mode, attr): a descriptor for the queue `name`
/// (without the leading slash the C library strips). The descriptor is
/// close-on-exec whatever `oflag` says, as on Linux; `mode` and `attr` are used only when
/// O_CREAT creates the queue.
pub fn sys_mq_open(name: u64, oflag: u32, mode: u32, attr: u64) -> SyscallResult {
    result(mq_open(name, oflag, mode, attr))
}

fn mq_open(name: u64, oflag: u32, mode: u32, attr: u64) -> Result<u64, u64> {
    let name = copy_cstr_from_user(name)?;
    store::check_name(name.as_bytes())?;
    let attr = if oflag & O_CREAT != 0 && attr != 0 {
        Some(copy_from_user(attr as *const MqAttr).map_err(|_| EFAULT as u64)?)
    } else {
        None
    };
    let cred = Credentials::current(false);
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;

    // Hold a descriptor slot first, so a full table refuses the open before
    // a queue is created.
    let slot = crate::process::with_process_manager(|manager| {
        let (_, process) = manager.find_process_by_thread_mut(tid)?;
        process.fd_table.reserve_slot()
    })
    .flatten()
    .ok_or(EMFILE as u64)?;

    let queue = match super::open(&name, oflag, mode, attr, &cred) {
        Ok(queue) => queue,
        Err(e) => {
            crate::process::with_process_manager(|manager| {
                if let Some((_, process)) = manager.find_process_by_thread_mut(tid) {
                    process.fd_table.release_reserved(slot);
                }
            });
            return Err(e);
        }
    };
    let file: Arc<dyn IpcFile> = Arc::new(MqFile(queue));
    let entry = FileDescriptor::opened(FdKind::Ipc(file), oflag | status_flags::O_CLOEXEC);
    crate::process::with_process_manager(|manager| {
        let (_, process) = manager.find_process_by_thread_mut(tid).ok_or(ESRCH as u64)?;
        process.fd_table.install_reserved(slot, entry).map_err(|_| EMFILE as u64)
    })
    .ok_or(ESRCH as u64)?
    .map(|fd| fd as u64)
}

/// mq_unlink(name): remove the name; open descriptors keep the queue.
pub fn sys_mq_unlink(name: u64) -> SyscallResult {
    result(mq_unlink(name))
}

fn mq_unlink(name: u64) -> Result<u64, u64> {
    let name = copy_cstr_from_user(name)?;
    store::check_name(name.as_bytes())?;
    super::unlink(&name, &Credentials::current(false))?;
    Ok(0)
}

/// A message queue descriptor of the calling process, with what a call on
/// it needs of the process.
struct Target {
    queue: Arc<MessageQueue>,
    /// A copy of the descriptor: its description word is the descriptor's.
    fd: FileDescriptor,
    /// The process's thread group ID and real user ID.
    tgid: u64,
    uid: u32,
}

/// Descriptor `mqd` of the calling process: EBADF if it is not open or not
/// a message queue.
fn target(mqd: i32) -> Result<Target, u64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    let guard = crate::process::manager();
    let (pid, process) = guard
        .as_ref()
        .and_then(|manager| manager.find_process_by_thread(tid))
        .ok_or(ESRCH as u64)?;
    let fd = process.fd_table.get(mqd).ok_or(EBADF as u64)?;
    let FdKind::Ipc(file) = &fd.kind else {
        return Err(EBADF as u64);
    };
    let queue = file.as_any().downcast_ref::<MqFile>().ok_or(EBADF as u64)?.0.clone();
    Ok(Target {
        queue,
        fd: fd.clone(),
        tgid: process.thread_group_id.unwrap_or(pid.as_u64()),
        uid: process.cred.uid,
    })
}

fn nonblocking(fd: &FileDescriptor) -> bool {
    fd.status_flags() & status_flags::O_NONBLOCK != 0
}

/// The absolute CLOCK_REALTIME deadline at `ts`. An unreadable timespec is
/// EFAULT at once; a field out of range stays in the result, reported only
/// if the call would block.
fn deadline(ts: u64) -> Result<Result<wait::IpcDeadline, u64>, u64> {
    match wait::abs_realtime(ts) {
        Err(e) if e == EFAULT as u64 => Err(e),
        d => Ok(d),
    }
}

/// mq_timedsend(mqd, msg, len, prio, abs_timeout).
pub fn sys_mq_timedsend(mqd: i32, msg: u64, len: u64, prio: u32, ts: u64) -> SyscallResult {
    result(mq_timedsend(mqd, msg, len, prio, ts))
}

fn mq_timedsend(mqd: i32, msg: u64, len: u64, prio: u32, ts: u64) -> Result<u64, u64> {
    if prio >= MQ_PRIO_MAX {
        return Err(EINVAL as u64);
    }
    let t = target(mqd)?;
    if !t.fd.writable() {
        return Err(EBADF as u64);
    }
    let len = len as usize;
    store::check_send(len, prio, t.queue.msgsize())?;
    let deadline = deadline(ts)?;
    let mut data = Vec::new();
    data.try_reserve_exact(len).map_err(|_| ENOMEM as u64)?;
    data.resize(len, 0);
    read_user_bytes(data.as_mut_ptr(), msg, len)?;
    t.queue.send(data, prio, nonblocking(&t.fd), &deadline, (t.tgid, t.uid))?;
    Ok(0)
}

/// mq_timedreceive(mqd, msg, len, prio, abs_timeout): the message's length.
/// A message whose copy-out faults is lost, as on Linux.
pub fn sys_mq_timedreceive(mqd: i32, msg: u64, len: u64, prio: u64, ts: u64) -> SyscallResult {
    result(mq_timedreceive(mqd, msg, len, prio, ts))
}

fn mq_timedreceive(mqd: i32, msg: u64, len: u64, prio: u64, ts: u64) -> Result<u64, u64> {
    let t = target(mqd)?;
    if !t.fd.readable() {
        return Err(EBADF as u64);
    }
    store::check_receive(len as usize, t.queue.msgsize())?;
    let deadline = deadline(ts)?;
    let (data, msg_prio) = t.queue.receive(nonblocking(&t.fd), &deadline)?;
    write_user_bytes(msg, data.as_ptr(), data.len())?;
    if prio != 0 {
        copy_to_user(prio as *mut u32, &msg_prio)?;
    }
    Ok(data.len() as u64)
}

/// mq_notify(mqd, sevp): register the calling process to be told when a
/// message arrives on the empty queue, or with a null `sevp` remove its
/// registration. SIGEV_NONE and SIGEV_SIGNAL are supported; SIGEV_THREAD is
/// the C library's to build, and like any other kind is EINVAL here.
pub fn sys_mq_notify(mqd: i32, sevp: u64) -> SyscallResult {
    result(mq_notify(mqd, sevp))
}

fn mq_notify(mqd: i32, sevp: u64) -> Result<u64, u64> {
    let request = if sevp == 0 {
        None
    } else {
        let ev = copy_from_user(sevp as *const SigEvent).map_err(|_| EFAULT as u64)?;
        let kind = match ev.notify {
            SIGEV_NONE => NotifyKind::Nothing,
            SIGEV_SIGNAL => NotifyKind::Signal,
            _ => return Err(EINVAL as u64),
        };
        if kind == NotifyKind::Signal && !(0..=crate::signal::constants::NSIG as i32).contains(&ev.signo) {
            return Err(EINVAL as u64);
        }
        Some((kind, ev.signo as u32, ev.value))
    };
    let t = target(mqd)?;
    let request = request.map(|(kind, signo, value)| MqNotify { pid: t.tgid, signo, value, kind });
    t.queue.set_notify(t.tgid, request)?;
    Ok(0)
}

/// mq_getsetattr(mqd, new, old): store the attributes in `old` and, with a
/// `new`, set the descriptor's O_NONBLOCK from its mq_flags. Any other flag
/// is EINVAL; the other fields of `new` are ignored.
pub fn sys_mq_getsetattr(mqd: i32, new: u64, old: u64) -> SyscallResult {
    result(mq_getsetattr(mqd, new, old))
}

fn mq_getsetattr(mqd: i32, new: u64, old: u64) -> Result<u64, u64> {
    let nonblock = if new != 0 {
        let attr = copy_from_user(new as *const MqAttr).map_err(|_| EFAULT as u64)?;
        Some(store::check_setattr(&attr)?)
    } else {
        None
    };
    let t = target(mqd)?;
    let attr = t.queue.attr(nonblocking(&t.fd));
    if let Some(nonblock) = nonblock {
        let kept = t.fd.status_flags() & status_flags::O_APPEND;
        t.fd.set_status_flags(kept | if nonblock { status_flags::O_NONBLOCK } else { 0 });
    }
    if old != 0 {
        copy_to_user(old as *mut MqAttr, &attr)?;
    }
    Ok(0)
}
