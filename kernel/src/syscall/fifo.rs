//! Filesystem node creation through mknod/mknodat.
use super::{errno::*, SyscallResult};
use crate::fs::{
    attrs::InodeAttrs,
    namei::{Last, Target},
    permissions::Credentials,
    vfs::{names, NewNode},
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
    let raw = match super::userptr::copy_path_bytes_from_user(pathname) {
        Ok(path) => path,
        Err(e) => return SyscallResult::Err(e),
    };
    mknodat_path(dirfd, &raw, mode, dev)
}
pub(crate) fn mknodat_path(dirfd: i32, raw: impl AsRef<[u8]>, mode: u32, dev: u64) -> SyscallResult {
    let raw = raw.as_ref();
    let kind = match super::fifo_policy::node_type(mode) {
        Ok(kind) => kind,
        Err(e) => return SyscallResult::Err(e),
    };
    let cred = match Credentials::current(false) {
        Ok(cred) => cred,
        Err(errno) => return SyscallResult::Err(errno),
    };
    let resolved = match super::metadata::resolve_create_at(dirfd, &raw, &cred) {
        Ok(r) => r,
        Err(e) => return SyscallResult::Err(e),
    };
    if matches!(resolved.target, Target::User { .. }) {
        return SyscallResult::Err(EPERM as u64);
    }
    if resolved.last != Last::Name || matches!(resolved.target, Target::Inode { .. }) {
        return SyscallResult::Err(EEXIST as u64);
    }
    let Target::Absent { .. } = resolved.target else {
        return SyscallResult::Err(EPERM as u64);
    };
    if resolved.trailing_slash {
        return SyscallResult::Err(ENOENT as u64);
    }
    let (fs, parent, attrs) = match names::entry_dir(&resolved) {
        Ok(parent) => parent,
        Err(errno) => return SyscallResult::Err(errno.into()),
    };
    let gid = match super::fifo_policy::check_parent_access(attrs.attrs, attrs.nlink, &cred, None) {
        Ok(gid) => gid,
        Err(e) => return SyscallResult::Err(e),
    };
    match fs.lookup(parent.ino, resolved.final_name()) {
        Ok(_) => return SyscallResult::Err(EEXIST as u64),
        Err(crate::fs::errno::Errno(ENOENT)) => {}
        Err(e) => return SyscallResult::Err(e.into()),
    }
    // Linux's zero-device character whiteout is exempt from CAP_MKNOD.
    let device_node = crate::process::privilege::Operation::DeviceNode;
    if (kind == 0o020000 || kind == 0o060000)
        && !(kind == 0o020000 && dev as u32 == 0)
        && !crate::process::privilege::allowed(&cred, device_node)
    {
        return SyscallResult::Err(device_node.errno());
    }
    let mode = super::fifo_policy::creation_mode(mode, &cred, gid, false);
    let attrs = InodeAttrs::new(mode.into(), cred.euid, gid);
    // Keep the syscall's existing device-number truncation at its ABI boundary.
    let rdev = u64::from(dev as u32);
    let node = match kind {
        0o100000 => NewNode::Regular { attrs },
        0o010000 => NewNode::Fifo { attrs },
        0o020000 => NewNode::CharDevice { attrs, rdev },
        0o060000 => NewNode::BlockDevice { attrs, rdev },
        0o140000 => NewNode::Socket { attrs },
        _ => return SyscallResult::Err(EINVAL as u64),
    };
    match fs.create(parent.ino, resolved.final_name(), node) {
        Ok(_) => SyscallResult::Ok(0),
        Err(error) => SyscallResult::Err(error.into()),
    }
}
