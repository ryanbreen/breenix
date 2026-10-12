//! The message queue system calls: mq_open, mq_unlink, mq_timedsend,
//! mq_timedreceive, mq_notify and mq_getsetattr, with Linux's structures.
//!
//! Each call copies its arguments in from user memory, looks the descriptor
//! up under PROCESS_MANAGER, then works on the queue with PM released, and
//! copies results out with no queue lock held.

use super::store::{self, MqAttr, MQ_PRIO_MAX, NAME_MAX, PATH_MAX};
use super::{Caller, Creator, MessageQueue, MqFile, MqNotify, NotifyKind};
use crate::fs::permissions::Credentials;
use crate::ipc::fd::{status_flags, FdKind, FileDescriptor};
use crate::ipc::object::{close_owner, IpcFile};
use crate::ipc::wait::IpcDeadline;
use crate::process::ProcessId;
use crate::signal::timers::{SIGEV_NONE, SIGEV_SIGNAL, SIGEV_THREAD};
use crate::syscall::errno::{EBADF, EFAULT, EINVAL, EMFILE, ENAMETOOLONG, ENOMEM, ESRCH};
use crate::syscall::userptr::{copy_from_user, copy_to_user, read_user_bytes, write_user_bytes};
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

/// struct timespec on both 64-bit Linux ABIs.
#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

fn result(r: Result<u64, u64>) -> SyscallResult {
    match r {
        Ok(value) => SyscallResult::Ok(value),
        Err(errno) => SyscallResult::Err(errno),
    }
}

/// A queue name read from user memory: a byte string, not text.
struct Name {
    bytes: [u8; NAME_MAX],
    len: usize,
}

impl Name {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Read and check the name at `ptr` without allocating: EFAULT for memory
/// that cannot be read before its terminator, ENAMETOOLONG for no
/// terminator in PATH_MAX bytes, then `store::check_scanned`.
fn read_name(ptr: u64) -> Result<Name, u64> {
    let mut name = Name { bytes: [0; NAME_MAX], len: 0 };
    let mut slash = false;
    for offset in 0..PATH_MAX {
        let addr = ptr
            .checked_add(offset as u64)
            .filter(|&addr| ptr != 0 && crate::memory::layout::is_valid_user_range(addr, 1))
            .ok_or(EFAULT as u64)?;
        let mut byte = 0u8;
        read_user_bytes(&mut byte, addr, 1)?;
        if byte == 0 {
            store::check_scanned(&name.bytes[..offset.min(NAME_MAX)], offset, slash)?;
            name.len = offset;
            return Ok(name);
        }
        slash |= byte == b'/';
        if offset < NAME_MAX {
            name.bytes[offset] = byte;
        }
    }
    Err(ENAMETOOLONG as u64)
}

/// The calling thread's ID and scheduling rank.
fn caller() -> Result<Caller, u64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    let rank = crate::task::scheduler::with_scheduler(|sched| sched.current_thread_mut().map(|t| t.sched.rank()))
        .flatten()
        .unwrap_or(0);
    Ok(Caller { tid, rank })
}

/// mq_open(name, oflag, mode, attr): a descriptor for the queue `name`
/// (without the leading slash the C library strips). The descriptor is
/// close-on-exec whatever `oflag` says, as on Linux; `mode` and `attr` are used only when
/// O_CREAT creates the queue, which is charged to the caller's real user.
pub fn sys_mq_open(name: u64, oflag: u32, mode: u32, attr: u64) -> SyscallResult {
    result(mq_open(name, oflag, mode, attr))
}

fn mq_open(name: u64, oflag: u32, mode: u32, attr: u64) -> Result<u64, u64> {
    // Linux reads the attributes first, whatever oflag says, and uses them
    // only when O_CREAT creates the queue.
    let attr = if attr != 0 {
        Some(copy_from_user(attr as *const MqAttr).map_err(|_| EFAULT as u64)?)
    } else {
        None
    };
    let attr = attr.filter(|_| oflag & O_CREAT != 0);
    let name = read_name(name)?;
    let cred = Credentials::current(false);
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;

    // Hold a descriptor slot first, so a full table refuses the open before
    // a queue is created.
    let (slot, creator) = crate::process::with_process_manager(|manager| {
        let (_, process) = manager.find_process_by_thread_mut(tid)?;
        let creator = Creator {
            uid: process.cred.uid,
            limit: process.limits.get(crate::process::limits::MSGQUEUE).soft,
        };
        process.fd_table.reserve_slot().map(|slot| (slot, creator))
    })
    .flatten()
    .ok_or(EMFILE as u64)?;

    let release = || {
        crate::process::with_process_manager(|manager| {
            if let Some((_, process)) = manager.find_process_by_thread_mut(tid) {
                process.fd_table.release_reserved(slot);
            }
        });
    };
    let file = super::open(name.as_bytes(), oflag, mode, attr, &cred, creator)
        .and_then(|queue| Arc::try_new(MqFile(queue)).map_err(|_| ENOMEM as u64));
    let file: Arc<dyn IpcFile> = match file {
        Ok(file) => file,
        Err(e) => {
            release();
            return Err(e);
        }
    };
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
    let name = read_name(name)?;
    super::unlink(name.as_bytes(), &Credentials::current(false))?;
    Ok(0)
}

/// A message queue descriptor of the calling process, with what a call on
/// it needs of the process.
struct Target {
    queue: Arc<MessageQueue>,
    /// A copy of the descriptor: its description word is the descriptor's.
    fd: FileDescriptor,
    /// The process's thread group ID, its program image and its real user
    /// ID.
    tgid: u64,
    exec_id: u64,
    uid: u32,
}

/// Descriptor `mqd` of the calling process: EBADF if it is not open or not
/// a message queue.
fn target(mqd: i32) -> Result<Target, u64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    let guard = crate::process::manager();
    let manager = guard.as_ref().ok_or(ESRCH as u64)?;
    let (pid, process) = manager.find_process_by_thread(tid).ok_or(ESRCH as u64)?;
    let fd = process.fd_table.get(mqd).ok_or(EBADF as u64)?;
    let FdKind::Ipc(file) = &fd.kind else {
        return Err(EBADF as u64);
    };
    let queue = file.as_any().downcast_ref::<MqFile>().ok_or(EBADF as u64)?.0.clone();
    // The group's own row carries its image, whichever thread calls; a
    // delivery checks the same row (`send_kernel_signal`).
    let tgid = close_owner(pid, process.thread_group_id).as_u64();
    let exec_id = manager.get_process(ProcessId::new(tgid)).map_or(process.exec_id, |p| p.exec_id);
    Ok(Target { queue, fd: fd.clone(), tgid, exec_id, uid: process.cred.uid })
}

fn nonblocking(fd: &FileDescriptor) -> bool {
    fd.status_flags() & status_flags::O_NONBLOCK != 0
}

/// The absolute CLOCK_REALTIME deadline at `ts`, checked before anything
/// else the call does, as Linux does: EFAULT for an unreadable timespec and
/// EINVAL for a field out of range (`store::check_timeout`), whether or not
/// the call would wait.
fn deadline(ts: u64) -> Result<IpcDeadline, u64> {
    if ts == 0 {
        return Ok(IpcDeadline::Never);
    }
    let t = copy_from_user(ts as *const Timespec).map_err(|_| EFAULT as u64)?;
    store::check_timeout(t.sec, t.nsec).map(IpcDeadline::Realtime)
}

/// mq_timedsend(mqd, msg, len, prio, abs_timeout).
pub fn sys_mq_timedsend(mqd: i32, msg: u64, len: u64, prio: u32, ts: u64) -> SyscallResult {
    result(mq_timedsend(mqd, msg, len, prio, ts))
}

fn mq_timedsend(mqd: i32, msg: u64, len: u64, prio: u32, ts: u64) -> Result<u64, u64> {
    let deadline = deadline(ts)?;
    if prio >= MQ_PRIO_MAX {
        return Err(EINVAL as u64);
    }
    let t = target(mqd)?;
    if !t.fd.writable() {
        return Err(EBADF as u64);
    }
    let len = len as usize;
    store::check_send(len, prio, t.queue.msgsize())?;
    let mut data = Vec::new();
    data.try_reserve_exact(len).map_err(|_| ENOMEM as u64)?;
    data.resize(len, 0);
    read_user_bytes(data.as_mut_ptr(), msg, len)?;
    t.queue.send(data, prio, nonblocking(&t.fd), &deadline, (t.tgid, t.uid), caller()?)?;
    Ok(0)
}

/// mq_timedreceive(mqd, msg, len, prio, abs_timeout): the message's length.
/// A message whose copy-out faults is lost, as on Linux.
pub fn sys_mq_timedreceive(mqd: i32, msg: u64, len: u64, prio: u64, ts: u64) -> SyscallResult {
    result(mq_timedreceive(mqd, msg, len, prio, ts))
}

fn mq_timedreceive(mqd: i32, msg: u64, len: u64, prio: u64, ts: u64) -> Result<u64, u64> {
    let deadline = deadline(ts)?;
    let t = target(mqd)?;
    if !t.fd.readable() {
        return Err(EBADF as u64);
    }
    store::check_receive(len as usize, t.queue.msgsize())?;
    let (data, msg_prio) = t.queue.receive(nonblocking(&t.fd), &deadline, caller()?)?;
    write_user_bytes(msg, data.as_ptr(), data.len())?;
    if prio != 0 {
        copy_to_user(prio as *mut u32, &msg_prio)?;
    }
    Ok(data.len() as u64)
}

/// The pipe whose write end is the calling process's descriptor `fd`, for a
/// SIGEV_THREAD registration: EBADF if `fd` is not open, EINVAL if it is not
/// a pipe's write end.
fn notify_pipe(fd: i32) -> Result<Arc<spin::Mutex<crate::ipc::pipe::PipeBuffer>>, u64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    crate::process::with_process_manager(|manager| {
        let (_, process) = manager.find_process_by_thread(tid).ok_or(ESRCH as u64)?;
        match &process.fd_table.get(fd).ok_or(EBADF as u64)?.kind {
            FdKind::PipeWrite(pipe) => Ok(pipe.clone()),
            _ => Err(EINVAL as u64),
        }
    })
    .ok_or(ESRCH as u64)?
}

/// mq_notify(mqd, sevp): register the calling process to be told when a
/// message arrives on the empty queue, or with a null `sevp` remove its
/// registration. SIGEV_NONE and SIGEV_SIGNAL (a signal from 1 to NSIG) are
/// as Linux has them. SIGEV_THREAD is the C library's interface to its
/// notification thread: sigev_signo is the write end of a pipe the library
/// reads, and the notification writes sigev_value to it as 8 bytes.
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
            // Signal 0 is the null signal, which is not delivered, so a
            // registration with it could not notify: EINVAL.
            SIGEV_SIGNAL if (1..=crate::signal::constants::NSIG as i32).contains(&ev.signo) => {
                NotifyKind::Signal(ev.signo as u32)
            }
            SIGEV_THREAD => NotifyKind::Thread(notify_pipe(ev.signo)?),
            _ => return Err(EINVAL as u64),
        };
        Some((kind, ev.value))
    };
    let t = target(mqd)?;
    let request = request.map(|(kind, value)| MqNotify { pid: t.tgid, exec_id: t.exec_id, value, kind });
    t.queue.set_notify(t.tgid, request)?;
    Ok(0)
}

/// mq_getsetattr(mqd, new, old): store the attributes in `old` and, with a
/// `new`, set the descriptor's O_NONBLOCK from its mq_flags, as one step.
/// Any other flag is EINVAL; the other fields of `new` are ignored.
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
    let attr = t.queue.getsetattr(&t.fd, nonblock);
    if old != 0 {
        copy_to_user(old as *mut MqAttr, &attr)?;
    }
    Ok(0)
}
