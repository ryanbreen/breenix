//! System V semaphore sets: semget, semop, semtimedop and semctl, and the
//! SEM_UNDO adjustments applied when a task leaves its undo list.

use super::state::{
    self, try_vec, Attempt, IpcIds, IpcPerm, SemLayout, SemOp, SemSet, SemWaiter, UndoLists, GETALL, GETNCNT, GETPID,
    GETVAL, GETZCNT, IPC_64, IPC_INFO, IPC_RMID, IPC_SET, IPC_STAT, READ, SEMMNI, SEMMSL, SEMOPM, SEM_INFO, SEM_STAT,
    SEM_STAT_ANY, SEM_UNDO, SETALL, SETVAL, WRITE,
};
use super::{caller, ipcperms, now, Caller, Waiters};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::ipc::wait::{IpcDeadline, WaitEnd};
use crate::syscall::errno::{E2BIG, EACCES, EAGAIN, EFAULT, EFBIG, EIDRM, EINTR, EINVAL, ENOMEM, EPERM, ERANGE};
use crate::syscall::userptr::{read_user_bytes, write_user_bytes};
use crate::syscall::SyscallResult;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub(crate) struct Sems {
    pub(crate) set: SemSet,
    pub(crate) waiters: Arc<Waiters>,
}

/// Every semaphore set, and every task's SEM_UNDO adjustments.
pub(crate) struct SemTable {
    pub(crate) ids: IpcIds<Sems>,
    pub(crate) undo: UndoLists,
}

pub(crate) static SYSV_SEM: IrqSafeMutex<SemTable> =
    IrqSafeMutex::new(SemTable { ids: IpcIds::new(SEMMNI), undo: UndoLists::new() });

fn err(e: i32) -> SyscallResult {
    SyscallResult::Err(e as u64)
}

/// semget: the set `key` names, made with `nsems` semaphores when IPC_CREAT
/// asks for it. For an existing set, asking for more semaphores than it has
/// is EINVAL before the permission check, as on Linux.
pub fn sys_semget(key: i32, nsems: i32, flags: u32) -> SyscallResult {
    if nsems < 0 || nsems as usize > SEMMSL {
        return err(EINVAL);
    }
    let me = caller();
    let t = now();
    let mut table = SYSV_SEM.lock();
    match table.ids.resolve(key, flags) {
        Err(e) => err(e),
        Ok(Some(id)) => match table.ids.get(id) {
            Some(entry) if nsems as usize > entry.obj.set.nsems() => err(EINVAL),
            Some(entry) if !ipcperms(&entry.perm, &me.cred, state::requested(flags)) => err(EACCES),
            _ => SyscallResult::Ok(id as u64),
        },
        Ok(None) if nsems == 0 => err(EINVAL),
        Ok(None) => {
            let perm = IpcPerm::new(key, me.cred.euid, me.cred.egid, flags);
            let sems = match (SemSet::try_new(nsems as usize, t), Waiters::new()) {
                (Ok(set), Ok(waiters)) => Sems { set, waiters },
                _ => return err(ENOMEM),
            };
            match table.ids.insert(perm, sems) {
                Ok(id) => SyscallResult::Ok(id as u64),
                Err(e) => err(e),
            }
        }
    }
}

/// semop and semtimedop: apply the `nsops` operations at `sops` all at once,
/// waiting until they can be applied unless one that cannot has
/// IPC_NOWAIT. `timeout` is a relative timespec (semtimedop) or 0; when it
/// passes the call fails with EAGAIN, as on Linux.
pub fn sys_semtimedop(id: i32, sops: u64, nsops: u64, timeout: u64) -> SyscallResult {
    if nsops < 1 || id < 0 {
        return err(EINVAL);
    }
    if nsops > SEMOPM as u64 {
        return err(E2BIG);
    }
    let mut raw = match try_vec(nsops as usize * 6, 0u8) {
        Ok(raw) => raw,
        Err(e) => return err(e),
    };
    if read_user_bytes(raw.as_mut_ptr(), sops, raw.len()).is_err() {
        return err(EFAULT);
    }
    let mut ops: Vec<SemOp> = Vec::new();
    if ops.try_reserve_exact(nsops as usize).is_err() {
        return err(ENOMEM);
    }
    ops.extend(raw.chunks_exact(6).map(|b| SemOp::from_bytes([b[0], b[1], b[2], b[3], b[4], b[5]])));
    let deadline = match crate::ipc::wait::relative(timeout) {
        Ok(d) => d,
        Err(e) => return SyscallResult::Err(e),
    };
    let undoes = ops.iter().any(|o| o.flg & SEM_UNDO != 0);
    // An array that changes a value needs only write permission, even when
    // it also waits for zero, which POSIX asks read for: Linux's rule.
    let wanted = if ops.iter().any(|o| o.op != 0) { WRITE } else { READ };
    let me = caller();
    let result = semop_as(&me, id, &ops, undoes, wanted, &deadline);
    if undoes {
        super::semop_finished(me.row, me.pid);
    }
    result
}

fn semop_as(me: &Caller, id: i32, ops: &[SemOp], undoes: bool, wanted: u32, deadline: &IpcDeadline) -> SyscallResult {
    let max = ops.iter().map(|o| usize::from(o.num)).max().unwrap_or(0);
    let tid = crate::task::scheduler::current_thread_id().unwrap_or(0);
    let mut slept: Option<Arc<Waiters>> = None;
    // Once the deadline has passed the operations are tried once more, so
    // that a wake that came before it is not lost to a late resume.
    let mut timed_out = false;
    loop {
        let t = now();
        let mut guard = SYSV_SEM.lock();
        let SemTable { ids, undo } = &mut *guard;
        let Some(entry) = ids.get_mut(id) else {
            return err(if slept.as_ref().is_some_and(|w| w.is_removed()) { EIDRM } else { EINVAL });
        };
        let nsems = entry.obj.set.nsems();
        if max >= nsems {
            return err(EFBIG);
        }
        if !ipcperms(&entry.perm, &me.cred, wanted) {
            return err(EACCES);
        }
        let attempt = if undoes {
            let list = match undo.join(me.row) {
                Ok(list) => list,
                Err(e) => return err(e),
            };
            match undo.adjustments(list, id, nsems) {
                Ok(adj) => entry.obj.set.attempt(ops, adj, me.pid, t),
                Err(e) => return err(e),
            }
        } else {
            entry.obj.set.attempt(ops, &mut [], me.pid, t)
        };
        let block = match attempt {
            Err(e) => return err(e),
            Ok(Attempt::Done) => {
                let w = entry.obj.waiters.clone();
                w.note_change();
                drop(guard);
                w.wake();
                return SyscallResult::Ok(0);
            }
            Ok(Attempt::Block { nowait: true, .. }) => return err(EAGAIN),
            Ok(Attempt::Block { .. }) if timed_out => return err(EAGAIN),
            Ok(Attempt::Block { num, zero, .. }) => SemWaiter { tid, num, zero },
        };
        entry.obj.set.waiters.retain(|w| w.tid != tid);
        if state::try_push(&mut entry.obj.set.waiters, block).is_err() {
            return err(ENOMEM);
        }
        let w = entry.obj.waiters.clone();
        let seen = w.seen();
        drop(guard);
        let end = w.wait(seen, deadline);
        if let Some(entry) = SYSV_SEM.lock().ids.get_mut(id) {
            entry.obj.set.waiters.retain(|w| w.tid != tid);
        }
        match end {
            WaitEnd::Interrupted => return err(EINTR),
            WaitEnd::TimedOut => timed_out = true,
            WaitEnd::Woken => {}
        }
        slept = Some(w);
    }
}

/// What semctl copies out, and its result.
enum Reply {
    Value(u64),
    Stat([u8; 104], usize, u64),
    Bytes(Vec<u8>),
}

/// semctl: the IPC commands, GETVAL, GETPID, GETNCNT, GETZCNT, GETALL,
/// SETVAL and SETALL, and Linux's IPC_INFO, SEM_INFO, SEM_STAT and
/// SEM_STAT_ANY. `arg` is union semun as a register: the value for SETVAL,
/// else a pointer.
pub fn sys_semctl(id: i32, num: i32, cmd: u32, arg: u64) -> SyscallResult {
    if id < 0 {
        return err(EINVAL);
    }
    let cmd = cmd & !IPC_64;
    let me = caller();
    let layout = SemLayout::NATIVE;
    let in_set = |set: &SemSet| num >= 0 && (num as usize) < set.nsems();
    match cmd {
        IPC_SET => {
            let mut ds = [0u8; 104];
            let ds = &mut ds[..layout.size()];
            if read_user_bytes(ds.as_mut_ptr(), arg, ds.len()).is_err() {
                return err(EFAULT);
            }
            let (uid, gid, mode) = state::perm_set_fields(ds);
            let t = now();
            let mut table = SYSV_SEM.lock();
            let Some(entry) = table.ids.get_mut(id) else { return err(EINVAL) };
            if !entry.perm.owned_by(me.cred.euid) {
                return err(EPERM);
            }
            if let Err(e) = entry.perm.set(uid, gid, mode) {
                return err(e);
            }
            entry.obj.set.ctime = t;
            SyscallResult::Ok(0)
        }
        IPC_RMID => {
            let mut table = SYSV_SEM.lock();
            let Some(entry) = table.ids.get(id) else { return err(EINVAL) };
            if !entry.perm.owned_by(me.cred.euid) {
                return err(EPERM);
            }
            let removed = table.ids.remove(id).map(|entry| entry.obj.waiters);
            table.undo.drop_set(id);
            if let Some(w) = &removed {
                w.note_removed();
            }
            drop(table);
            if let Some(w) = removed {
                w.wake();
            }
            SyscallResult::Ok(0)
        }
        SETVAL => {
            // Linux checks the value, then the set, the semaphore number and
            // the permission.
            let val = arg as i32;
            if !state::in_range(val) {
                return err(ERANGE);
            }
            let t = now();
            let mut guard = SYSV_SEM.lock();
            let SemTable { ids, undo } = &mut *guard;
            let Some(entry) = ids.get_mut(id) else { return err(EINVAL) };
            if !in_set(&entry.obj.set) {
                return err(EINVAL);
            }
            if !ipcperms(&entry.perm, &me.cred, WRITE) {
                return err(EACCES);
            }
            entry.obj.set.set_val(num as usize, val, me.pid, t);
            undo.clear(id, Some(num as usize));
            let w = entry.obj.waiters.clone();
            w.note_change();
            drop(guard);
            w.wake();
            SyscallResult::Ok(0)
        }
        SETALL => {
            let (nsems, w) = {
                let table = SYSV_SEM.lock();
                let Some(entry) = table.ids.get(id) else { return err(EINVAL) };
                if !ipcperms(&entry.perm, &me.cred, WRITE) {
                    return err(EACCES);
                }
                (entry.obj.set.nsems(), entry.obj.waiters.clone())
            };
            let (mut raw, mut vals) = match (try_vec(nsems * 2, 0u8), try_vec(nsems, 0u16)) {
                (Ok(raw), Ok(vals)) => (raw, vals),
                _ => return err(ENOMEM),
            };
            if read_user_bytes(raw.as_mut_ptr(), arg, raw.len()).is_err() {
                return err(EFAULT);
            }
            for (v, b) in vals.iter_mut().zip(raw.chunks_exact(2)) {
                *v = u16::from_ne_bytes([b[0], b[1]]);
            }
            let t = now();
            let mut guard = SYSV_SEM.lock();
            let SemTable { ids, undo } = &mut *guard;
            let Some(entry) = ids.get_mut(id).filter(|e| Arc::ptr_eq(&e.obj.waiters, &w)) else {
                return err(EIDRM);
            };
            if let Err(e) = entry.obj.set.set_all(&vals, me.pid, t) {
                return err(e);
            }
            undo.clear(id, None);
            w.note_change();
            drop(guard);
            w.wake();
            SyscallResult::Ok(0)
        }
        IPC_INFO | SEM_INFO => {
            let (info, max) = {
                let table = SYSV_SEM.lock();
                let usage = (cmd == SEM_INFO)
                    .then(|| (table.ids.in_use(), table.ids.iter().map(|(_, e)| e.obj.set.nsems()).sum()));
                (state::seminfo(usage), table.ids.highest_index())
            };
            match write_user_bytes(arg, info.as_ptr(), info.len()) {
                Ok(()) => SyscallResult::Ok(max as u64),
                Err(_) => err(EFAULT),
            }
        }
        IPC_STAT | SEM_STAT | SEM_STAT_ANY | GETVAL | GETPID | GETNCNT | GETZCNT | GETALL => {
            let reply = {
                let table = SYSV_SEM.lock();
                // SEM_STAT and SEM_STAT_ANY take an index into the table and
                // return the identifier of the set there; SEM_STAT_ANY skips
                // the read permission check.
                let found = match cmd {
                    SEM_STAT | SEM_STAT_ANY => table.ids.at_index(id),
                    _ => table.ids.get(id).map(|entry| (id, entry)),
                };
                let Some((sid, entry)) = found else { return err(EINVAL) };
                if cmd != SEM_STAT_ANY && !ipcperms(&entry.perm, &me.cred, READ) {
                    return err(EACCES);
                }
                let set = &entry.obj.set;
                if !matches!(cmd, IPC_STAT | SEM_STAT | SEM_STAT_ANY | GETALL) && !in_set(set) {
                    return err(EINVAL);
                }
                let n = num as usize;
                match cmd {
                    IPC_STAT => Reply::Stat(state::semid_ds(&entry.perm, set, layout), layout.size(), 0),
                    SEM_STAT | SEM_STAT_ANY => {
                        Reply::Stat(state::semid_ds(&entry.perm, set, layout), layout.size(), sid as u64)
                    }
                    GETALL => {
                        let mut bytes = match try_vec(set.nsems() * 2, 0u8) {
                            Ok(bytes) => bytes,
                            Err(e) => return err(e),
                        };
                        for (b, v) in bytes.chunks_exact_mut(2).zip(&set.vals) {
                            b.copy_from_slice(&v.to_ne_bytes());
                        }
                        Reply::Bytes(bytes)
                    }
                    GETVAL => Reply::Value(u64::from(set.vals[n])),
                    GETPID => Reply::Value(set.pids[n] as u64),
                    GETNCNT => Reply::Value(set.counts(n).0 as u64),
                    _ => Reply::Value(set.counts(n).1 as u64),
                }
            };
            let (bytes, result): (&[u8], u64) = match &reply {
                Reply::Value(v) => return SyscallResult::Ok(*v),
                Reply::Stat(ds, len, result) => (&ds[..*len], *result),
                Reply::Bytes(b) => (b, 0),
            };
            match write_user_bytes(arg, bytes.as_ptr(), bytes.len()) {
                Ok(()) => SyscallResult::Ok(result),
                Err(_) => err(EFAULT),
            }
        }
        _ => err(EINVAL),
    }
}
