//! sysinfo, the Linux syscall and its 64-bit `struct sysinfo` layout.
use libbreenix::raw;

#[cfg(target_arch = "aarch64")]
const SYSINFO: u64 = 179;
#[cfg(target_arch = "x86_64")]
const SYSINFO: u64 = 99;

#[repr(C)]
pub struct SysInfo {
    pub uptime: i64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
}
#[no_mangle]
pub unsafe extern "C" fn sysinfo(out: *mut SysInfo) -> i32 {
    // The kernel fills the structure and reports EFAULT for any unwritable pointer.
    crate::syscall_result_to_c_int(raw::syscall1(SYSINFO, out as u64) as i64)
}
