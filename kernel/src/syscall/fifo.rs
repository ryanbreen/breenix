//! FIFO-related syscalls
//!
//! Implements: mkfifo (via mknod with S_IFIFO)

use super::userptr::copy_cstr_from_user;
use super::SyscallResult;
use crate::ipc::fifo::FIFO_REGISTRY;

/// sys_mkfifo - Create a FIFO (named pipe)
///
/// # Arguments
/// * `pathname` - Path for the FIFO (userspace pointer)
/// * `mode` - File creation mode (permissions)
///
/// # Returns
/// 0 on success, negative errno on failure
pub fn sys_mkfifo(pathname: u64, mode: u32) -> SyscallResult {
    use crate::fs::namei::{Last, Target};

    let raw_path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };
    // A FIFO is registered under the physical pathname of its new name.
    let resolved = match super::metadata::resolve_create_at(super::at_path::AT_FDCWD, &raw_path, &crate::fs::permissions::Credentials::current(false)) {
        Ok(r) => r,
        Err(errno) => return SyscallResult::Err(errno),
    };
    if resolved.last != Last::Name || matches!(resolved.target, Target::Inode { .. }) {
        return SyscallResult::Err(super::errno::EEXIST as u64);
    }
    if resolved.target == Target::Virtual {
        return SyscallResult::Err(super::errno::EPERM as u64);
    }
    // Only a directory is created at a name ending in `/`.
    if resolved.trailing_slash {
        return SyscallResult::Err(super::errno::ENOENT as u64);
    }

    let path = resolved.path.as_str();
    log::debug!("sys_mkfifo: path={}, mode={:#o}", path, mode);

    // Create the FIFO in the registry
    let cred = crate::fs::permissions::Credentials::current(false);
    // The namei walk checks search permission; creation also needs write
    // permission on the parent and inherits its group when setgid is set.
    let gid = match check_parent_access(&resolved, &cred, None) {
        Ok(gid) => gid,
        Err(errno) => return SyscallResult::Err(errno),
    };
    match FIFO_REGISTRY.create(path, mode & 0o7777 & !cred.umask, cred.euid, gid) {
        Ok(()) => {
            log::info!("Created FIFO: {}", path);
            SyscallResult::Ok(0)
        }
        Err(errno) => {
            log::debug!("sys_mkfifo failed: errno={}", errno);
            SyscallResult::Err(errno as u64)
        }
    }
}

/// sys_mknod - Create a special file (device, FIFO, etc.)
///
/// This is a more general interface that can create FIFOs among other things.
/// Currently only supports S_IFIFO mode.
///
/// # Arguments
/// * `pathname` - Path for the special file (userspace pointer)
/// * `mode` - File type and permissions (S_IFIFO | perms)
/// * `dev` - Device number (ignored for FIFOs)
///
/// # Returns
/// 0 on success, negative errno on failure
pub fn sys_mknod(pathname: u64, mode: u32, _dev: u64) -> SyscallResult {
    use super::fs::S_IFIFO;
    use super::fs::S_IFMT;

    let file_type = mode & S_IFMT;

    if file_type == S_IFIFO {
        // Creating a FIFO - delegate to mkfifo
        let perms = mode & 0o7777;
        sys_mkfifo(pathname, perms)
    } else {
        // Other special file types not yet implemented
        log::warn!("sys_mknod: file type {:#o} not supported", file_type);
        SyscallResult::Err(38) // ENOSYS
    }
}

/// Check a registry FIFO's real parent without holding a FIFO/PM lock.
/// An owner selects unlink's sticky-directory rule; creation omits it.
pub(crate) fn check_parent_access(
    resolved: &crate::fs::namei::Resolved,
    cred: &crate::fs::permissions::Credentials,
    owner: Option<u32>,
) -> Result<u32, u64> {
    use super::errno::{EACCES, EIO, ENOENT, EPERM};
    use crate::fs::namei::Target;
    let Target::Absent { mount, parent } = resolved.target else {
        return Err(ENOENT as u64);
    };
    let guard = mount.read();
    let fs = guard.as_ref().ok_or(EIO as u64)?;
    let inode = fs.read_inode(parent).map_err(|_| EIO as u64)?;
    if !cred.permits(&inode, 3) {
        return Err(EACCES as u64);
    }
    if let Some(owner) = owner {
        if inode.permissions() & 0o1000 != 0
            && cred.euid != 0
            && cred.euid != inode.uid()
            && cred.euid != owner
        {
            return Err(EPERM as u64);
        }
    }
    Ok(if inode.permissions() & 0o2000 != 0 {
        inode.gid()
    } else {
        cred.egid
    })
}
