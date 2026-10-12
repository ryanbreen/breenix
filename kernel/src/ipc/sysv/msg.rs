//! System V message queues: msgget, msgsnd, msgrcv and msgctl.

use super::state::{
    self, try_vec, IpcIds, IpcPerm, Message, MsgQueue, Select, IPC_64, IPC_INFO, IPC_NOWAIT, IPC_RMID, IPC_SET, IPC_STAT,
    MSGMAX, MSGMNB, MSGMNI, MSG_INFO, MSG_NOERROR, MSG_STAT, MSG_STAT_ANY, MSQID_DS_SIZE, READ, WRITE,
};
use super::{caller, interrupted_unless_handled, ipcperms, now, Waiters};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::ipc::wait::{IpcDeadline, WaitEnd};
use crate::syscall::errno::{EACCES, EAGAIN, EFAULT, EIDRM, EINVAL, ENOMEM, ENOMSG, EPERM};
use crate::syscall::userptr::{read_user_bytes, write_user_bytes};
use crate::syscall::SyscallResult;
use alloc::sync::Arc;

pub(crate) struct Msq {
    q: MsgQueue,
    waiters: Arc<Waiters>,
}

pub(crate) static SYSV_MSG: IrqSafeMutex<IpcIds<Msq>> = IrqSafeMutex::new(IpcIds::new(MSGMNI));

fn err(e: i32) -> SyscallResult {
    SyscallResult::Err(e as u64)
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
            let Ok(waiters) = Waiters::new() else { return err(ENOMEM) };
            match ids.insert(perm, Msq { q: MsgQueue::new(t), waiters }) {
                Ok(id) => SyscallResult::Ok(id as u64),
                Err(e) => err(e),
            }
        }
    }
}

/// msgsnd: queue the message at `msgp`, a long type and `msgsz` bytes of
/// text, waiting for room unless IPC_NOWAIT.
pub fn sys_msgsnd(id: i32, msgp: u64, msgsz: u64, flags: u32) -> SyscallResult {
    // Linux reads the type before it checks the arguments, so a bad buffer
    // is EFAULT even with a bad identifier or size.
    let mut mtype = [0u8; 8];
    if read_user_bytes(mtype.as_mut_ptr(), msgp, 8).is_err() {
        return err(EFAULT);
    }
    if msgsz > MSGMAX as u64 || id < 0 {
        return err(EINVAL);
    }
    let mtype = i64::from_ne_bytes(mtype);
    if mtype < 1 {
        return err(EINVAL);
    }
    let mut text = match try_vec(msgsz as usize, 0u8) {
        Ok(text) => text,
        Err(e) => return err(e),
    };
    if read_user_bytes(text.as_mut_ptr(), msgp.wrapping_add(8), text.len()).is_err() {
        return err(EFAULT);
    }
    let me = caller();
    let mut msg = Some(Message { mtype, text });
    let mut slept: Option<Arc<Waiters>> = None;
    loop {
        let t = now();
        let mut ids = SYSV_MSG.lock();
        if Waiters::gone(&slept, ids.get(id).map(|e| &e.obj.waiters)) {
            return err(EIDRM);
        }
        let Some(entry) = ids.get_mut(id) else { return err(EINVAL) };
        if !ipcperms(&entry.perm, &me.cred, WRITE) {
            return err(EACCES);
        }
        let len = msg.as_ref().map_or(0, |m| m.text.len());
        if entry.obj.q.has_room(len) {
            if let Some(m) = msg.take() {
                if let Err(e) = entry.obj.q.push(m, me.pid, t) {
                    return err(e);
                }
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
        if let WaitEnd::Interrupted(e) = w.wait(seen, &IpcDeadline::Never) {
            return err(interrupted_unless_handled(e));
        }
        slept = Some(w);
    }
}

/// msgrcv: take the message `msgtyp` names into `msgp`, at most `msgsz`
/// bytes of text, waiting for one unless IPC_NOWAIT. The text's length.
/// Linux's MSG_EXCEPT and MSG_COPY select as `state::select` reads them.
pub fn sys_msgrcv(id: i32, msgp: u64, msgsz: u64, msgtyp: i64, flags: u32) -> SyscallResult {
    if (msgsz as i64) < 0 || id < 0 {
        return err(EINVAL);
    }
    let select = match state::select(msgtyp, flags) {
        Ok(select) => select,
        Err(e) => return err(e),
    };
    // MSG_COPY reads the buffer, up to MSGMAX bytes, before the queue is
    // looked up, as Linux does when it prepares the copy: an unreadable
    // buffer is EFAULT whatever the queue holds.
    if let Select::Copy(_) = select {
        let mut chunk = [0u8; 256];
        let len = msgsz.min(MSGMAX as u64);
        let mut at = 0;
        while at < len {
            let n = (len - at).min(chunk.len() as u64);
            if read_user_bytes(chunk.as_mut_ptr(), msgp.wrapping_add(at), n as usize).is_err() {
                return err(EFAULT);
            }
            at += n;
        }
    }
    let me = caller();
    let mut slept: Option<Arc<Waiters>> = None;
    let msg = loop {
        let t = now();
        let mut ids = SYSV_MSG.lock();
        if Waiters::gone(&slept, ids.get(id).map(|e| &e.obj.waiters)) {
            return err(EIDRM);
        }
        let Some(entry) = ids.get_mut(id) else { return err(EINVAL) };
        if !ipcperms(&entry.perm, &me.cred, READ) {
            return err(EACCES);
        }
        match entry.obj.q.receive(select, msgsz as usize, flags & MSG_NOERROR != 0, me.pid, t) {
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
        if let WaitEnd::Interrupted(e) = w.wait(seen, &IpcDeadline::Never) {
            return err(interrupted_unless_handled(e));
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

/// msgctl: IPC_STAT, IPC_SET and IPC_RMID, and Linux's IPC_INFO, MSG_INFO,
/// MSG_STAT and MSG_STAT_ANY.
pub fn sys_msgctl(id: i32, cmd: u32, buf: u64) -> SyscallResult {
    if id < 0 {
        return err(EINVAL);
    }
    let me = caller();
    let cmd = cmd & !IPC_64;
    match cmd {
        IPC_STAT | MSG_STAT | MSG_STAT_ANY => {
            let (ds, result) = {
                let ids = SYSV_MSG.lock();
                // MSG_STAT and MSG_STAT_ANY take an index into the table and
                // return the identifier of the queue there; MSG_STAT_ANY
                // skips the read permission check.
                let found = match cmd {
                    IPC_STAT => ids.get(id).map(|entry| (id, entry)),
                    _ => ids.at_index(id),
                };
                let Some((qid, entry)) = found else { return err(EINVAL) };
                if cmd != MSG_STAT_ANY && !ipcperms(&entry.perm, &me.cred, READ) {
                    return err(EACCES);
                }
                let result = if cmd == IPC_STAT { 0 } else { qid as u64 };
                (state::msqid_ds(&entry.perm, &entry.obj.q), result)
            };
            match write_user_bytes(buf, ds.as_ptr(), ds.len()) {
                Ok(()) => SyscallResult::Ok(result),
                Err(_) => err(EFAULT),
            }
        }
        IPC_INFO | MSG_INFO => {
            let (info, max) = {
                let ids = SYSV_MSG.lock();
                let usage = (cmd == MSG_INFO).then(|| {
                    let (msgs, bytes) =
                        ids.iter().fold((0, 0), |(n, b), (_, e)| (n + e.obj.q.qnum(), b + e.obj.q.cbytes));
                    (ids.in_use(), msgs, bytes)
                });
                (state::msginfo(usage), ids.highest_index())
            };
            match write_user_bytes(buf, info.as_ptr(), info.len()) {
                Ok(()) => SyscallResult::Ok(max as u64),
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
            // CAP_SYS_RESOURCE does on Linux; any owner may set it at or
            // below the default, where POSIX asks privilege to raise it at
            // all. Linux's rule.
            if qbytes > MSGMNB as u64 && me.cred.euid != 0 {
                return err(EPERM);
            }
            if let Err(e) = entry.perm.set(uid, gid, mode) {
                return err(e);
            }
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
