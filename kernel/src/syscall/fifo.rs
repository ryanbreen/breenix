//! Named FIFO creation through Linux mknod/mknodat on ext2.
use super::{errno::*, SyscallResult};
use crate::fs::{
    namei::{Last, Target},
    permissions::Credentials,
};

pub fn sys_mkfifo(pathname: u64, mode: u32) -> SyscallResult {
    sys_mkfifo_at(super::at_path::AT_FDCWD, pathname, mode)
}
pub(crate) fn sys_mkfifo_at(dirfd: i32, pathname: u64, mode: u32) -> SyscallResult {
    sys_mknodat(dirfd, pathname, 0o010000 | (mode & 0o7777), 0)
}
pub fn sys_mknod(pathname: u64, mode: u32, dev: u64) -> SyscallResult {
    sys_mknodat(super::at_path::AT_FDCWD, pathname, mode, dev)
}
pub(crate) fn sys_mknodat(dirfd: i32, pathname: u64, mode: u32, dev: u64) -> SyscallResult {
    if let Err(e) = super::fifo_policy::node_type(mode) {
        return SyscallResult::Err(e);
    }
    let raw = match super::userptr::copy_cstr_from_user(pathname) {
        Ok(path) => path,
        Err(e) => return SyscallResult::Err(e),
    };
    mknodat_path(dirfd, &raw, mode, dev)
}
pub(crate) fn mknodat_path(dirfd: i32, raw: &str, mode: u32, dev: u64) -> SyscallResult {
    let kind = match super::fifo_policy::node_type(mode) {
        Ok(kind) => kind,
        Err(e) => return SyscallResult::Err(e),
    };
    let cred = Credentials::current(false);
    let resolved = match super::metadata::resolve_create_at(dirfd, &raw, &cred) {
        Ok(r) => r,
        Err(e) => return SyscallResult::Err(e),
    };
    if resolved.last != Last::Name || matches!(resolved.target, Target::Inode { .. }) {
        return SyscallResult::Err(EEXIST as u64);
    }
    let Target::Absent { mount, .. } = resolved.target else {
        return SyscallResult::Err(EPERM as u64);
    };
    if resolved.trailing_slash {
        return SyscallResult::Err(ENOENT as u64);
    }
    let Some(parent) = resolved.entry_dir() else {
        return SyscallResult::Err(EIO as u64);
    };
    let mut guard = match mount.write() { Ok(guard) => guard, Err(errno) => return SyscallResult::Err(errno) };
    let Some(fs) = guard.as_mut() else {
        return SyscallResult::Err(EIO as u64);
    };
    let parent = match parent.verify(fs) {
        Ok(ino) => ino,
        Err(_) => return SyscallResult::Err(ENOENT as u64),
    };
    let inode = match fs.read_inode(parent) {
        Ok(inode) => inode,
        Err(e) => return SyscallResult::Err(crate::fs::namei::fs_errno(e)),
    };
    let gid = match super::fifo_policy::check_parent_access(&inode, &cred, None) {
        Ok(gid) => gid,
        Err(e) => return SyscallResult::Err(e),
    };
    match fs.lookup_in_dir(&inode, resolved.final_name()) {
        Ok(Some(_)) => return SyscallResult::Err(EEXIST as u64),
        Ok(None) => {}
        Err(e) => return SyscallResult::Err(crate::fs::namei::fs_errno(e)),
    }
    // Linux's zero-device character whiteout is exempt from CAP_MKNOD.
    if (kind == 0o020000 || kind == 0o060000)
        && !(kind == 0o020000 && dev as u32 == 0)
        && cred.euid != 0
    {
        return SyscallResult::Err(EPERM as u64);
    }
    let mode = kind | super::fifo_policy::creation_mode(mode, &cred, gid, false);
    match fs.create_node(
        parent,
        resolved.final_name(),
        mode,
        cred.euid,
        gid,
        dev as u32,
    ) {
        Ok(_) => SyscallResult::Ok(0),
        Err(error) => SyscallResult::Err(if error.contains("already exists") {
            EEXIST
        } else if error.contains("not found") {
            ENOENT
        } else if error.contains("No free") || error.contains("No space") {
            ENOSPC
        } else if error.contains("memory") {
            ENOMEM
        } else {
            EIO
        } as u64),
    }
}
