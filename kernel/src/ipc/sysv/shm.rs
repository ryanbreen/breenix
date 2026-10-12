//! System V shared memory segments: shmget, shmat, shmdt and shmctl.
//!
//! A segment is a `ShmObject` with no name. Each attachment is a MAP_SHARED
//! mapping of it whose VMA holds a `ShmMapping`, so fork, munmap, exec and
//! exit attach and detach through the VMAs, and shm_nattch is the object's
//! count of mappings. IPC_RMID of an attached segment removes its key, marks
//! it SHM_DEST and keeps only a weak reference: the last detach frees it, and
//! the next lookup of its identifier finds it gone.

use super::state::{
    self, IpcIds, IpcPerm, ShmStat, IPC_64, IPC_RMID, IPC_SET, IPC_STAT, READ, SHMID_DS_SIZE, SHMLBA, SHMMAX, SHMMIN,
    SHMMNI, SHM_DEST, SHM_EXEC, SHM_RDONLY, SHM_RND, WRITE,
};
use super::{caller, ipcperms, now};
use crate::ipc::shmem::{self, ShmObject};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::memory::vma::Protection;
use crate::syscall::errno::{EACCES, EEXIST, EFAULT, EINVAL, EPERM};
use crate::syscall::userptr::{read_user_bytes, write_user_bytes};
use crate::syscall::SyscallResult;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

enum Backing {
    Live(Arc<ShmObject>),
    /// Removed while attached: alive only while a mapping holds it.
    Removed(Weak<ShmObject>),
}

pub(crate) struct Segment {
    backing: Backing,
    segsz: u64,
    cpid: i32,
    lpid: i32,
    atime: i64,
    dtime: i64,
    ctime: i64,
}

impl Segment {
    fn object(&self) -> Option<Arc<ShmObject>> {
        match &self.backing {
            Backing::Live(obj) => Some(obj.clone()),
            Backing::Removed(weak) => weak.upgrade(),
        }
    }

    fn is(&self, obj: &Arc<ShmObject>) -> bool {
        match &self.backing {
            Backing::Live(o) => Arc::ptr_eq(o, obj),
            Backing::Removed(weak) => core::ptr::eq(weak.as_ptr(), Arc::as_ptr(obj)),
        }
    }
}

pub(crate) static SYSV_SHM: IrqSafeMutex<IpcIds<Segment>> = IrqSafeMutex::new(IpcIds::new(SHMMNI));

fn err(e: i32) -> SyscallResult {
    SyscallResult::Err(e as u64)
}

/// The segment `id` names and its object, or EINVAL. A removed segment whose
/// last mapping is gone leaves the table here; its object, if this drops the
/// last reference, is freed by the caller after the table is released.
fn live(ids: &mut IpcIds<Segment>, id: i32) -> Result<Arc<ShmObject>, i32> {
    let entry = ids.get(id).ok_or(EINVAL)?;
    match entry.obj.object() {
        Some(obj) => Ok(obj),
        None => {
            ids.remove(id);
            Err(EINVAL)
        }
    }
}

/// shmget: the segment `key` names, made with `size` bytes of zeroed memory
/// when IPC_CREAT asks for it.
pub fn sys_shmget(key: i32, size: u64, flags: u32) -> SyscallResult {
    let me = caller();
    let existing = {
        let ids = SYSV_SHM.lock();
        match ids.resolve(key, flags) {
            Err(e) => return err(e),
            Ok(Some(id)) => match ids.get(id) {
                Some(entry) if !ipcperms(&entry.perm, &me.cred, state::requested(flags)) => return err(EACCES),
                Some(entry) if size > entry.obj.segsz => return err(EINVAL),
                _ => Some(id),
            },
            Ok(None) => None,
        }
    };
    if let Some(id) = existing {
        return SyscallResult::Ok(id as u64);
    }
    if !(SHMMIN..=SHMMAX).contains(&size) {
        return err(EINVAL);
    }
    // The frames are allocated and zeroed with no table locked.
    let perm = IpcPerm::new(key, me.cred.euid, me.cred.egid, flags);
    let obj = match ShmObject::new(size, perm.mode, perm.uid, perm.gid) {
        Ok(obj) => obj,
        Err(e) => return SyscallResult::Err(e),
    };
    let t = now();
    let mut ids = SYSV_SHM.lock();
    // Another process may have made the key meanwhile.
    if key != state::IPC_PRIVATE {
        if let Ok(Some(id)) = ids.resolve(key, flags) {
            drop(ids);
            return if flags & state::IPC_EXCL != 0 { err(EEXIST) } else { SyscallResult::Ok(id as u64) };
        }
    }
    // Removed segments whose last mapping has gone give their slots back.
    let gone: Vec<i32> = ids.iter_mut().filter(|(_, e)| e.obj.object().is_none()).map(|(id, _)| id).collect();
    for id in gone {
        ids.remove(id);
    }
    let segment = Segment { backing: Backing::Live(obj), segsz: size, cpid: me.pid, lpid: 0, atime: 0, dtime: 0, ctime: t };
    let made = ids.insert(perm, segment);
    drop(ids);
    match made {
        Ok(id) => SyscallResult::Ok(id as u64),
        Err(e) => err(e),
    }
}

/// shmat: map segment `id` shared at `addr`, or where the system chooses
/// when `addr` is 0: read-only under SHM_RDONLY. The address.
pub fn sys_shmat(id: i32, addr: u64, flags: u32) -> SyscallResult {
    if id < 0 {
        return err(EINVAL);
    }
    let mut addr = addr;
    if addr & (SHMLBA - 1) != 0 {
        if flags & SHM_RND == 0 {
            return err(EINVAL);
        }
        addr &= !(SHMLBA - 1);
    }
    let me = caller();
    let rdonly = flags & SHM_RDONLY != 0;
    let (obj, segsz) = {
        let mut ids = SYSV_SHM.lock();
        let obj = match live(&mut ids, id) {
            Ok(obj) => obj,
            Err(e) => return err(e),
        };
        let Some(entry) = ids.get(id) else { return err(EINVAL) };
        if !ipcperms(&entry.perm, &me.cred, if rdonly { READ } else { READ | WRITE }) {
            return err(EACCES);
        }
        (obj, entry.obj.segsz)
    };
    let mut prot = Protection::READ.bits();
    if !rdonly {
        prot |= Protection::WRITE.bits();
    }
    if flags & SHM_EXEC != 0 {
        prot |= Protection::EXEC.bits();
    }
    let start = match shmem::map_object(&obj, addr, segsz, Protection::from_bits_truncate(prot), addr != 0) {
        Ok(start) => start,
        Err(e) => return SyscallResult::Err(e),
    };
    let t = now();
    {
        let mut ids = SYSV_SHM.lock();
        if let Some(entry) = ids.get_mut(id).filter(|e| e.obj.is(&obj)) {
            entry.obj.lpid = me.pid;
            entry.obj.atime = t;
        };
    }
    drop(obj);
    SyscallResult::Ok(start)
}

/// shmdt: detach the segment attached at `addr`.
pub fn sys_shmdt(addr: u64) -> SyscallResult {
    let Some(obj) = shmem::object_at(addr) else { return err(EINVAL) };
    let len = obj.size().div_ceil(SHMLBA) * SHMLBA;
    if let SyscallResult::Err(e) = crate::syscall::mmap::sys_munmap(addr, len) {
        return SyscallResult::Err(e);
    }
    let me = caller();
    let t = now();
    {
        let mut ids = SYSV_SHM.lock();
        if let Some((_, entry)) = ids.iter_mut().find(|(_, e)| e.obj.is(&obj)) {
            entry.obj.lpid = me.pid;
            entry.obj.dtime = t;
        };
    }
    drop(obj);
    SyscallResult::Ok(0)
}

/// shmctl: IPC_STAT, IPC_SET and IPC_RMID.
pub fn sys_shmctl(id: i32, cmd: u32, buf: u64) -> SyscallResult {
    if id < 0 {
        return err(EINVAL);
    }
    let me = caller();
    // An object reference taken under the table lock is dropped after it,
    // since dropping the last one frees the segment's frames.
    let mut held: Option<Arc<ShmObject>> = None;
    let result = match cmd & !IPC_64 {
        IPC_STAT => {
            let ds = {
                let mut ids = SYSV_SHM.lock();
                match live(&mut ids, id) {
                    Err(e) => Err(e),
                    Ok(obj) => {
                        let nattch = obj.attachments() as u64;
                        held = Some(obj);
                        match ids.get(id) {
                            Some(entry) if !ipcperms(&entry.perm, &me.cred, READ) => Err(EACCES),
                            Some(entry) => {
                                let s = &entry.obj;
                                let stat = ShmStat {
                                    segsz: s.segsz,
                                    atime: s.atime,
                                    dtime: s.dtime,
                                    ctime: s.ctime,
                                    cpid: s.cpid,
                                    lpid: s.lpid,
                                    nattch,
                                };
                                Ok(state::shmid_ds(&entry.perm, &stat))
                            }
                            None => Err(EINVAL),
                        }
                    }
                }
            };
            match ds {
                Err(e) => err(e),
                Ok(ds) => match write_user_bytes(buf, ds.as_ptr(), ds.len()) {
                    Ok(()) => SyscallResult::Ok(0),
                    Err(_) => err(EFAULT),
                },
            }
        }
        IPC_SET => {
            let mut ds = [0u8; SHMID_DS_SIZE];
            if read_user_bytes(ds.as_mut_ptr(), buf, ds.len()).is_err() {
                return err(EFAULT);
            }
            let (uid, gid, mode) = state::perm_set_fields(&ds);
            let t = now();
            let mut ids = SYSV_SHM.lock();
            match live(&mut ids, id) {
                Err(e) => err(e),
                Ok(obj) => {
                    held = Some(obj);
                    match ids.get_mut(id) {
                        Some(entry) if !entry.perm.owned_by(me.cred.euid) => err(EPERM),
                        Some(entry) => match entry.perm.set(uid, gid, mode) {
                            Err(e) => err(e),
                            Ok(()) => {
                                entry.obj.ctime = t;
                                SyscallResult::Ok(0)
                            }
                        },
                        None => err(EINVAL),
                    }
                }
            }
        }
        IPC_RMID => {
            let mut ids = SYSV_SHM.lock();
            match live(&mut ids, id) {
                Err(e) => err(e),
                Ok(obj) => match ids.get(id) {
                    Some(entry) if !entry.perm.owned_by(me.cred.euid) => {
                        held = Some(obj);
                        err(EPERM)
                    }
                    Some(_) if obj.attachments() == 0 => {
                        held = Some(obj);
                        ids.remove(id);
                        SyscallResult::Ok(0)
                    }
                    Some(_) => {
                        ids.forget_key(id);
                        if let Some(entry) = ids.get_mut(id) {
                            entry.perm.mode |= SHM_DEST;
                            entry.obj.backing = Backing::Removed(Arc::downgrade(&obj));
                        }
                        held = Some(obj);
                        SyscallResult::Ok(0)
                    }
                    None => err(EINVAL),
                },
            }
        }
        _ => err(EINVAL),
    };
    drop(held);
    result
}
