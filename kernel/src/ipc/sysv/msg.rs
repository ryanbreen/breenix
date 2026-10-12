//! System V message queues: msgget, msgsnd, msgrcv and msgctl.

use super::state::{
    self, IpcIds, IpcPerm, Message, MsgQueue, IPC_64, IPC_NOWAIT, IPC_RMID, IPC_SET, IPC_STAT, MSGMAX, MSGMNB, MSGMNI,
    MSG_NOERROR, MSQID_DS_SIZE, READ, WRITE,
};
use super::{caller, ipcperms, now, Waiters};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::ipc::wait::{IpcDeadline, WaitEnd};
use crate::syscall::errno::{EACCES, EAGAIN, EFAULT, EIDRM, EINTR, EINVAL, ENOMSG, EPERM};
use crate::syscall::userptr::{read_user_bytes, write_user_bytes};
use crate::syscall::SyscallResult;
use alloc::sync::Arc;
use alloc::vec;

pub(crate) struct Msq {
    q: MsgQueue,
    waiters: Arc<Waiters>,
}

pub(crate) static SYSV_MSG: IrqSafeMutex<IpcIds<Msq>> = IrqSafeMutex::new(IpcIds::new(MSGMNI));

fn err(e: i32) -> SyscallResult {
    SyscallResult::Err(e as u64)
}

/// The queue `id` names, as a call that may have slept on `slept` finds it:
/// EIDRM when the queue it slept on was removed, EINVAL for any other id that
/// names nothing.
fn missing(slept: &Option<Arc<Waiters>>) -> i32 {
    if slept.as_ref().is_some_and(|w| w.is_removed()) {
        EIDRM
    } else {
        EINVAL
    }
}

/// msgget: the queue `key` names, made when IPC_CREAT asks for it.
pub fn sys_msgget(key: i32, flags: u32) -> SyscallResult {
    let me = caller();
    let t = now();
    let mut ids = SYSV_MSG.lock();
    match ids.resolve(key, flags) {
        Err(e) => err(e),
        Ok(Some(id)) => match ids.get(id) {
            Some(entry) if !ipcperms(&entry.perm, &me.cred, state::requested(flags)) => err(EACCES),
            _ => SyscallResult::Ok(id as u64),
        },
        Ok(None) => {
            let perm = IpcPerm::new(key, me.cred.euid, me.cred.egid, flags);
            match ids.insert(perm, Msq { q: MsgQueue::new(t), waiters: Waiters::new() }) {
                Ok(id) => SyscallResult::Ok(id as u64),
                Err(e) => err(e),
            }
        }
    }
}

/// msgsnd: queue the message at `msgp`, a long type and `msgsz` bytes of
/// text, waiting for room unless IPC_NOWAIT.
pub fn sys_msgsnd(id: i32, msgp: u64, msgsz: u64, flags: u32) -> SyscallResult {
    if msgsz > MSGMAX as u64 || id < 0 {
        return err(EINVAL);
    }
    let mut mtype = [0u8; 8];
    if read_user_bytes(mtype.as_mut_ptr(), msgp, 8).is_err() {
        return err(EFAULT);
    }
    let mtype = i64::from_ne_bytes(mtype);
    if mtype < 1 {
        return err(EINVAL);
    }
    let mut text = vec![0u8; msgsz as usize];
    if read_user_bytes(text.as_mut_ptr(), msgp.wrapping_add(8), text.len()).is_err() {
        return err(EFAULT);
    }
    let me = caller();
    let mut msg = Some(Message { mtype, text });
    let mut slept: Option<Arc<Waiters>> = None;
    loop {
        let t = now();
        let mut ids = SYSV_MSG.lock();
        let Some(entry) = ids.get_mut(id) else { return err(missing(&slept)) };
        if !ipcperms(&entry.perm, &me.cred, WRITE) {
            return err(EACCES);
        }
        let len = msg.as_ref().map_or(0, |m| m.text.len());
        if entry.obj.q.has_room(len) {
            if let Some(m) = msg.take() {
                entry.obj.q.push(m, me.pid, t);
            }
            let w = entry.obj.waiters.clone();
            w.note_change();
            drop(ids);
            w.wake();
            return SyscallResult::Ok(0);
        }
        if flags & IPC_NOWAIT != 0 {
            return err(EAGAIN);
        }
        let w = entry.obj.waiters.clone();
        let seen = w.seen();
        drop(ids);
        if let WaitEnd::Interrupted = w.wait(seen, &IpcDeadline::Never) {
            return err(EINTR);
        }
        slept = Some(w);
    }
}

/// msgrcv: take the message `msgtyp` names into `msgp`, at most `msgsz`
/// bytes of text, waiting for one unless IPC_NOWAIT. The text's length.
pub fn sys_msgrcv(id: i32, msgp: u64, msgsz: u64, msgtyp: i64, flags: u32) -> SyscallResult {
    if (msgsz as i64) < 0 || id < 0 {
        return err(EINVAL);
    }
    let me = caller();
    let mut slept: Option<Arc<Waiters>> = None;
    let msg = loop {
        let t = now();
        let mut ids = SYSV_MSG.lock();
        let Some(entry) = ids.get_mut(id) else { return err(missing(&slept)) };
        if !ipcperms(&entry.perm, &me.cred, READ) {
            return err(EACCES);
        }
        match entry.obj.q.receive(msgtyp, msgsz as usize, flags & MSG_NOERROR != 0, me.pid, t) {
            Err(e) => return err(e),
            Ok(Some(msg)) => {
                let w = entry.obj.waiters.clone();
                w.note_change();
                drop(ids);
                w.wake();
                break msg;
            }
            Ok(None) if flags & IPC_NOWAIT != 0 => return err(ENOMSG),
            Ok(None) => {}
        }
        let w = entry.obj.waiters.clone();
        let seen = w.seen();
        drop(ids);
        if let WaitEnd::Interrupted = w.wait(seen, &IpcDeadline::Never) {
            return err(EINTR);
        }
        slept = Some(w);
    };
    let mtype = msg.mtype.to_ne_bytes();
    if write_user_bytes(msgp, mtype.as_ptr(), 8).is_err()
        || write_user_bytes(msgp.wrapping_add(8), msg.text.as_ptr(), msg.text.len()).is_err()
    {
        return err(EFAULT);
    }
    SyscallResult::Ok(msg.text.len() as u64)
}

/// msgctl: IPC_STAT, IPC_SET and IPC_RMID.
pub fn sys_msgctl(id: i32, cmd: u32, buf: u64) -> SyscallResult {
    if id < 0 {
        return err(EINVAL);
    }
    let me = caller();
    match cmd & !IPC_64 {
        IPC_STAT => {
            let ds = {
                let ids = SYSV_MSG.lock();
                let Some(entry) = ids.get(id) else { return err(EINVAL) };
                if !ipcperms(&entry.perm, &me.cred, READ) {
                    return err(EACCES);
                }
                state::msqid_ds(&entry.perm, &entry.obj.q)
            };
            match write_user_bytes(buf, ds.as_ptr(), ds.len()) {
                Ok(()) => SyscallResult::Ok(0),
                Err(_) => err(EFAULT),
            }
        }
        IPC_SET => {
            let mut ds = [0u8; MSQID_DS_SIZE];
            if read_user_bytes(ds.as_mut_ptr(), buf, ds.len()).is_err() {
                return err(EFAULT);
            }
            let (uid, gid, mode) = state::perm_set_fields(&ds);
            let qbytes = state::msqid_qbytes(&ds);
            let t = now();
            let mut ids = SYSV_MSG.lock();
            let Some(entry) = ids.get_mut(id) else { return err(EINVAL) };
            if !entry.perm.owned_by(me.cred.euid) {
                return err(EPERM);
            }
            // Raising msg_qbytes past the default takes privilege, as
            // CAP_SYS_RESOURCE does on Linux.
            if qbytes > MSGMNB as u64 && me.cred.euid != 0 {
                return err(EPERM);
            }
            entry.perm.set(uid, gid, mode);
            entry.obj.q.qbytes = qbytes as usize;
            entry.obj.q.ctime = t;
            let w = entry.obj.waiters.clone();
            w.note_change();
            drop(ids);
            w.wake();
            SyscallResult::Ok(0)
        }
        IPC_RMID => {
            let mut ids = SYSV_MSG.lock();
            let Some(entry) = ids.get(id) else { return err(EINVAL) };
            if !entry.perm.owned_by(me.cred.euid) {
                return err(EPERM);
            }
            let removed = ids.remove(id).map(|entry| entry.obj.waiters);
            if let Some(w) = &removed {
                w.note_removed();
            }
            drop(ids);
            if let Some(w) = removed {
                w.wake();
            }
            SyscallResult::Ok(0)
        }
        _ => err(EINVAL),
    }
}
