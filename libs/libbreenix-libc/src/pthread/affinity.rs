//! Affinity follows Breenix's byte-count syscall ABI; sysconf counts online
//! processor records independently of the calling thread's affinity.
use libbreenix::syscall::raw;

#[cfg(target_arch = "x86_64")]
const SETAFFINITY: u64 = 203;
#[cfg(target_arch = "aarch64")]
const SETAFFINITY: u64 = 122;
#[cfg(target_arch = "x86_64")]
const GETAFFINITY: u64 = 204;
#[cfg(target_arch = "aarch64")]
const GETAFFINITY: u64 = 123;

/// The kernel returns the bytes written, while the C API returns zero and
/// zero-fills the unused part of the caller's set on success.
#[no_mangle]
pub unsafe extern "C" fn sched_getaffinity(pid: i32, len: usize, mask: *mut u8) -> i32 {
    let result = raw::syscall3(GETAFFINITY, pid as u64, len as u64, mask as u64) as i64;
    if result < 0 {
        return super::super::syscall_result_to_c_int(result);
    }
    let written = result as usize;
    if written > len {
        super::set_errno(super::super::EIO);
        return -1;
    }
    mask.add(written).write_bytes(0, len - written);
    0
}

/// The kernel returns zero, as the C API does.
#[no_mangle]
pub unsafe extern "C" fn sched_setaffinity(pid: i32, len: usize, mask: *const u8) -> i32 {
    let result = raw::syscall3(SETAFFINITY, pid as u64, len as u64, mask as u64) as i64;
    super::super::syscall_result_to_c_int(result)
}

/// sysconf's online and configured processor count: the processor records in
/// /proc/cpuinfo. sysconf has no errors to report for these names, so when
/// procfs cannot be read the count falls back to the caller's affinity, and
/// to one processor; errno is left as it was.
pub(crate) fn processor_count() -> i64 {
    use super::super::__errno_location;
    unsafe {
        let saved = *__errno_location();
        let count = match cpuinfo_records() {
            0 => affinity_count().max(1),
            records => records,
        };
        *__errno_location() = saved;
        count as i64
    }
}

/// Processor records in /proc/cpuinfo, or 0 when it cannot be read. Uses the
/// library's own descriptor calls, so sysconf is never a cancellation point.
unsafe fn cpuinfo_records() -> usize {
    use super::super::{__errno_location, close, open, read, EINTR};
    let fd = open(b"/proc/cpuinfo\0".as_ptr(), 0x80000, 0); // O_RDONLY | O_CLOEXEC
    if fd < 0 {
        return 0;
    }
    let mut count = super::cpuinfo::ProcessorCount::new();
    let mut buf = [0u8; 256];
    let complete = loop {
        let got = read(fd, buf.as_mut_ptr(), buf.len());
        if got > 0 {
            count.feed(&buf[..got as usize]);
        } else if got == 0 {
            break true;
        } else if *__errno_location() != EINTR {
            break false;
        }
    };
    close(fd);
    if complete { count.get() } else { 0 }
}

/// The processors the calling thread may run on, or 0 when unknown.
unsafe fn affinity_count() -> usize {
    let mut set = [0u64; 16];
    if sched_getaffinity(0, core::mem::size_of_val(&set), set.as_mut_ptr().cast()) != 0 {
        return 0;
    }
    set.iter().map(|word| word.count_ones() as usize).sum()
}
