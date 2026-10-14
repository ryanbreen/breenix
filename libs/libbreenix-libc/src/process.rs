//! Process groups and priorities, written from POSIX.1-2024 and the Linux syscall ABI.
use crate::pthread::{errno, set_errno};
use libbreenix::{raw, syscall::nr};

#[cfg(target_arch = "aarch64")]
const GETPRIORITY: u64 = 141;
#[cfg(target_arch = "x86_64")]
const GETPRIORITY: u64 = 140;
#[cfg(target_arch = "aarch64")]
const SETPRIORITY: u64 = 140;
#[cfg(target_arch = "x86_64")]
const SETPRIORITY: u64 = 141;

#[no_mangle]
pub extern "C" fn getpgid(pid: i32) -> i32 {
    crate::syscall_result_to_c_int(unsafe { raw::syscall1(nr::GETPGID, pid as i64 as u64) } as i64)
}
#[no_mangle]
pub extern "C" fn getpgrp() -> i32 {
    getpgid(0)
}
#[no_mangle]
pub extern "C" fn setsid() -> i32 {
    crate::syscall_result_to_c_int(unsafe { raw::syscall0(nr::SETSID) } as i64)
}
#[no_mangle]
pub extern "C" fn killpg(group: i32, signal: i32) -> i32 {
    // Preserve the group-zero extension, but reject the all-process selector.
    // POSIX leaves groups <= 1 undefined.
    if group < 0 || group == 1 {
        set_errno(22);
        return -1;
    }
    crate::kill(-group, signal)
}
#[no_mangle]
pub extern "C" fn getpriority(which: i32, who: u32) -> i32 {
    let result = unsafe { raw::syscall2(GETPRIORITY, which as u64, who as u64) } as i64;
    if result < 0 {
        crate::syscall_result_to_c_int(result)
    } else {
        20 - result as i32
    }
}
#[no_mangle]
pub extern "C" fn setpriority(which: i32, who: u32, priority: i32) -> i32 {
    crate::syscall_result_to_c_int(unsafe {
        raw::syscall3(
            SETPRIORITY,
            which as u64,
            who as u64,
            priority as i64 as u64,
        )
    } as i64)
}
#[no_mangle]
pub extern "C" fn nice(increment: i32) -> i32 {
    let saved = errno();
    set_errno(0);
    let old = getpriority(0, 0);
    if old == -1 && errno() != 0 {
        return -1;
    }
    if setpriority(0, 0, old.saturating_add(increment).clamp(-20, 19)) != 0 {
        // POSIX nice reports EPERM when permission to decrease the nice value is denied.
        if errno() == 13 {
            set_errno(1);
        }
        return -1;
    }
    let new = getpriority(0, 0);
    if new != -1 || errno() == 0 {
        set_errno(saved);
    }
    new
}
#[no_mangle]
pub unsafe extern "C" fn sethostname(_name: *const u8, _len: usize) -> i32 {
    // No Breenix syscall implements this operation. Avoid unknown-syscall diagnostics.
    set_errno(38);
    -1
}
