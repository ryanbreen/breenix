//! Mode and ownership updates through the filesystem interface.

use super::{errno::*, SyscallResult};
use crate::fs::{ext2::errno::errno as ext2_errno, namei, permissions::Credentials};
use crate::fs::vfs::{self, FileSystem, attributes::{attribute_change, AttributeChange as Change}};
use crate::ipc::FdKind;

use super::at_path::AT_FDCWD;
const AT_SYMLINK_NOFOLLOW: u32 = 0x100;
const AT_EMPTY_PATH: u32 = 0x1000;

pub(crate) fn descriptor(fd: i32) -> Result<FdKind, u64> {
    descriptor_entry(fd).map(|entry| entry.kind)
}

pub(crate) fn descriptor_entry(fd: i32) -> Result<crate::ipc::fd::FileDescriptor, u64> {
    let tid = crate::task::scheduler::current_thread_id().ok_or(ESRCH as u64)?;
    let guard = crate::process::manager();
    guard
        .as_ref()
        .and_then(|m| m.find_process_by_thread(tid))
        .and_then(|(_, p)| p.fd_table.get(fd))
        .cloned()
        .ok_or(EBADF as u64)
}

pub(crate) fn directory(fd: i32) -> Result<namei::WorkingDir, u64> {
    directory_of(descriptor(fd)?)
}

/// The directory an open descriptor holds; ENOTDIR for a non-directory.
pub(crate) fn directory_of(kind: FdKind) -> Result<namei::WorkingDir, u64> {
    if let FdKind::Ipc(ref object) = kind {
        if let Some(file) = object.as_any().downcast_ref::<crate::fs::ufs::file::UfsFile>() {
            if !file.directory { return Err(ENOTDIR as u64); }
            return Ok(namei::WorkingDir::User { mount: file.mount, node: file.node, handle: object.clone() });
        }
    }
    let FdKind::Directory(dir) = kind else {
        return Err(ENOTDIR as u64);
    };
    let handle = dir.lock().open.ext2_handle().ok_or(EIO as u64)?;
    let mount = namei::Mount(handle.object.mount.mount_id);
    let guard = mount.read()?;
    let fs = guard.as_ref().ok_or(EIO as u64)?;
    let ino = handle.verify(fs).map_err(ext2_errno)?;
    if !fs.read_inode(ino).map_err(ext2_errno)?.is_dir() {
        return Err(ENOTDIR as u64);
    }
    let dir = handle;
    Ok(namei::WorkingDir::Ext2 { mount, dir })
}

pub(crate) fn resolve_at(
    fd: i32,
    path: &[u8],
    follow: bool,
    cred: &Credentials,
) -> Result<namei::Resolved<alloc::vec::Vec<u8>>, u64> {
    let start = super::at_path::directory_start(fd, path.starts_with(b"/"), directory)?;
    namei::resolve_from(path, follow, start, cred)
}

pub(crate) fn resolve_create_at(fd: i32, path: &[u8], cred: &Credentials) -> Result<namei::Resolved<alloc::vec::Vec<u8>>, u64> {
    let start = super::at_path::directory_start(fd, path.starts_with(b"/"), directory)?;
    namei::resolve_create_from(path, start, cred)
}

fn update(fs: &dyn FileSystem, ino: u64, change: Change, cred: &Credentials) -> SyscallResult {
    // Bound metadata contention so the syscall returns to signal delivery.
    for _ in 0..16 {
        let attrs = match fs.getattr(ino) {
            Ok(attrs) => attrs.attrs,
            Err(error) => return SyscallResult::Err(error.into()),
        };
        let request = match attribute_change(attrs, change, cred) {
            Ok(request) => request,
            Err(error) => return SyscallResult::Err(error.into()),
        };
        match fs.setattr(ino, request) {
            Err(error) if error.0 == EAGAIN => continue,
            Ok(()) => return SyscallResult::Ok(0),
            Err(error) => return SyscallResult::Err(error.into()),
        }
    }
    SyscallResult::Err(EAGAIN as u64)
}

fn fifo(
    entry: &alloc::sync::Arc<spin::Mutex<crate::ipc::fifo::FifoEntry>>,
    change: Change,
    cred: &Credentials,
) -> SyscallResult {
    let Some(handle) = entry.lock().handle.clone() else { return SyscallResult::Err(EIO as u64); };
    match vfs::names::held(&handle) {
        Ok((fs, node)) => update(fs.as_ref(), node.ino, change, cred),
        Err(error) => SyscallResult::Err(error.into()),
    }
}

fn pathname(fd: i32, pathname: u64, follow: bool, empty: bool, change: Change) -> SyscallResult {
    let path = match super::userptr::copy_path_bytes_from_user(pathname) {
        Ok(path) => path,
        Err(errno) => return SyscallResult::Err(errno),
    };
    if empty && path.is_empty() {
        return if fd == AT_FDCWD { by_cwd(change) } else { by_fd(fd, change) };
    }
    let cred = match Credentials::current(false) {
        Ok(cred) => cred,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let resolved = match resolve_at(fd, &path, follow, &cred) {
        Ok(r) => r,
        Err(errno) => return SyscallResult::Err(errno),
    };
    match resolved.target {
        namei::Target::Absent { .. } | namei::Target::User { node: 0, .. } => return SyscallResult::Err(ENOENT as u64),
        namei::Target::Virtual | namei::Target::User { .. } => return SyscallResult::Err(EOPNOTSUPP as u64),
        namei::Target::Inode { .. } => {}
    }
    match vfs::names::target(&resolved) {
        Ok((fs, node)) => update(fs.as_ref(), node.ino, change, &cred),
        Err(error) => SyscallResult::Err(error.into()),
    }
}

// AT_EMPTY_PATH selects the held object, without searching a pathname.
fn by_cwd(change: Change) -> SyscallResult {
    let cred = match Credentials::current(false) {
        Ok(cred) => cred,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let target = match namei::current_working_dir() {
        namei::WorkingDir::Root => vfs::mount::filesystem(namei::Mount::root().0)
            .and_then(|fs| fs.root().and_then(|ino| fs.open(ino)).map(|node| (fs, node))),
        namei::WorkingDir::Ext2 { dir, .. } => vfs::names::held(&dir),
        namei::WorkingDir::Virtual(_) | namei::WorkingDir::User { .. } => return SyscallResult::Err(EOPNOTSUPP as u64),
    };
    match target {
        Ok((fs, node)) => update(fs.as_ref(), node.ino, change, &cred),
        Err(error) => SyscallResult::Err(error.into()),
    }
}

fn by_fd(fd: i32, change: Change) -> SyscallResult {
    let cred = match Credentials::current(false) {
        Ok(cred) => cred,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let entry = match descriptor_entry(fd) { Ok(entry) => entry, Err(e) => return SyscallResult::Err(e) };
    // A terminal revoked from this description changes through it no more.
    if entry.terminal_revoked() {
        return SyscallResult::Err(EIO as u64);
    }
    if let Some(handle) = &entry.native_inode {
        return match vfs::names::held(handle) {
            Ok((fs, node)) => update(fs.as_ref(), node.ino, change, &cred),
            Err(error) => SyscallResult::Err(error.into()),
        };
    }
    let kind = entry.kind;
    match kind {
        FdKind::PtySlave(pair) => {
            let result = match change {
                Change::Mode(mode) => pair.change_slave_metadata(Some(mode), None, &cred),
                Change::Owner(uid, gid) => pair.change_slave_metadata(None, Some((uid, gid)), &cred),
            };
            match result { Ok(()) => SyscallResult::Ok(0), Err(error) => SyscallResult::Err(error) }
        }
        FdKind::Device(crate::fs::devfs::DeviceType::Vt(n)) => {
            let result = match change {
                Change::Mode(mode) => crate::fs::devfs::change_vt_metadata(n, Some(mode), None, &cred),
                Change::Owner(uid, gid) => crate::fs::devfs::change_vt_metadata(n, None, Some((uid, gid)), &cred),
            };
            match result { Ok(()) => SyscallResult::Ok(0), Err(error) => SyscallResult::Err(error) }
        }
        FdKind::Ipc(object) => {
            let result = match change {
                Change::Mode(mode) => object.chmod(mode, &cred),
                Change::Owner(uid, gid) => object.chown(uid, gid, &cred),
            };
            match result { Ok(()) => SyscallResult::Ok(0), Err(e) => SyscallResult::Err(e) }
        }
        FdKind::RegularFile(file) => {
            let file = file.lock();
            let (fs, node) = (file.open.fs.clone(), file.open.node.clone());
            drop(file);
            update(fs.as_ref(), node.ino, change, &cred)
        }
        FdKind::Directory(dir) => {
            let dir = dir.lock();
            let (fs, node) = (dir.open.fs.clone(), dir.open.node.clone());
            drop(dir);
            update(fs.as_ref(), node.ino, change, &cred)
        }
        FdKind::FifoRead(_, _, entry) | FdKind::FifoWrite(_, _, entry) | FdKind::FifoReadWrite(_, _, entry) => {
            fifo(&entry, change, &cred)
        }
        _ => SyscallResult::Err(EOPNOTSUPP as u64),
    }
}

pub fn sys_chmod(path: u64, mode: u32) -> SyscallResult {
    pathname(AT_FDCWD, path, true, false, Change::Mode(mode))
}
pub fn sys_fchmod(fd: i32, mode: u32) -> SyscallResult {
    by_fd(fd, Change::Mode(mode))
}
pub fn sys_fchmodat(fd: i32, path: u64, mode: u32) -> SyscallResult {
    pathname(fd, path, true, false, Change::Mode(mode))
}
pub fn sys_chown(path: u64, uid: u32, gid: u32) -> SyscallResult {
    pathname(AT_FDCWD, path, true, false, Change::Owner(uid, gid))
}
pub fn sys_lchown(path: u64, uid: u32, gid: u32) -> SyscallResult {
    pathname(AT_FDCWD, path, false, false, Change::Owner(uid, gid))
}
pub fn sys_fchown(fd: i32, uid: u32, gid: u32) -> SyscallResult {
    by_fd(fd, Change::Owner(uid, gid))
}
pub fn sys_fchownat(fd: i32, path: u64, uid: u32, gid: u32, flags: u32) -> SyscallResult {
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return SyscallResult::Err(EINVAL as u64);
    }
    pathname(
        fd,
        path,
        flags & AT_SYMLINK_NOFOLLOW == 0,
        flags & AT_EMPTY_PATH != 0,
        Change::Owner(uid, gid),
    )
}
