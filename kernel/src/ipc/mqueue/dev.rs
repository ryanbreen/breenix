//! Mqueue pathname routing seams, implemented by the MQ lane.
use crate::syscall::SyscallResult;
use crate::fs::permissions::Credentials;
pub fn virtual_entry(path:&str)->Option<Option<bool>>{
    if path == "/dev/mqueue" { return Some(Some(true)); }
    None
}
pub fn open_path(_path:&str,_flags:u32,_mode:u32)->Option<SyscallResult>{None}
pub fn unlink_path(_path:&str)->Option<SyscallResult>{None}
pub fn stat_path(_path:&str,_statbuf:u64)->Option<SyscallResult>{None}
pub fn access_path(_path:&str,_mode:u32,_cred:&Credentials)->Option<SyscallResult>{None}
