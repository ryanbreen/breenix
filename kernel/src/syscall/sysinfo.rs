//! sysinfo(2): Linux's `struct sysinfo`, filled from kernel counters.

use super::SyscallResult;

/// Linux's 64-bit `struct sysinfo` (112 bytes). Padding is named so the copy
/// to user space carries zeros, never kernel stack bytes.
#[repr(C)]
#[derive(Clone, Copy)]
struct SysInfo {
    uptime: i64,
    loads: [u64; 3],
    totalram: u64,
    freeram: u64,
    sharedram: u64,
    bufferram: u64,
    totalswap: u64,
    freeswap: u64,
    procs: u16,
    pad: u16,
    pad2: u32,
    totalhigh: u64,
    freehigh: u64,
    mem_unit: u32,
    pad3: u32,
}

const _: () = assert!(core::mem::size_of::<SysInfo>() == 112);

pub fn sys_sysinfo(out: u64) -> SyscallResult {
    if out == 0 {
        return SyscallResult::Err(super::errno::EFAULT as u64);
    }
    // Linux reports boot time in whole seconds, rounding a partial second up.
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    let stats = crate::memory::frame_allocator::memory_stats();
    let used = stats.allocated_frames.saturating_sub(stats.free_list_frames) as u64 * 4096;
    let procs = {
        let guard = crate::process::manager();
        guard.as_ref().map_or(0, |manager| manager.process_count())
    };
    let info = SysInfo {
        uptime: (secs + (nanos != 0) as u64) as i64,
        // Breenix keeps no load average yet.
        loads: [0; 3],
        totalram: stats.total_bytes,
        freeram: stats.total_bytes.saturating_sub(used),
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: procs.min(u16::MAX as usize) as u16,
        pad: 0,
        pad2: 0,
        totalhigh: 0,
        freehigh: 0,
        mem_unit: 1,
        pad3: 0,
    };
    match super::userptr::copy_to_user(out as *mut SysInfo, &info) {
        Ok(()) => SyscallResult::Ok(0),
        Err(errno) => SyscallResult::Err(errno),
    }
}
