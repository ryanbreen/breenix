//! FIFO-related syscalls
//!
//! Implements: mkfifo (via mknod with S_IFIFO)

use super::SyscallResult;
use super::userptr::copy_cstr_from_user;
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
    // Copy path from userspace
    let raw_path = match copy_cstr_from_user(pathname) {
        Ok(p) => p,
        Err(errno) => return SyscallResult::Err(errno),
    };

    // Normalize path
    let path = if raw_path.starts_with('/') {
        raw_path
    } else {
        // Get current process's cwd
        let cwd = super::fs::get_current_cwd().unwrap_or_else(|| alloc::string::String::from("/"));
        let absolute = if cwd.ends_with('/') {
            alloc::format!("{}{}", cwd, raw_path)
        } else {
            alloc::format!("{}/{}", cwd, raw_path)
        };
        super::fs::normalize_path(&absolute)
    };

    log::debug!("sys_mkfifo: path={}, mode={:#o}", path, mode);

    // Create the FIFO in the registry
    match FIFO_REGISTRY.create(&path, mode) {
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
    use super::fs::S_IFMT;
    use super::fs::S_IFIFO;

    let file_type = mode & S_IFMT;

    if file_type == S_IFIFO {
        // Creating a FIFO - delegate to mkfifo
        let perms = mode & 0o777;
        sys_mkfifo(pathname, perms)
    } else {
        // Other special file types not yet implemented
        log::warn!("sys_mknod: file type {:#o} not supported", file_type);
        SyscallResult::Err(38) // ENOSYS
    }
}
