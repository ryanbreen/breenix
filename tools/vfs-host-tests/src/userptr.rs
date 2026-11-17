//! Host ioctl callers supply live output objects, without a guest address space.
pub fn copy_to_user<T: Copy>(pointer: *mut T, value: &T) -> Result<(), u64> {
    if pointer.is_null() {
        return Err(crate::syscall_errno::EFAULT as u64);
    }
    // SAFETY: host fixture callers retain the output allocation across ioctl.
    unsafe { pointer.write_unaligned(*value); }
    Ok(())
}
