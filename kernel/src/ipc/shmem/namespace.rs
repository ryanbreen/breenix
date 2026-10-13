//! /dev/shm hooks and descriptor methods shared by both syscall dispatchers.
use super::{map_range, MappingOrigin, ShmObject};
use crate::ipc::fd::{FdKind, FileDescriptor};
use crate::ipc::object::IpcFile;
use crate::irq_safe_mutex::IrqSafeMutex;
use crate::memory::vma::{MmapFlags, Protection};
use crate::syscall::{errno, fs::Stat, SyscallResult};
use alloc::{string::String, sync::Arc, vec::Vec};

struct NameEntry {
    name: String,
    object: Arc<ShmObject>,
    cookie: u64,
}
static NEXT_COOKIE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(3);
static SHM_NAMES: IrqSafeMutex<Vec<NameEntry>> = IrqSafeMutex::new(Vec::new());
const O_CREAT: u32 = 0x40;
const O_EXCL: u32 = 0x80;
const O_TRUNC: u32 = 0x200;

fn component(path: &str) -> Option<Result<&str, u64>> {
    if path == "/dev/shm" || path == "/dev/shm/" {
        return Some(Ok(""));
    }
    let name = path.strip_prefix("/dev/shm/")?;
    Some(if name.len() > 255 {
        Err(errno::ENAMETOOLONG as u64)
    } else if name.contains('/') || name == "." || name == ".." {
        Err(errno::EINVAL as u64)
    } else {
        Ok(name)
    })
}
fn device() -> u64 {
    crate::fs::vfs::mount::find_mount("/dev").unwrap_or(0) as u64
}
fn fill_directory(st: &mut Stat) {
    st.st_dev = device();
    st.st_ino = 0x1000;
    st.st_mode = 0o040000 | 0o1777;
    st.st_nlink = 2;
    st.st_blksize = 4096;
}
struct ShmDirectory {
    cookie: IrqSafeMutex<u64>,
}
impl core::fmt::Debug for ShmDirectory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("ShmDirectory")
    }
}
impl IpcFile for ShmDirectory {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn fill_stat(&self, st: &mut Stat) {
        fill_directory(st);
    }
    fn seek(&self, offset: i64, whence: i32) -> Result<u64, u64> {
        let old = reserve_position(&self.cookie);
        let result = seek_position(old, 0, offset, whence, true);
        *self.cookie.lock() = result.as_ref().copied().unwrap_or(old);
        result
    }
    fn read(&self, _buf: u64, _count: usize) -> Result<u64, u64> {
        Err(errno::EISDIR as u64)
    }
    fn write(&self, _bytes: &[u8], _append: bool) -> Result<u64, u64> {
        Err(errno::EISDIR as u64)
    }
    fn getdents(&self, dirp: u64, count: usize) -> Result<u64, u64> {
        // Reserve the description's cookie without holding an IRQ-safe lock
        // during user access. A concurrent reader retries instead of copying
        // the same batch. u64::MAX is outside the inode cookie domain.
        let offset = loop {
            let mut cookie = self.cookie.lock();
            if *cookie != u64::MAX {
                let offset = *cookie;
                *cookie = u64::MAX;
                break offset;
            }
            drop(cookie);
            crate::task::scheduler::yield_current();
        };
        let batch = directory_entries(offset, count);
        let result = batch.and_then(|bytes| {
            crate::syscall::userptr::write_user_bytes(dirp, bytes.as_ptr(), bytes.len())
                .map_err(|_| errno::EFAULT as u64)?;
            let next = if bytes.is_empty() {
                offset
            } else {
                let mut at = 0;
                let mut next = offset;
                while at < bytes.len() {
                    next = u64::from_ne_bytes(bytes[at + 8..at + 16].try_into().unwrap());
                    at += u16::from_ne_bytes(bytes[at + 16..at + 18].try_into().unwrap()) as usize;
                }
                next
            };
            Ok((bytes.len() as u64, next))
        });
        *self.cookie.lock() = result.as_ref().map_or(offset, |(_, next)| *next);
        result.map(|(len, _)| len)
    }
}
pub struct ShmFile {
    object: Arc<ShmObject>,
    writable: bool,
    linkable: bool,
    position: IrqSafeMutex<u64>,
}
impl core::fmt::Debug for ShmFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ShmFile")
            .field("object", &self.object)
            .finish()
    }
}
impl IpcFile for ShmFile {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
    fn fill_stat(&self, st: &mut Stat) {
        st.st_dev = device();
        st.st_ino = self.object.ino();
        let (mode, uid, gid, _) = self.object.metadata();
        st.st_mode = 0o100000 | mode;
        st.st_nlink = self.object.links().into();
        st.st_uid = uid;
        st.st_gid = gid;
        st.st_size = self.object.size() as i64;
        st.st_blksize = 4096;
        st.st_blocks = (self.object.allocated_pages() * 8) as i64;
    }
    fn chmod(&self, mode: u32, cred: &crate::fs::permissions::Credentials) -> Result<(), u64> {
        self.object.chmod(mode, cred)
    }
    fn chown(
        &self,
        uid: u32,
        gid: u32,
        cred: &crate::fs::permissions::Credentials,
    ) -> Result<(), u64> {
        self.object.chown(uid, gid, cred)
    }
    fn seek(&self, offset: i64, whence: i32) -> Result<u64, u64> {
        let old = reserve_position(&self.position);
        let result = seek_position(old, self.object.size(), offset, whence, false);
        *self.position.lock() = result.as_ref().copied().unwrap_or(old);
        result
    }
    fn read(&self, buf: u64, count: usize) -> Result<u64, u64> {
        let old = reserve_position(&self.position);
        let _io = self.object.io_guard();
        let result = (|| {
            let len = (count as u64)
                .min(self.object.size().saturating_sub(old))
                .min(64 * 1024) as usize;
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(len)
                .map_err(|_| errno::ENOMEM as u64)?;
            bytes.resize(len, 0);
            let mut done = 0;
            while done < len {
                let offset = old + done as u64;
                let part =
                    (len - done).min((super::PAGE_SIZE - offset % super::PAGE_SIZE) as usize);
                if let Some(frame) = self.object.frame_at(offset) {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            (crate::memory::physical_memory_offset().as_u64()
                                + frame.start_address().as_u64()
                                + offset % super::PAGE_SIZE)
                                as *const u8,
                            bytes.as_mut_ptr().add(done),
                            part,
                        );
                    }
                    super::release_frame(frame);
                }
                done += part;
            }
            crate::syscall::userptr::write_user_bytes(buf, bytes.as_ptr(), len)
                .map_err(|_| errno::EFAULT as u64)?;
            Ok(len as u64)
        })();
        *self.position.lock() = old + result.as_ref().copied().unwrap_or(0);
        result
    }
    fn write(&self, bytes: &[u8], append: bool) -> Result<u64, u64> {
        let old = reserve_position(&self.position);
        let _io = self.object.io_guard();
        let start = if append { self.object.size() } else { old };
        let result = (|| {
            if bytes.is_empty() {
                return Ok(0);
            }
            let end = start
                .checked_add(bytes.len() as u64)
                .filter(|v| *v <= i64::MAX as u64)
                .ok_or(errno::EFBIG as u64)?;
            if end > self.object.size() {
                if end > crate::syscall::resource::current_fsize() {
                    crate::syscall::resource::signal_fsize();
                    return Err(errno::EFBIG as u64);
                }
                self.object.resize_locked(end)?;
            }
            let mut done = 0;
            while done < bytes.len() {
                let offset = start + done as u64;
                let part = (bytes.len() - done)
                    .min((super::PAGE_SIZE - offset % super::PAGE_SIZE) as usize);
                match self.object.ensure_page(offset) {
                    Err(e) if e == errno::EAGAIN as u64 => {
                        crate::task::scheduler::yield_current();
                        continue;
                    }
                    Err(e) => {
                        return if done == 0 { Err(e) } else { Ok(done as u64) };
                    }
                    Ok(()) => {}
                }
                let Some(frame) = self.object.frame_at(offset) else {
                    return if done == 0 {
                        Err(errno::EIO as u64)
                    } else {
                        Ok(done as u64)
                    };
                };
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        bytes.as_ptr().add(done),
                        (crate::memory::physical_memory_offset().as_u64()
                            + frame.start_address().as_u64()
                            + offset % super::PAGE_SIZE) as *mut u8,
                        part,
                    );
                }
                super::release_frame(frame);
                done += part;
            }
            Ok(done as u64)
        })();
        *self.position.lock() = result.as_ref().map_or(old, |n| start + n);
        result
    }
    fn truncate(&self, len: u64) -> Result<(), u64> {
        let _io = self.object.io_guard();
        if !self.writable {
            return Err(errno::EINVAL as u64);
        }
        if len > self.object.size() && len > crate::syscall::resource::current_fsize() {
            crate::syscall::resource::signal_fsize();
            return Err(errno::EFBIG as u64);
        }
        self.object.resize_locked(len)
    }
}
struct Slot {
    tid: u64,
    fd: i32,
    installed: bool,
}
impl Slot {
    fn reserve() -> Result<Self, u64> {
        let tid = super::memory_common::get_current_thread_id().ok_or(errno::ESRCH as u64)?;
        let mut guard = crate::process::manager();
        let (_, process) = guard
            .as_mut()
            .and_then(|m| m.find_process_by_thread_mut(tid))
            .ok_or(errno::ESRCH as u64)?;
        let fd = process
            .fd_table
            .reserve_slot()
            .ok_or(errno::EMFILE as u64)?;
        Ok(Self {
            tid,
            fd,
            installed: false,
        })
    }
    fn install(mut self, descriptor: FileDescriptor) -> Result<u64, u64> {
        let mut guard = crate::process::manager();
        let (_, process) = guard
            .as_mut()
            .and_then(|m| m.find_process_by_thread_mut(self.tid))
            .ok_or(errno::ESRCH as u64)?;
        let fd = process
            .fd_table
            .install_reserved(self.fd, descriptor)
            .map_err(|e| e as u64)?;
        self.installed = true;
        Ok(fd as u64)
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        if !self.installed {
            let mut guard = crate::process::manager();
            if let Some((_, process)) = guard
                .as_mut()
                .and_then(|m| m.find_process_by_thread_mut(self.tid))
            {
                process.fd_table.release_reserved(self.fd);
            }
        }
    }
}
fn result(r: Result<u64, u64>) -> SyscallResult {
    match r {
        Ok(v) => SyscallResult::Ok(v),
        Err(e) => SyscallResult::Err(e),
    }
}

pub fn open_path(path: &str, flags: u32, mode: u32) -> Option<SyscallResult> {
    let name = match component(path)? {
        Ok(name) => name,
        Err(e) => return Some(SyscallResult::Err(e)),
    };
    Some(result((|| {
        if flags & 3 == 3 {
            return Err(errno::EINVAL as u64);
        }
        let cred = crate::fs::permissions::Credentials::current(false);
        let slot = Slot::reserve()?;
        if name.is_empty() {
            if flags & 0x400000 != 0 {
                if flags & 3 != 2 {
                    return Err(errno::EINVAL as u64);
                }
                let object = ShmObject::new(0, mode & !cred.umask, cred.euid, cred.egid)?;
                object.unlink();
                let file = Arc::try_new(ShmFile {
                    object,
                    writable: true,
                    linkable: flags & O_EXCL == 0,
                    position: IrqSafeMutex::new(0),
                })
                .map_err(|_| errno::ENOMEM as u64)?;
                return slot.install(FileDescriptor::try_opened(FdKind::Ipc(file), flags)?);
            }
            if flags & 3 != 0 || flags & (O_CREAT | O_TRUNC) != 0 {
                return Err(errno::EISDIR as u64);
            }
            let file = Arc::try_new(ShmDirectory {
                cookie: IrqSafeMutex::new(0),
            })
            .map_err(|_| errno::ENOMEM as u64)?;
            return slot.install(FileDescriptor::try_opened(FdKind::Ipc(file), flags)?);
        }
        if flags & crate::syscall::fs::O_DIRECTORY != 0 {
            return Err(errno::ENOTDIR as u64);
        }
        let mut key = String::new();
        key.try_reserve_exact(name.len())
            .map_err(|_| errno::ENOMEM as u64)?;
        key.push_str(name);
        // Prepare the candidate outside the name-table lock. If a creator
        // wins the race, lookup below retains its object instead.
        let candidate = if flags & O_CREAT != 0 {
            Some(ShmObject::new(
                0,
                mode & 0o7777 & !cred.umask,
                cred.euid,
                cred.egid,
            )?)
        } else {
            None
        };
        let (object, descriptor) = {
            let mut names = SHM_NAMES.lock();
            if let Some(entry) = names.iter().find(|e| e.name == name) {
                let object = &entry.object;
                if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL {
                    return Err(errno::EEXIST as u64);
                }
                let wanted = match flags & 3 {
                    0 => 4,
                    1 => 2,
                    _ => 6,
                } | if flags & O_TRUNC != 0 { 2 } else { 0 };
                let (mode, uid, gid, _) = object.metadata();
                if !cred.permits_mode(uid, gid, mode, wanted) {
                    return Err(errno::EACCES as u64);
                }
                let file = Arc::try_new(ShmFile {
                    object: object.clone(),
                    writable: flags & 3 != 0,
                    linkable: false,
                    position: IrqSafeMutex::new(0),
                })
                .map_err(|_| errno::ENOMEM as u64)?;
                let descriptor = FileDescriptor::try_opened(FdKind::Ipc(file), flags)?;
                (object.clone(), descriptor)
            } else {
                if flags & O_CREAT == 0 {
                    return Err(errno::ENOENT as u64);
                }
                let object = candidate.ok_or(errno::ENOMEM as u64)?;
                names.try_reserve(1).map_err(|_| errno::ENOMEM as u64)?;
                let file = Arc::try_new(ShmFile {
                    object: object.clone(),
                    writable: flags & 3 != 0,
                    linkable: false,
                    position: IrqSafeMutex::new(0),
                })
                .map_err(|_| errno::ENOMEM as u64)?;
                let descriptor = FileDescriptor::try_opened(FdKind::Ipc(file), flags)?;
                let at = names.len();
                names.insert(
                    at,
                    NameEntry {
                        name: key,
                        object: object.clone(),
                        cookie: NEXT_COOKIE.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
                    },
                );
                (object, descriptor)
            }
        };
        if flags & O_TRUNC != 0 {
            object.resize(0)?;
        }
        slot.install(descriptor)
    })()))
}
pub fn unlink_path(path: &str) -> Option<SyscallResult> {
    let name = match component(path)? {
        Ok(name) => name,
        Err(e) => return Some(SyscallResult::Err(e)),
    };
    Some(result((|| {
        if name.is_empty() {
            return Err(errno::EISDIR as u64);
        }
        let cred = crate::fs::permissions::Credentials::current(false);
        let removed = {
            let mut names = SHM_NAMES.lock();
            let index = names
                .iter()
                .position(|e| e.name == name)
                .ok_or(errno::ENOENT as u64)?;
            let object = &names[index].object;
            // The flat directory is mode 1777: only the owner or root unlinks.
            if cred.euid != 0 && cred.euid != object.metadata().1 {
                return Err(errno::EPERM as u64);
            }
            object.unlink();
            names.remove(index)
        };
        drop(removed);
        Ok(0)
    })()))
}
pub fn virtual_entry(path: &str) -> Option<Option<bool>> {
    Some(match component(path)? {
        Ok("") => Some(true),
        Ok(name) => SHM_NAMES
            .lock()
            .iter()
            .any(|e| e.name == name)
            .then_some(false),
        Err(_) => None,
    })
}
pub fn stat_path(path: &str, statbuf: u64) -> Option<SyscallResult> {
    let name = match component(path)? {
        Ok(name) => name,
        Err(e) => return Some(SyscallResult::Err(e)),
    };
    Some(result((|| {
        let mut st = Stat::zeroed();
        if name.is_empty() {
            fill_directory(&mut st);
        } else {
            let object = SHM_NAMES
                .lock()
                .iter()
                .find(|e| e.name == name)
                .map(|e| e.object.clone())
                .ok_or(errno::ENOENT as u64)?;
            ShmFile {
                object,
                writable: false,
                linkable: false,
                position: IrqSafeMutex::new(0),
            }
            .fill_stat(&mut st);
        }
        crate::syscall::userptr::copy_to_user(statbuf as *mut Stat, &st)
            .map_err(|_| errno::EFAULT as u64)?;
        Ok(0)
    })()))
}
pub fn mmap_fd(
    fd: i32,
    addr: u64,
    len: u64,
    prot: Protection,
    flags: MmapFlags,
    offset: u64,
) -> Option<SyscallResult> {
    let tid = super::memory_common::get_current_thread_id()?;
    let file = {
        let guard = crate::process::manager();
        let (_, process) = guard.as_ref()?.find_process_by_thread(tid)?;
        let descriptor = process.fd_table.get(fd)?;
        let FdKind::Ipc(file) = &descriptor.kind else {
            return None;
        };
        if !file.as_any().is::<ShmFile>() {
            return None;
        }
        if !descriptor.readable()
            || (prot.contains(Protection::WRITE)
                && flags.contains(MmapFlags::SHARED)
                && !descriptor.writable())
        {
            return Some(SyscallResult::Err(errno::EACCES as u64));
        }
        file.clone()
    };
    Some(result((|| {
        let kind = flags.bits() & 3;
        if kind == 0 {
            return Err(errno::EINVAL as u64);
        }
        // MAP_SHARED_VALIDATE has the shared type bits plus strict flag checks.
        const KNOWN: u32 = 3
            | 0x10
            | 0x100
            | 0x800
            | 0x1000
            | 0x2000
            | 0x4000
            | 0x8000
            | 0x10000
            | 0x20000
            | 0x10_0000
            | if cfg!(target_arch = "x86_64") {
                0x40
            } else {
                0
            };
        if kind == 3 && flags.bits() & !KNOWN != 0 {
            return Err(errno::EOPNOTSUPP as u64);
        }
        // Linux recognizes MAP_GROWSDOWN as a legacy flag but refuses it
        // for file mappings, including tmpfs: EINVAL, not EOPNOTSUPP.
        if flags.bits() & 0x100 != 0 {
            return Err(errno::EINVAL as u64);
        }
        let flags = if kind == 3 {
            MmapFlags::from_bits_truncate((flags.bits() & !3) | 1)
        } else {
            flags
        };
        let file = file
            .as_any()
            .downcast_ref::<ShmFile>()
            .ok_or(errno::ENODEV as u64)?;
        map_range(
            &file.object,
            addr,
            len,
            prot,
            flags,
            offset,
            MappingOrigin::Mmap,
            flags.contains(MmapFlags::PRIVATE) || file.writable,
        )
    })()))
}

pub fn access_path(
    path: &str,
    mode: u32,
    cred: &crate::fs::permissions::Credentials,
) -> Option<SyscallResult> {
    let name = match component(path)? {
        Ok(n) => n,
        Err(e) => return Some(SyscallResult::Err(e)),
    };
    Some(result((|| {
        if mode & !7 != 0 {
            return Err(errno::EINVAL as u64);
        }
        if name.is_empty() {
            return Ok(0);
        }
        let object = SHM_NAMES
            .lock()
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.object.clone())
            .ok_or(errno::ENOENT as u64)?;
        let (bits, uid, gid, _) = object.metadata();
        if cred.permits_mode(uid, gid, bits, mode) {
            Ok(0)
        } else {
            Err(errno::EACCES as u64)
        }
    })()))
}

// Linux dirent64: ino@0, next-cookie@8, reclen@16, type@18, name@19.
// Monotonic inode cookies survive deletion/insertion without skipping a later
// surviving entry. Dot entries use cookies 1 and 2.
fn dirent(
    out: &mut Vec<u8>,
    count: usize,
    ino: u64,
    cookie: u64,
    kind: u8,
    name: &str,
) -> Result<bool, u64> {
    let len = (19 + name.len() + 1 + 7) & !7;
    if len > count.saturating_sub(out.len()) {
        return if out.is_empty() {
            Err(errno::EINVAL as u64)
        } else {
            Ok(false)
        };
    }
    out.try_reserve(len).map_err(|_| errno::ENOMEM as u64)?;
    let start = out.len();
    out.resize(start + len, 0);
    out[start..start + 8].copy_from_slice(&ino.to_ne_bytes());
    out[start + 8..start + 16].copy_from_slice(&cookie.to_ne_bytes());
    out[start + 16..start + 18].copy_from_slice(&(len as u16).to_ne_bytes());
    out[start + 18] = kind;
    out[start + 19..start + 19 + name.len()].copy_from_slice(name.as_bytes());
    Ok(true)
}
fn directory_entries(offset: u64, count: usize) -> Result<Vec<u8>, u64> {
    (|| {
        let mut out = Vec::new();
        // getdents may return a short batch. Reserve before the table lock;
        // appending the bounded batch below cannot allocate with IRQs masked.
        let count = count.min(64 * 1024);
        out.try_reserve_exact(count)
            .map_err(|_| errno::ENOMEM as u64)?;
        if offset < 1 && !dirent(&mut out, count, 0x1000, 1, 4, ".")? {
            return Ok(out);
        }
        if offset < 2 && !dirent(&mut out, count, 1, 2, 4, "..")? {
            return Ok(out);
        }
        let names = SHM_NAMES.lock();
        // Entries are kept in inode order even when creators prepare their
        // candidates concurrently; cookies remain stable across unlink.
        for entry in names.iter().filter(|e| e.cookie > offset.max(2)) {
            let ino = entry.object.ino();
            if !dirent(&mut out, count, ino, entry.cookie, 8, &entry.name)? {
                break;
            }
        }
        Ok(out)
    })()
}

/// Atomic hard-link publication lets libc initialize a private temporary
/// semaphore completely before any opener can see its public name.
pub fn link_path(old: &str, new: &str) -> Option<SyscallResult> {
    let old_name = component(old);
    let new_name = component(new);
    if old_name.is_none() && new_name.is_none() {
        return None;
    }
    Some(result((|| {
        let old = old_name.ok_or(errno::EXDEV as u64)??;
        let new = new_name.ok_or(errno::EXDEV as u64)??;
        if old.is_empty() || new.is_empty() {
            return Err(errno::EPERM as u64);
        }
        let cred = crate::fs::permissions::Credentials::current(false);
        let mut key = String::new();
        key.try_reserve_exact(new.len())
            .map_err(|_| errno::ENOMEM as u64)?;
        key.push_str(new);
        let mut names = SHM_NAMES.lock();
        let object = names
            .iter()
            .find(|e| e.name == old)
            .ok_or(errno::ENOENT as u64)?
            .object
            .clone();
        let (mode, uid, gid, _) = object.metadata();
        if cred.euid != 0
            && cred.euid != uid
            && (!cred.permits_mode(uid, gid, mode, 6) || mode & 0o6000 != 0)
        {
            return Err(errno::EPERM as u64);
        }
        if names.iter().any(|e| e.name == new) {
            return Err(errno::EEXIST as u64);
        }
        names.try_reserve(1).map_err(|_| errno::ENOMEM as u64)?;
        object.link()?;
        names.push(NameEntry {
            name: key,
            object,
            cookie: NEXT_COOKIE.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        });
        Ok(0)
    })()))
}
pub fn rename_path(old: &str, new: &str) -> Option<SyscallResult> {
    let old_name = component(old);
    let new_name = component(new);
    if old_name.is_none() && new_name.is_none() {
        return None;
    }
    Some(result((|| {
        let old = old_name.ok_or(errno::EXDEV as u64)??;
        let new = new_name.ok_or(errno::EXDEV as u64)??;
        if old.is_empty() || new.is_empty() {
            return Err(errno::EBUSY as u64);
        }
        let mut key = String::new();
        key.try_reserve_exact(new.len())
            .map_err(|_| errno::ENOMEM as u64)?;
        key.push_str(new);
        let cred = crate::fs::permissions::Credentials::current(false);
        let removed = {
            let mut names = SHM_NAMES.lock();
            let from = names
                .iter()
                .position(|e| e.name == old)
                .ok_or(errno::ENOENT as u64)?;
            if cred.euid != 0 && cred.euid != names[from].object.metadata().1 {
                return Err(errno::EPERM as u64);
            }
            let target = names.iter().position(|e| e.name == new);
            if target.is_some_and(|to| Arc::ptr_eq(&names[to].object, &names[from].object)) {
                return Ok(0);
            }
            if let Some(to) = target {
                if cred.euid != 0 && cred.euid != names[to].object.metadata().1 {
                    return Err(errno::EPERM as u64);
                }
            }
            names[from].name = key;
            target.map(|to| {
                names[to].object.unlink();
                names.remove(to)
            })
        };
        drop(removed);
        Ok(0)
    })()))
}

/// Link an unnamed O_TMPFILE description with AT_EMPTY_PATH. No public
/// name exists until its contents are initialized; failed creators leave no name.
pub fn link_fd(fd: i32, path: &str) -> Option<SyscallResult> {
    let new = match component(path)? {
        Ok(n) => n,
        Err(e) => return Some(SyscallResult::Err(e)),
    };
    Some(result((|| {
        if new.is_empty() {
            return Err(errno::EEXIST as u64);
        }
        let tid = super::memory_common::get_current_thread_id().ok_or(errno::ESRCH as u64)?;
        let file = {
            let guard = crate::process::manager();
            let (_, p) = guard
                .as_ref()
                .and_then(|m| m.find_process_by_thread(tid))
                .ok_or(errno::ESRCH as u64)?;
            let desc = p.fd_table.get(fd).ok_or(errno::EBADF as u64)?;
            let FdKind::Ipc(file) = &desc.kind else {
                return Err(errno::EXDEV as u64);
            };
            if !file
                .as_any()
                .downcast_ref::<ShmFile>()
                .is_some_and(|f| f.linkable)
            {
                return Err(errno::ENOENT as u64);
            }
            file.clone()
        };
        let object = &file
            .as_any()
            .downcast_ref::<ShmFile>()
            .ok_or(errno::EXDEV as u64)?
            .object;
        let mut key = String::new();
        key.try_reserve_exact(new.len())
            .map_err(|_| errno::ENOMEM as u64)?;
        key.push_str(new);
        let mut names = SHM_NAMES.lock();
        if names.iter().any(|e| e.name == new) {
            return Err(errno::EEXIST as u64);
        }
        names.try_reserve(1).map_err(|_| errno::ENOMEM as u64)?;
        object.link()?;
        names.push(NameEntry {
            name: key,
            object: object.clone(),
            cookie: NEXT_COOKIE.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        });
        Ok(0)
    })()))
}

fn reserve_position(position: &IrqSafeMutex<u64>) -> u64 {
    loop {
        let mut p = position.lock();
        if *p != u64::MAX {
            let old = *p;
            *p = u64::MAX;
            return old;
        }
        drop(p);
        crate::task::scheduler::yield_current();
    }
}
fn seek_position(
    old: u64,
    size: u64,
    offset: i64,
    whence: i32,
    directory: bool,
) -> Result<u64, u64> {
    let base = match whence {
        0 => 0,
        1 => old,
        2 if !directory => size,
        _ => return Err(errno::EINVAL as u64),
    };
    let pos = i128::from(base) + i128::from(offset);
    if !(0..=i128::from(i64::MAX)).contains(&pos) {
        return Err(errno::EINVAL as u64);
    }
    Ok(pos as u64)
}
