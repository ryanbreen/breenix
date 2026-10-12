//! Shared-memory routing seams, owned by the SHMSEM lane.
use crate::memory::vma::{MmapFlags, Protection};
use crate::syscall::SyscallResult;

pub fn open_path(_name: &str, _flags: u32, _mode: u32) -> Option<SyscallResult> {
    None
}
pub fn unlink_path(_path: &str) -> Option<SyscallResult> {
    None
}
pub fn stat_path(_path: &str, _statbuf: u64) -> Option<SyscallResult> {
    None
}
pub fn virtual_entry(_path: &str) -> Option<Option<bool>> {
    None
}
pub fn mmap_fd(
    _fd: i32,
    _addr: u64,
    _len: u64,
    _prot: Protection,
    _flags: MmapFlags,
    _offset: u64,
) -> Option<SyscallResult> {
    None
}

/// The SHMSEM namespace supplies object ownership/mode using the caller's
/// chosen credentials (real IDs for access, effective IDs for AT_EACCESS).
pub fn access_path(path: &str, mode: u32, cred: &crate::fs::permissions::Credentials) -> Option<SyscallResult> {
    if path == "/dev/shm" {
        return Some(if cred.permits_mode(0, 0, crate::fs::devfs::SHM_DIRECTORY_MODE, mode) {
            SyscallResult::Ok(0)
        } else { SyscallResult::Err(13) });
    }
    None
}

/// Object policy checked before mprotect changes any VMA or leaf.
pub fn mprotect_allowed(_vma: &crate::memory::vma::Vma, _prot: Protection) -> bool {
    true
}
/// Allocate lazy object pages outside PM, before revalidating their VMA.
pub fn prepare_population(_thread: u64, _address: u64) -> Result<(), u64> {
    Ok(())
}
/// Called under PM; Some(false) retries a changed/reserved mapping and
/// Some(true) denotes a resident object page. An absent result selects ordinary memory.
pub fn populate_page(_process: &mut crate::process::Process, _address: u64) -> Option<Result<bool, u64>> {
    None
}
