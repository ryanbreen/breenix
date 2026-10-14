//! chroot: change the calling process's root directory.

use super::errno::EPERM;
use super::SyscallResult;

/// chroot(path): make the directory `path` names the calling process's root
/// directory, which `/` names from then on and which `..` does not leave. The
/// working directory does not move. Only a privileged caller (effective user
/// ID 0, Breenix's stand-in for CAP_SYS_CHROOT) may change it.
///
/// # Errors
/// * EFAULT, ENAMETOOLONG, ENOENT, ENOTDIR, ELOOP - `path` names no directory
/// * EACCES - a component of `path`, or the directory itself, may not be searched
/// * EPERM - the caller is not privileged
pub fn sys_chroot(path: u64) -> SyscallResult {
    match chroot(path) {
        Ok(()) => SyscallResult::Ok(0),
        Err(errno) => SyscallResult::Err(errno),
    }
}

fn chroot(path: u64) -> Result<(), u64> {
    let path = super::userptr::copy_path_bytes_from_user(path)?;
    let cred = crate::fs::permissions::Credentials::current(false);
    let dir = {
        let resolved = crate::fs::namei::resolve_bytes(&path, true)?;
        crate::fs::namei::working_dir(&resolved)?
    };
    crate::fs::namei::may_search(&dir, &cred)?;
    if cred.euid != 0 {
        return Err(EPERM as u64);
    }
    crate::fs::namei::set_root(dir)
}
