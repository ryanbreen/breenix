//! POSIX shared-memory names over the flat /dev/shm namespace.
const EINVAL: i64 = 22;
const ENAMETOOLONG: i64 = 36;
#[cfg(target_arch = "x86_64")]
const O_NOFOLLOW: i32 = 0x20000;
#[cfg(target_arch = "aarch64")]
const O_NOFOLLOW: i32 = 0x8000;
const O_CLOEXEC: i32 = 0x80000;

/// Breenix accepts a leading slash and a single nonempty component, at most
/// 255 bytes including the optional named-semaphore prefix.
pub(crate) unsafe fn name_with_prefix(
    name: *const u8,
    prefix: &[u8],
    out: &mut [u8],
) -> Result<(), i64> {
    if name.is_null() || *name != b'/' {
        return Err(EINVAL);
    }
    if *name.add(1) == 0 {
        return Err(EINVAL);
    }
    out[..prefix.len()].copy_from_slice(prefix);
    let mut len = prefix.len();
    let mut index = 1;
    loop {
        let byte = *name.add(index);
        if byte == 0 {
            out[len] = 0;
            return Ok(());
        }
        if byte == b'/' {
            return Err(EINVAL);
        }
        if len + 1 >= out.len() {
            return Err(ENAMETOOLONG);
        }
        out[len] = byte;
        len += 1;
        index += 1;
    }
}

#[no_mangle]
pub unsafe extern "C" fn shm_open(name: *const u8, oflag: i32, mode: u32) -> i32 {
    let mut path = [0u8; 265];
    if let Err(e) = name_with_prefix(name, b"/dev/shm/", &mut path) {
        return crate::syscall_result_to_c_int(-e);
    }
    // POSIX specifies only these two access modes for shared memory.
    if oflag & 3 != 0 && oflag & 3 != 2 {
        return crate::syscall_result_to_c_int(-EINVAL);
    }
    crate::open(path.as_ptr(), oflag | O_NOFOLLOW | O_CLOEXEC, mode)
}
#[no_mangle]
pub unsafe extern "C" fn shm_unlink(name: *const u8) -> i32 {
    let mut path = [0u8; 265];
    if let Err(e) = name_with_prefix(name, b"/dev/shm/", &mut path) {
        return crate::syscall_result_to_c_int(-e);
    }
    crate::unlink(path.as_ptr())
}
