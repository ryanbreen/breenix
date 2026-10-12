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
