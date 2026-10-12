//! System V semaphore sets: semget, semop, semtimedop and semctl, and the
//! SEM_UNDO adjustments applied when a process exits.

use super::state::{
    self, Attempt, IpcIds, IpcPerm, SemLayout, SemOp, SemSet, SemWaiter, UndoLists, GETALL, GETNCNT, GETPID, GETVAL,
    GETZCNT, IPC_64, IPC_RMID, IPC_SET, IPC_STAT, READ, SEMMNI, SEMMSL, SEMOPM, SEM_UNDO, SETALL, SETVAL, WRITE,
};
use super::{caller, ipcperms, now, wake_all, Waiters};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::ipc::wait::WaitEnd;
use crate::syscall::errno::{E2BIG, EACCES, EAGAIN, EFAULT, EFBIG, EIDRM, EINTR, EINVAL, EPERM};
use crate::syscall::userptr::{read_user_bytes, write_user_bytes};
use crate::syscall::SyscallResult;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

pub(crate) struct Sems {
    set: SemSet,
    waiters: Arc<Waiters>,
}

/// Every semaphore set, and every process's SEM_UNDO adjustments.
pub(crate) struct SemTable {
    ids: IpcIds<Sems>,
    undo: UndoLists,
}

pub(crate) static SYSV_SEM: IrqSafeMutex<SemTable> =
    IrqSafeMutex::new(SemTable { ids: IpcIds::new(SEMMNI), undo: UndoLists::new() });

fn err(e: i32) -> SyscallResult {
    SyscallResult::Err(e as u64)
}

/// semget: the set `key` names, made with `nsems` semaphores when IPC_CREAT
/// asks for it.
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
            Some(entry) if !ipcperms(&entry.perm, &me.cred, state::requested(flags)) => err(EACCES),
            Some(entry) if nsems as usize > entry.obj.set.nsems() => err(EINVAL),
            _ => SyscallResult::Ok(id as u64),
        },
        Ok(None) if nsems == 0 => err(EINVAL),
        Ok(None) => {
            let perm = IpcPerm::new(key, me.cred.euid, me.cred.egid, flags);
            let sems = Sems { set: SemSet::new(nsems as usize, t), waiters: Waiters::new() };
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
    let mut raw = vec![0u8; nsops as usize * 6];
    if read_user_bytes(raw.as_mut_ptr(), sops, raw.len()).is_err() {
        return err(EFAULT);
    }
    let ops: Vec<SemOp> = raw.chunks_exact(6).map(|b| SemOp::from_bytes([b[0], b[1], b[2], b[3], b[4], b[5]])).collect();
    let deadline = match crate::ipc::wait::relative(timeout) {
        Ok(d) => d,
        Err(e) => return SyscallResult::Err(e),
    };
    let max = ops.iter().map(|o| usize::from(o.num)).max().unwrap_or(0);
    let undoes = ops.iter().any(|o| o.flg & SEM_UNDO != 0);
    let wanted = if ops.iter().any(|o| o.op != 0) { WRITE } else { READ };
    let me = caller();
    let tgid = me.pid as u64;
    let tid = crate::task::scheduler::current_thread_id().unwrap_or(0);
    let mut slept: Option<Arc<Waiters>> = None;
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
        let mut adj = if undoes { undo.current(tgid, id, nsems) } else { vec![0; nsems] };
        let block = match entry.obj.set.attempt(&ops, &mut adj, me.pid, t) {
            Err(e) => return err(e),
            Ok(Attempt::Done) => {
                if undoes {
                    *undo.list(tgid, id, nsems) = adj;
                }
                let w = entry.obj.waiters.clone();
                w.note_change();
                drop(guard);
                w.wake();
                return SyscallResult::Ok(0);
            }
            Ok(Attempt::Block { nowait: true, .. }) => return err(EAGAIN),
            Ok(Attempt::Block { num, zero, .. }) => SemWaiter { tid, num, zero },
        };
        entry.obj.set.waiters.retain(|w| w.tid != tid);
        entry.obj.set.waiters.push(block);
        let w = entry.obj.waiters.clone();
        let seen = w.seen();
        drop(guard);
        let end = w.wait(seen, &deadline);
        if let Some(entry) = SYSV_SEM.lock().ids.get_mut(id) {
            entry.obj.set.waiters.retain(|w| w.tid != tid);
        }
        match end {
            WaitEnd::Interrupted => return err(EINTR),
            WaitEnd::TimedOut => return err(if w.is_removed() { EIDRM } else { EAGAIN }),
            WaitEnd::Woken => slept = Some(w),
        }
    }
}

/// What semctl returns or copies out.
enum Reply {
    Value(u64),
    Bytes(Vec<u8>),
}

/// semctl: the IPC commands and GETVAL, GETPID, GETNCNT, GETZCNT, GETALL,
/// SETVAL and SETALL. `arg` is union semun as a register: the value for
/// SETVAL, else a pointer.
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
            let mut ds = vec![0u8; layout.size()];
            if read_user_bytes(ds.as_mut_ptr(), arg, ds.len()).is_err() {
                return err(EFAULT);
            }
            let (uid, gid, mode) = state::perm_set_fields(&ds);
            let t = now();
            let mut table = SYSV_SEM.lock();
            let Some(entry) = table.ids.get_mut(id) else { return err(EINVAL) };
            if !entry.perm.owned_by(me.cred.euid) {
                return err(EPERM);
            }
            entry.perm.set(uid, gid, mode);
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
            wake_all(removed.into_iter().collect());
            SyscallResult::Ok(0)
        }
        SETVAL => {
            let t = now();
            let mut guard = SYSV_SEM.lock();
            let SemTable { ids, undo } = &mut *guard;
            let Some(entry) = ids.get_mut(id) else { return err(EINVAL) };
            if !ipcperms(&entry.perm, &me.cred, WRITE) {
                return err(EACCES);
            }
            if !in_set(&entry.obj.set) {
                return err(EINVAL);
            }
            if let Err(e) = entry.obj.set.set_val(num as usize, arg as i32, me.pid, t) {
                return err(e);
            }
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
            let mut raw = vec![0u8; nsems * 2];
            if read_user_bytes(raw.as_mut_ptr(), arg, raw.len()).is_err() {
                return err(EFAULT);
            }
            let vals: Vec<u16> = raw.chunks_exact(2).map(|b| u16::from_ne_bytes([b[0], b[1]])).collect();
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
        IPC_STAT | GETVAL | GETPID | GETNCNT | GETZCNT | GETALL => {
            let reply = {
                let table = SYSV_SEM.lock();
                let Some(entry) = table.ids.get(id) else { return err(EINVAL) };
                if !ipcperms(&entry.perm, &me.cred, READ) {
                    return err(EACCES);
                }
                let set = &entry.obj.set;
                if !matches!(cmd, IPC_STAT | GETALL) && !in_set(set) {
                    return err(EINVAL);
                }
                let n = num as usize;
                match cmd {
                    IPC_STAT => Reply::Bytes(state::semid_ds(&entry.perm, set, layout)[..layout.size()].to_vec()),
                    GETALL => Reply::Bytes(set.vals.iter().flat_map(|v| v.to_ne_bytes()).collect()),
                    GETVAL => Reply::Value(u64::from(set.vals[n])),
                    GETPID => Reply::Value(set.pids[n] as u64),
                    GETNCNT => Reply::Value(set.counts(n).0 as u64),
                    _ => Reply::Value(set.counts(n).1 as u64),
                }
            };
            match reply {
                Reply::Value(v) => SyscallResult::Ok(v),
                Reply::Bytes(b) => match write_user_bytes(arg, b.as_ptr(), b.len()) {
                    Ok(()) => SyscallResult::Ok(0),
                    Err(_) => err(EFAULT),
                },
            }
        }
        _ => err(EINVAL),
    }
}

/// Row `row` of thread group `tgid` is ending; `live` lists the group's rows
/// still running. Whether the group ends with it.
pub(crate) fn row_ending(tgid: u64, row: u64, live: &[u64]) -> bool {
    SYSV_SEM.lock().undo.row_ending(tgid, row, live)
}

/// Thread group `tgid` has exited: apply and drop its SEM_UNDO adjustments.
pub(crate) fn process_exit(tgid: u64) {
    let mut woken = Vec::new();
    {
        let mut guard = SYSV_SEM.lock();
        let SemTable { ids, undo } = &mut *guard;
        for (semid, adj) in undo.take_group(tgid) {
            if let Some(entry) = ids.get_mut(semid) {
                if entry.obj.set.apply_undo(&adj, tgid as i32) {
                    entry.obj.waiters.note_change();
                    woken.push(entry.obj.waiters.clone());
                }
            }
        }
    }
    wake_all(woken);
}
