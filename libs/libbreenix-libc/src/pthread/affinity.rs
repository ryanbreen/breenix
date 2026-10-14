//! Affinity follows Breenix's byte-count syscall ABI; sysconf counts online
//! processor records independently of the calling thread's affinity.
use libbreenix::syscall::raw;

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

pub(crate) fn processor_count() -> i64 {
    use super::super::{__errno_location, close_point, open_point, read_point, EINTR, EIO};
    unsafe {
        let fd = open_point(b"/proc/cpuinfo\0".as_ptr(), 0x80000, 0); // O_RDONLY | O_CLOEXEC
        if fd < 0 { return -1; }
        let mut count = super::cpuinfo::ProcessorCount::new();
        let mut buf = [0u8; 256];
        let error = loop {
            let read = read_point(fd, buf.as_mut_ptr(), buf.len());
            if read > 0 {
                count.feed(&buf[..read as usize]);
            } else if read == 0 {
                break if count.get() == 0 { EIO } else { 0 };
            } else if *__errno_location() != EINTR {
                break *__errno_location();
            }
        };
        close_point(fd);
        if error != 0 {
            super::set_errno(error);
            -1
        } else {
            count.get() as i64
        }
    }
}
