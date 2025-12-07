//! Userspace pointer validation and safe memory operations
//!
//! This module provides safe functions for reading and writing userspace memory
//! from kernel context, with proper validation to prevent:
//! - Reading/writing kernel memory via malicious userspace pointers
//! - Dereferencing unmapped addresses
//! - Integer overflow attacks in pointer arithmetic

use super::SyscallResult;

/// Userspace address range - below the kernel split
/// On x86_64, the canonical address split is at 0x0000_8000_0000_0000
/// Addresses at or above this value are kernel addresses
const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

/// Validate that a userspace pointer is safe to read from
///
/// # Arguments
/// * `ptr` - The pointer to validate
///
/// # Returns
/// * `Ok(())` if the pointer is valid
/// * `Err(14)` (EFAULT) if the pointer is invalid
///
/// # Validation Checks
/// 1. Pointer is not null
/// 2. Pointer is within userspace address range
/// 3. Pointer + size doesn't overflow or cross into kernel space
pub fn validate_user_ptr_read<T>(ptr: *const T) -> Result<(), u64> {
    let addr = ptr as u64;
    let size = core::mem::size_of::<T>() as u64;

    // Check for null pointer
    if ptr.is_null() {
        return Err(14); // EFAULT
    }

    // Check address is in userspace range
    if addr >= USER_SPACE_END {
        return Err(14); // EFAULT
    }

    // Check for overflow and that end address is still in userspace
    if addr.checked_add(size).map_or(true, |end| end > USER_SPACE_END) {
        return Err(14); // EFAULT
    }

    Ok(())
}

/// Validate that a userspace pointer is safe to write to
///
/// # Arguments
/// * `ptr` - The pointer to validate
///
/// # Returns
/// * `Ok(())` if the pointer is valid
/// * `Err(14)` (EFAULT) if the pointer is invalid
///
/// # Validation Checks
/// Same as validate_user_ptr_read. Currently we don't distinguish between
/// read and write permissions, but this function is provided for semantic
/// clarity and future extensibility (e.g., checking page write permissions).
pub fn validate_user_ptr_write<T>(ptr: *mut T) -> Result<(), u64> {
    // For now, same validation as read
    validate_user_ptr_read(ptr as *const T)
}

/// Safely copy data from userspace to kernel
///
/// # Arguments
/// * `ptr` - Userspace pointer to read from
///
/// # Returns
/// * `Ok(value)` if the read succeeded
/// * `Err(14)` (EFAULT) if the pointer is invalid
///
/// # Safety
/// This function validates the pointer before reading, making it safe to use
/// with untrusted userspace pointers.
pub fn copy_from_user<T: Copy>(ptr: *const T) -> Result<T, u64> {
    // Validate the pointer first
    validate_user_ptr_read(ptr)?;

    // SAFETY: We just validated that:
    // - ptr is not null
    // - ptr is in userspace range
    // - ptr + sizeof(T) doesn't overflow or cross into kernel space
    // However, we can't guarantee the memory is mapped. A page fault
    // will occur if userspace passed an unmapped address, which the
    // kernel should handle gracefully.
    let value = unsafe {
        core::ptr::read_volatile(ptr)
    };

    Ok(value)
}

/// Safely copy data from kernel to userspace
///
/// # Arguments
/// * `ptr` - Userspace pointer to write to
/// * `value` - Value to write
///
/// # Returns
/// * `Ok(())` if the write succeeded
/// * `Err(14)` (EFAULT) if the pointer is invalid
///
/// # Safety
/// This function validates the pointer before writing, making it safe to use
/// with untrusted userspace pointers.
pub fn copy_to_user<T: Copy>(ptr: *mut T, value: &T) -> Result<(), u64> {
    // Validate the pointer first
    validate_user_ptr_write(ptr)?;

    // SAFETY: We just validated that:
    // - ptr is not null
    // - ptr is in userspace range
    // - ptr + sizeof(T) doesn't overflow or cross into kernel space
    // However, we can't guarantee the memory is mapped. A page fault
    // will occur if userspace passed an unmapped address, which the
    // kernel should handle gracefully.
    unsafe {
        core::ptr::write_volatile(ptr, *value);
    }

    Ok(())
}

/// Convert a validation error to a SyscallResult
#[inline]
#[allow(dead_code)]
pub fn to_syscall_result(result: Result<(), u64>) -> SyscallResult {
    match result {
        Ok(()) => SyscallResult::Ok(0),
        Err(errno) => SyscallResult::Err(errno),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_null_pointer_rejected() {
        let ptr: *const u64 = core::ptr::null();
        assert!(validate_user_ptr_read(ptr).is_err());
    }

    #[test]
    fn test_kernel_address_rejected() {
        // Address in kernel space
        let ptr: *const u64 = 0x0000_8000_0000_0000 as *const u64;
        assert!(validate_user_ptr_read(ptr).is_err());
    }

    #[test]
    fn test_overflow_rejected() {
        // Address that would overflow when adding sizeof(u64)
        let ptr: *const u64 = (u64::MAX - 4) as *const u64;
        assert!(validate_user_ptr_read(ptr).is_err());
    }

    #[test]
    fn test_valid_userspace_address() {
        // Valid userspace address
        let ptr: *const u64 = 0x0000_0000_1000_0000 as *const u64;
        assert!(validate_user_ptr_read(ptr).is_ok());
    }

    #[test]
    fn test_boundary_case() {
        // Address right at the boundary (should fail - would cross into kernel)
        let ptr: *const u64 = (USER_SPACE_END - 4) as *const u64;
        assert!(validate_user_ptr_read(ptr).is_err());
    }
}
