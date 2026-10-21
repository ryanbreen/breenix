//! Mode and ownership updates. Resolve once, then read and write the same
//! inode under its filesystem write guard. Errors after a write allocate nothing.

use super::{errno::*, SyscallResult};
use crate::fs::{ext2, namei, permissions::Credentials};
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

/// The ext2 directory an open descriptor's kind holds; ENOTDIR for any other.
pub(crate) fn directory_of(kind: FdKind) -> Result<namei::WorkingDir, u64> {
    let FdKind::Directory(dir) = kind else {
        return Err(ENOTDIR as u64);
    };
    let handle = dir.lock().handle.clone();
    let mount = if ext2::home_mount_id() == Some(handle.object.mount.mount_id) {
        namei::Mount::Home
    } else {
        namei::Mount::Root
    };
    let guard = mount.read()?;
    let fs = guard.as_ref().ok_or(EIO as u64)?;
    let ino = handle.verify(fs).map_err(|_| EIO as u64)?;
    if !fs.read_inode(ino).map_err(|_| EIO as u64)?.is_dir() {
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

enum Change {
    Mode(u32),
    Owner(u32, u32),
}

fn update(fs: &mut ext2::Ext2Fs, ino: u32, change: Change, cred: &Credentials) -> SyscallResult {
    let mut inode = match fs.read_inode(ino) {
        Ok(inode) => inode,
        Err(_) => return SyscallResult::Err(EIO as u64),
    };
    if let Err(errno) = apply(&mut inode, change, cred) {
        return SyscallResult::Err(errno);
    }
    inode.update_timestamps(false, false, true);
    match fs.write_inode(ino, &inode) {
        Ok(()) => SyscallResult::Ok(0),
        Err(_) => SyscallResult::Err(EIO as u64),
    }
}

fn apply(inode: &mut ext2::Ext2Inode, change: Change, cred: &Credentials) -> Result<(), u64> {
    let uid = inode.uid();
    let gid = inode.gid();
    if cred.euid != 0 && cred.euid != uid {
        return Err(EPERM as u64);
    }
    match change {
        Change::Mode(mode) => {
            let mut bits = (mode & 0o7777) as u16;
            // An unprivileged owner cannot set SGID to a group it is not in.
            if cred.euid != 0 && !cred.in_group(gid) {
                bits &= !0o2000;
            }
            inode.i_mode = (inode.i_mode & 0o170000) | bits;
        }
        Change::Owner(owner, group) => {
            if cred.euid != 0
                && ((owner != u32::MAX && owner != uid)
                    || (group != u32::MAX && group != gid && !cred.in_group(group)))
            {
                return Err(EPERM as u64);
            }
            inode.set_owner(
                if owner == u32::MAX { uid } else { owner },
                if group == u32::MAX { gid } else { group },
            );
            // Breenix clears both execution privilege bits on regular files
            // even for root. Directories retain SGID for group inheritance.
            if !inode.is_dir() && (inode.is_file() || cred.euid != 0) {
                inode.i_mode &= !0o6000;
            }
        }
    }
    Ok(())
}

fn fifo(
    entry: &alloc::sync::Arc<spin::Mutex<crate::ipc::fifo::FifoEntry>>,
    change: Change,
    cred: &Credentials,
) -> SyscallResult {
    let Some(handle) = entry.lock().handle.clone() else { return SyscallResult::Err(EIO as u64); };
    let mut guard = match ext2::write_mount(handle.object.mount) {
        Ok(g) => g, Err(error) => return SyscallResult::Err(crate::fs::namei::fs_errno(error)),
    };
    let Some(fs) = guard.as_mut() else { return SyscallResult::Err(EIO as u64); };
    let ino = match handle.verify(fs) { Ok(ino) => ino, Err(_) => return SyscallResult::Err(EIO as u64) };
    update(fs, ino, change, cred)

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
    let (mount, ino) = match resolved.target {
        namei::Target::Inode { mount, ino, .. } => (mount, ino),
        namei::Target::Absent { .. } => return SyscallResult::Err(ENOENT as u64),
        namei::Target::Virtual => return SyscallResult::Err(EOPNOTSUPP as u64),
    };
    let mut guard = match mount.write() { Ok(guard) => guard, Err(errno) => return SyscallResult::Err(errno) };
    match guard.as_mut() {
        Some(fs) => update(fs, ino, change, &cred),
        None => SyscallResult::Err(EIO as u64),
    }
}

// AT_EMPTY_PATH selects the held object, without searching a pathname.
fn by_cwd(change: Change) -> SyscallResult {
    let cred = match Credentials::current(false) {
        Ok(cred) => cred,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let (mount, handle) = match namei::current_working_dir() {
        namei::WorkingDir::Root => (namei::Mount::Root, None),
        namei::WorkingDir::Ext2 { mount, dir } => (mount, Some(dir)),
        namei::WorkingDir::Virtual(_) => return SyscallResult::Err(EOPNOTSUPP as u64),
    };
    let mut guard = match mount.write() { Ok(guard) => guard, Err(errno) => return SyscallResult::Err(errno) };
    let Some(fs) = guard.as_mut() else {
        return SyscallResult::Err(EIO as u64);
    };
    let ino = match handle.as_ref() {
        Some(handle) => match handle.verify(fs) {
            Ok(ino) => ino,
            Err(_) => return SyscallResult::Err(EIO as u64),
        },
        None => ext2::root_inode(),
    };
    update(fs, ino, change, &cred)
}

fn by_fd(fd: i32, change: Change) -> SyscallResult {
    let cred = match Credentials::current(false) {
        Ok(cred) => cred,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let entry = match descriptor_entry(fd) { Ok(entry) => entry, Err(e) => return SyscallResult::Err(e) };
    if let Some(handle) = &entry.native_inode {
        let mut guard = match ext2::write_mount(handle.object.mount) { Ok(g) => g, Err(error) => return SyscallResult::Err(crate::fs::namei::fs_errno(error)) };
        let Some(fs) = guard.as_mut() else { return SyscallResult::Err(EIO as u64); };
        let ino = match handle.verify(fs) { Ok(ino) => ino, Err(_) => return SyscallResult::Err(EIO as u64) };
        return update(fs, ino, change, &cred);
    }
    let kind = entry.kind;
    match kind {
        FdKind::Ipc(object) => {
            let result = match change {
                Change::Mode(mode) => object.chmod(mode, &cred),
                Change::Owner(uid, gid) => object.chown(uid, gid, &cred),
            };
            match result { Ok(()) => SyscallResult::Ok(0), Err(e) => SyscallResult::Err(e) }
        }
        FdKind::RegularFile(file) => {
            let handle = file.lock().handle.clone();
            let mut guard = match ext2::write_mount(handle.object.mount) {
                Ok(g) => g,
                Err(error) => return SyscallResult::Err(crate::fs::namei::fs_errno(error)),
            };
            let Some(fs) = guard.as_mut() else {
                return SyscallResult::Err(EIO as u64);
            };
            let ino = match handle.verify(fs) {
                Ok(ino) => ino,
                Err(_) => return SyscallResult::Err(EIO as u64),
            };
            update(fs, ino, change, &cred)
        }
        FdKind::Directory(dir) => {
            let handle = dir.lock().handle.clone();
            let mut guard = match ext2::write_mount(handle.object.mount) {
                Ok(g) => g,
                Err(error) => return SyscallResult::Err(crate::fs::namei::fs_errno(error)),
            };
            let Some(fs) = guard.as_mut() else {
                return SyscallResult::Err(EIO as u64);
            };
            let ino = match handle.verify(fs) {
                Ok(ino) => ino,
                Err(_) => return SyscallResult::Err(EIO as u64),
            };
            update(fs, ino, change, &cred)
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
