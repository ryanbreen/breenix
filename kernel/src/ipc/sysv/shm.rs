//! System V shared memory segments: shmget, shmat, shmdt and shmctl.
//!
//! A segment is a `ShmObject` with no name. Each attachment is a MAP_SHARED
//! mapping of it whose VMA holds a `ShmMapping`, so fork, munmap, exec and
//! exit attach and detach through the VMAs, and shm_nattch is the object's
//! count of mappings. IPC_RMID of an attached segment removes its key, marks
//! it SHM_DEST and keeps only a weak reference: the last detach frees it, and
//! the next lookup of its identifier finds it gone.

use super::state::{
    self, IpcIds, IpcPerm, ShmStat, IPC_64, IPC_INFO, IPC_RMID, IPC_SET, IPC_STAT, READ, SHMID_DS_SIZE, SHMLBA, SHMMAX,
    SHMMIN, SHMMNI, SHM_DEST, SHM_EXEC, SHM_INFO, SHM_INFO_SIZE, SHMINFO_SIZE, SHM_LOCK, SHM_LOCKED, SHM_RDONLY, SHM_REMAP, SHM_RND, SHM_STAT,
    SHM_STAT_ANY, SHM_UNLOCK, WRITE,
};
use super::{caller, ipcperms, now};
use crate::fs::permissions::Credentials;
use crate::ipc::shmem::{self, ShmObject};
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::memory::vma::Protection;
use crate::syscall::errno::{EACCES, EEXIST, EFAULT, EINVAL, ENOMEM, EPERM, ESRCH};
use crate::syscall::memory_common::{self, PAGE_SIZE};
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
    /// The real user SHM_LOCK charged the segment's pages to, while it is
    /// locked.
    locker: Option<u32>,
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

    /// Whether the segment still exists: not removed, or removed with a
    /// mapping still holding it.
    fn exists(&self) -> bool {
        match &self.backing {
            Backing::Live(_) => true,
            Backing::Removed(weak) => weak.strong_count() > 0,
        }
    }

    fn pages(&self) -> u64 {
        self.segsz.div_ceil(PAGE_SIZE)
    }

    fn stat(&self, nattch: u64) -> ShmStat {
        ShmStat {
            segsz: self.segsz,
            atime: self.atime,
            dtime: self.dtime,
            ctime: self.ctime,
            cpid: self.cpid,
            lpid: self.lpid,
            nattch,
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
/// when IPC_CREAT asks for it. For an existing segment, asking for more
/// than its size is EINVAL before the permission check, as on Linux.
pub fn sys_shmget(key: i32, size: u64, flags: u32) -> SyscallResult {
    let me = caller();
    let existing = {
        let ids = SYSV_SHM.lock();
        match ids.resolve(key, flags) {
            Err(e) => return err(e),
            Ok(Some(id)) => match ids.get(id) {
                Some(entry) if size > entry.obj.segsz => return err(EINVAL),
                Some(entry) if !ipcperms(&entry.perm, &me.cred, state::requested(flags)) => return err(EACCES),
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
    loop {
        let gone = ids.iter().find(|(_, e)| !e.obj.exists()).map(|(id, _)| id);
        let Some(id) = gone else { break };
        ids.remove(id);
    }
    let segment = Segment {
        backing: Backing::Live(obj),
        segsz: size,
        cpid: me.pid,
        lpid: 0,
        atime: 0,
        dtime: 0,
        ctime: t,
        locker: None,
    };
    let made = ids.insert(perm, segment);
    drop(ids);
    match made {
        Ok(id) => SyscallResult::Ok(id as u64),
        Err(e) => err(e),
    }
}

/// Whether the calling process has anything mapped in `start..end`.
fn range_mapped(start: u64, end: u64) -> Result<bool, i32> {
    let tid = memory_common::get_current_thread_id().ok_or(ESRCH)?;
    let mut guard = crate::process::manager_for_address_space(tid);
    let (_, process) = guard.as_mut().and_then(|m| m.find_address_space_by_thread_mut(tid)).ok_or(ESRCH)?;
    Ok(process.vmas.iter().any(|v| v.start.as_u64() < end && start < v.end.as_u64()))
}

/// shmat: map segment `id` shared at `addr`, or where the system chooses
/// when `addr` is 0: read-only under SHM_RDONLY. The address. As on Linux,
/// an address range that already holds a mapping is EINVAL unless
/// SHM_REMAP asks to replace it, and SHM_REMAP needs an address.
pub fn sys_shmat(id: i32, addr: u64, flags: u32) -> SyscallResult {
    if id < 0 {
        return err(EINVAL);
    }
    let fixed = addr != 0;
    let mut addr = addr;
    if fixed {
        if addr & (SHMLBA - 1) != 0 {
            if flags & SHM_RND == 0 {
                return err(EINVAL);
            }
            addr &= !(SHMLBA - 1);
            if addr == 0 && flags & SHM_REMAP != 0 {
                return err(EINVAL);
            }
        }
    } else if flags & SHM_REMAP != 0 {
        return err(EINVAL);
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
    if fixed && flags & SHM_REMAP == 0 {
        let Some(end) = addr.checked_add(segsz) else { return err(EINVAL) };
        match range_mapped(addr, end) {
            Ok(false) => {}
            Ok(true) => return err(EINVAL),
            Err(e) => return err(e),
        }
    }
    let mut prot = Protection::READ.bits();
    if !rdonly {
        prot |= Protection::WRITE.bits();
    }
    if flags & SHM_EXEC != 0 {
        prot |= Protection::EXEC.bits();
    }
    let start = match shmem::map_object(&obj, addr, segsz, Protection::from_bits_truncate(prot), fixed) {
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

/// The attachment at `addr`, as Linux's shmdt finds it: the lowest shared
/// memory VMA at or above `addr` whose object offset places the segment's
/// start at `addr` names the segment, and the VMAs to unmap are that
/// segment's ones, placed the same way, that end within its size of `addr`.
/// Anything else mapped in that range stays.
fn attachment_at(addr: u64) -> Result<(Arc<ShmObject>, Vec<(u64, u64)>), i32> {
    let tid = memory_common::get_current_thread_id().ok_or(EINVAL)?;
    let mut guard = crate::process::manager_for_address_space(tid);
    let (_, process) = guard.as_mut().and_then(|m| m.find_address_space_by_thread_mut(tid)).ok_or(EINVAL)?;
    let placed = |v: &crate::memory::vma::Vma| {
        let start = v.start.as_u64();
        start >= addr && v.shm.is_some() && start - addr == v.shm_offset
    };
    let first = process.vmas.iter().filter(|v| placed(v)).min_by_key(|v| v.start.as_u64()).ok_or(EINVAL)?;
    let obj = first.shm.as_ref().map(|m| m.object().clone()).ok_or(EINVAL)?;
    let span = obj.size().div_ceil(PAGE_SIZE) * PAGE_SIZE;
    let mut ranges = Vec::new();
    for v in process.vmas.iter().filter(|v| placed(v)) {
        let (start, end) = (v.start.as_u64(), v.end.as_u64());
        let same = v.shm.as_ref().is_some_and(|m| Arc::ptr_eq(m.object(), &obj));
        if same && end - addr <= span {
            state::try_push(&mut ranges, (start, end)).map_err(|_| ENOMEM)?;
        }
    }
    Ok((obj, ranges))
}

/// shmdt: detach the segment attached at `addr`.
pub fn sys_shmdt(addr: u64) -> SyscallResult {
    if addr & (PAGE_SIZE - 1) != 0 {
        return err(EINVAL);
    }
    let (obj, ranges) = match attachment_at(addr) {
        Ok(found) => found,
        Err(e) => return err(e),
    };
    // Each range is one whole VMA of the attachment, which munmap removes
    // and no more.
    for (start, end) in ranges {
        let _ = crate::syscall::mmap::sys_munmap(start, end - start);
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

/// The calling process's RLIMIT_MEMLOCK, in bytes.
fn memlock_limit() -> u64 {
    crate::task::scheduler::current_thread_id()
        .and_then(|tid| {
            crate::process::with_process_manager(|manager| {
                manager
                    .find_process_by_thread(tid)
                    .map(|(_, p)| p.limits.get(crate::process::limits::MEMLOCK).soft)
            })
            .flatten()
        })
        .unwrap_or(0)
}

/// shmctl: IPC_STAT, IPC_SET and IPC_RMID, and Linux's IPC_INFO, SHM_INFO,
/// SHM_STAT, SHM_STAT_ANY, SHM_LOCK and SHM_UNLOCK.
pub fn sys_shmctl(id: i32, cmd: u32, buf: u64) -> SyscallResult {
    if id < 0 {
        return err(EINVAL);
    }
    let me = caller();
    let cmd = cmd & !IPC_64;
    // An object reference taken under the table lock is dropped after it,
    // since dropping the last one frees the segment's frames.
    let mut held: Option<Arc<ShmObject>> = None;
    let result = match cmd {
        IPC_STAT | SHM_STAT | SHM_STAT_ANY => {
            let ds = {
                let mut ids = SYSV_SHM.lock();
                // SHM_STAT and SHM_STAT_ANY take an index into the table and
                // return the identifier of the segment there; SHM_STAT_ANY
                // skips the read permission check.
                let sid = match cmd {
                    IPC_STAT => Some(id),
                    _ => ids.at_index(id).map(|(sid, _)| sid),
                };
                match sid.ok_or(EINVAL).and_then(|sid| live(&mut ids, sid).map(|obj| (sid, obj))) {
                    Err(e) => Err(e),
                    Ok((sid, obj)) => {
                        let nattch = obj.attachments() as u64;
                        held = Some(obj);
                        match ids.get(sid) {
                            Some(entry) if cmd != SHM_STAT_ANY && !ipcperms(&entry.perm, &me.cred, READ) => {
                                Err(EACCES)
                            }
                            Some(entry) => {
                                let result = if cmd == IPC_STAT { 0 } else { sid as u64 };
                                Ok((state::shmid_ds(&entry.perm, &entry.obj.stat(nattch)), result))
                            }
                            None => Err(EINVAL),
                        }
                    }
                }
            };
            match ds {
                Err(e) => err(e),
                Ok((ds, result)) => match write_user_bytes(buf, ds.as_ptr(), ds.len()) {
                    Ok(()) => SyscallResult::Ok(result),
                    Err(_) => err(EFAULT),
                },
            }
        }
        IPC_INFO | SHM_INFO => {
            let (info, len, max) = {
                let ids = SYSV_SHM.lock();
                let max = ids.highest_index_where(|e| e.obj.exists());
                if cmd == IPC_INFO {
                    (state::shminfo(), SHMINFO_SIZE, max)
                } else {
                    let (used, pages) = ids
                        .iter()
                        .filter(|(_, e)| e.obj.exists())
                        .fold((0, 0), |(n, p), (_, e)| (n + 1, p + e.obj.pages()));
                    let mut info = [0u8; SHMINFO_SIZE];
                    info[..SHM_INFO_SIZE].copy_from_slice(&state::shm_info(used, pages));
                    (info, SHM_INFO_SIZE, max)
                }
            };
            match write_user_bytes(buf, info.as_ptr(), len) {
                Ok(()) => SyscallResult::Ok(max as u64),
                Err(_) => err(EFAULT),
            }
        }
        SHM_LOCK | SHM_UNLOCK => {
            // Linux lets the owner or creator lock a segment without
            // CAP_IPC_LOCK, charging its pages to the locker's real user
            // within RLIMIT_MEMLOCK. Breenix has no swap, so the lock is
            // that accounting and the SHM_LOCKED mode bit.
            let real_uid = Credentials::current(true).euid;
            let limit = memlock_limit();
            let privileged = me.cred.euid == 0;
            let mut ids = SYSV_SHM.lock();
            match live(&mut ids, id) {
                Err(e) => err(e),
                Ok(obj) => {
                    held = Some(obj);
                    let charged: u64 = ids
                        .iter()
                        .filter(|(_, e)| e.obj.locker == Some(real_uid) && e.obj.exists())
                        .map(|(_, e)| e.obj.pages())
                        .sum();
                    match ids.get_mut(id) {
                        None => err(EINVAL),
                        Some(entry) if !privileged && !entry.perm.owned_by(me.cred.euid) => err(EPERM),
                        Some(_) if !privileged && cmd == SHM_LOCK && limit == 0 => err(EPERM),
                        Some(entry) if cmd == SHM_LOCK => {
                            if entry.perm.mode & SHM_LOCKED != 0 {
                                SyscallResult::Ok(0)
                            } else if !privileged
                                && limit != crate::process::limits::INFINITY
                                && charged + entry.obj.pages() > limit / PAGE_SIZE
                            {
                                err(ENOMEM)
                            } else {
                                entry.perm.mode |= SHM_LOCKED;
                                entry.obj.locker = Some(real_uid);
                                SyscallResult::Ok(0)
                            }
                        }
                        Some(entry) => {
                            entry.perm.mode &= !SHM_LOCKED;
                            entry.obj.locker = None;
                            SyscallResult::Ok(0)
                        }
                    }
                }
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
