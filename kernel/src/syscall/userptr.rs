//! Userspace pointer validation and safe memory operations
//!
//! This module provides safe functions for reading and writing userspace memory
//! from kernel context, with proper validation to prevent:
//! - Reading/writing kernel memory via malicious userspace pointers
//! - Dereferencing unmapped addresses
//! - Integer overflow attacks in pointer arithmetic
//!
//! This module's copies, and the byte-buffer copies in `handlers.rs`, go
//! through `read_user_bytes` and `write_user_bytes`. They copy through a small
//! assembly routine whose user-access instructions the page-fault (x86_64) and
//! data-abort (aarch64) handlers recognise through `uaccess_fixup`: a fault
//! there that the handler cannot resolve as it would for a user-mode access
//! (a copy-on-write write, or x86_64 user-stack growth) resumes at the
//! routine's fault exit, so a bad pointer becomes EFAULT instead of a kernel
//! fault. On aarch64 the routine uses the unprivileged `ldtr`/`sttr` forms, so
//! the page must also be readable or writable from EL0; on x86_64 CR0.WP makes
//! a write to a read-only page fault.

use super::SyscallResult;

// x86_64: one routine serves both directions. `rep movsb` is restartable, so a
// fault leaves RIP on it with RCX holding the bytes still to copy.
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    r#"
    .pushsection .text.breenix_uaccess, "ax"
    .global breenix_uaccess_copy
    .global breenix_uaccess_insn
    .global breenix_uaccess_fixup
// breenix_uaccess_copy(dst: RDI, src: RSI, len: RDX) -> RAX (0 = copied, 1 = faulted)
breenix_uaccess_copy:
    mov rcx, rdx
breenix_uaccess_insn:
    rep movsb
    xor eax, eax
    ret
breenix_uaccess_fixup:
    mov eax, 1
    ret
    .popsection
"#
);

// aarch64: the user side of each copy is a single unprivileged access, so only
// those instructions are recognised; the kernel-side access is not. When both
// pointers are 8-byte aligned the copy moves words, then the tail in bytes.
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    r#"
    .pushsection .text.breenix_uaccess, "ax"
    .global breenix_uaccess_read
    .global breenix_uaccess_write
    .global breenix_uaccess_load8
    .global breenix_uaccess_load
    .global breenix_uaccess_store8
    .global breenix_uaccess_store
    .global breenix_uaccess_fixup
// breenix_uaccess_read(dst: x0 kernel, src: x1 user, len: x2) -> x0 (0 = copied, 1 = faulted)
breenix_uaccess_read:
    orr x4, x0, x1
    tst x4, #7
    b.ne 2f
1:
    cmp x2, #8
    b.lo 2f
breenix_uaccess_load8:
    ldtr x3, [x1]
    str x3, [x0], #8
    add x1, x1, #8
    sub x2, x2, #8
    b 1b
2:
    cbz x2, 4f
3:
breenix_uaccess_load:
    ldtrb w3, [x1]
    strb w3, [x0], #1
    add x1, x1, #1
    subs x2, x2, #1
    b.ne 3b
4:
    mov x0, #0
    ret
// breenix_uaccess_write(dst: x0 user, src: x1 kernel, len: x2) -> x0 (0 = copied, 1 = faulted)
breenix_uaccess_write:
    orr x4, x0, x1
    tst x4, #7
    b.ne 6f
5:
    cmp x2, #8
    b.lo 6f
    ldr x3, [x1], #8
breenix_uaccess_store8:
    sttr x3, [x0]
    add x0, x0, #8
    sub x2, x2, #8
    b 5b
6:
    cbz x2, 8f
7:
    ldrb w3, [x1], #1
breenix_uaccess_store:
    sttrb w3, [x0]
    add x0, x0, #1
    subs x2, x2, #1
    b.ne 7b
8:
    mov x0, #0
    ret
breenix_uaccess_fixup:
    mov x0, #1
    ret
    .popsection
"#
);

extern "C" {
    #[cfg(target_arch = "x86_64")]
    fn breenix_uaccess_copy(dst: *mut u8, src: *const u8, len: usize) -> usize;
    #[cfg(target_arch = "x86_64")]
    static breenix_uaccess_insn: u8;
    #[cfg(target_arch = "aarch64")]
    fn breenix_uaccess_read(dst: *mut u8, src: *const u8, len: usize) -> usize;
    #[cfg(target_arch = "aarch64")]
    fn breenix_uaccess_write(dst: *mut u8, src: *const u8, len: usize) -> usize;
    #[cfg(target_arch = "aarch64")]
    static breenix_uaccess_load8: u8;
    #[cfg(target_arch = "aarch64")]
    static breenix_uaccess_load: u8;
    #[cfg(target_arch = "aarch64")]
    static breenix_uaccess_store8: u8;
    #[cfg(target_arch = "aarch64")]
    static breenix_uaccess_store: u8;
    static breenix_uaccess_fixup: u8;
}

/// Where a kernel-mode fault at `pc` on address `addr` resumes, if `pc` is one
/// of the user-copy routine's user-access instructions and `addr` is a user
/// address. Called from the fault handlers: no locks, no allocation, no output.
pub fn uaccess_fixup(pc: u64, addr: u64) -> Option<u64> {
    if addr >= crate::memory::layout::USER_STACK_REGION_END {
        return None;
    }
    // Only the addresses of these assembly labels are taken.
    #[cfg(target_arch = "x86_64")]
    let hit = pc == core::ptr::addr_of!(breenix_uaccess_insn) as u64;
    #[cfg(target_arch = "aarch64")]
    let hit = [
        core::ptr::addr_of!(breenix_uaccess_load8),
        core::ptr::addr_of!(breenix_uaccess_load),
        core::ptr::addr_of!(breenix_uaccess_store8),
        core::ptr::addr_of!(breenix_uaccess_store),
    ]
    .iter()
    .any(|&insn| pc == insn as u64);
    hit.then(|| core::ptr::addr_of!(breenix_uaccess_fixup) as u64)
}

/// Copy `len` bytes from user address `src` into the kernel buffer `dst`. The
/// user pointer needs no particular alignment.
///
/// `Err(14)` (EFAULT) if the range is not a legitimate user range, or any byte
/// of it is unmapped or not readable by the process.
pub fn read_user_bytes(dst: *mut u8, src: u64, len: usize) -> Result<(), u64> {
    if len == 0 {
        return Ok(());
    }
    if src == 0 || !crate::memory::layout::is_valid_user_range(src, len) {
        return Err(14); // EFAULT
    }
    // SAFETY: `dst` is a kernel buffer of `len` bytes owned by the caller; the
    // user side faults into the routine's fault exit rather than the kernel.
    #[cfg(target_arch = "x86_64")]
    let faulted = unsafe { breenix_uaccess_copy(dst, src as *const u8, len) };
    #[cfg(target_arch = "aarch64")]
    let faulted = unsafe { breenix_uaccess_read(dst, src as *const u8, len) };
    if faulted == 0 {
        Ok(())
    } else {
        Err(14) // EFAULT
    }
}

/// Copy `len` bytes from the kernel buffer `src` to user address `dst`.
///
/// `Err(14)` (EFAULT) if the range is not a legitimate user range, or any byte
/// of it is unmapped or not writable by the process. A copy-on-write page is
/// copied by the fault handler and the write goes ahead. A fault part way
/// through leaves the bytes before it written.
pub fn write_user_bytes(dst: u64, src: *const u8, len: usize) -> Result<(), u64> {
    if len == 0 {
        return Ok(());
    }
    if dst == 0 || !crate::memory::layout::is_valid_user_range(dst, len) {
        return Err(14); // EFAULT
    }
    // SAFETY: `src` is a kernel buffer of `len` bytes owned by the caller; the
    // user side faults into the routine's fault exit rather than the kernel.
    #[cfg(target_arch = "x86_64")]
    let faulted = unsafe { breenix_uaccess_copy(dst as *mut u8, src, len) };
    #[cfg(target_arch = "aarch64")]
    let faulted = unsafe { breenix_uaccess_write(dst as *mut u8, src, len) };
    if faulted == 0 {
        Ok(())
    } else {
        Err(14) // EFAULT
    }
}

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
    let size = core::mem::size_of::<T>() as usize;

    // Check for null pointer
    if ptr.is_null() {
        return Err(14); // EFAULT
    }

    // Validate against the closed allow-list of legitimate userspace regions
    // (code/data, mmap, stack), not merely "somewhere below USER_SPACE_END".
    // On x86_64, USER_SPACE_END alone is not a safe bound: it also covers
    // the kernel's own mapped PIE image and heap, which
    // ProcessPageTable::new copies (without USER_ACCESSIBLE) into every
    // process's page table, so a kernel-mode read/write of an address there
    // still succeeds even though userspace itself could never fault it in.
    // See memory::layout::is_valid_user_range's doc comment (#729 review
    // finding B4, and the same root cause found here while fixing it).
    if !crate::memory::layout::is_valid_user_range(addr, size) {
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

    // Byte-wise, so the user pointer needs no alignment, and through the
    // fault-tolerant routine, so an unmapped or unreadable page is EFAULT.
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    read_user_bytes(
        value.as_mut_ptr() as *mut u8,
        ptr as u64,
        core::mem::size_of::<T>(),
    )?;
    // SAFETY: every byte of `value` was just written. As before, callers only
    // use this for plain-data types for which any byte pattern is valid.
    Ok(unsafe { value.assume_init() })
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

    // Byte-wise, so the user pointer needs no alignment, and through the
    // fault-tolerant routine, so an unmapped or read-only page is EFAULT.
    write_user_bytes(
        ptr as u64,
        value as *const T as *const u8,
        core::mem::size_of::<T>(),
    )
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

/// Maximum path length for copy_cstr_from_user
const MAX_PATH_LEN: usize = 4096;

/// Validate a user buffer with explicit length
///
/// # Arguments
/// * `ptr` - Start of the buffer
/// * `len` - Length of the buffer in bytes
///
/// # Returns
/// * `Ok(())` if the buffer is valid
/// * `Err(14)` (EFAULT) if the buffer is invalid
#[allow(dead_code)] // Part of userspace pointer validation API
pub fn validate_user_buffer(ptr: *const u8, len: usize) -> Result<(), u64> {
    let addr = ptr as u64;

    // Check for null pointer
    if ptr.is_null() {
        return Err(14); // EFAULT
    }

    // Validate against the closed allow-list of legitimate userspace regions
    // -- see validate_user_ptr_read's comment above for why the old
    // USER_SPACE_END-only bound was unsafe on x86_64 (#729 review finding
    // B4).
    if !crate::memory::layout::is_valid_user_range(addr, len) {
        return Err(14); // EFAULT
    }

    Ok(())
}

/// Copy a C-style null-terminated string from userspace
///
/// Reads bytes from userspace until a null terminator is found or
/// MAX_PATH_LEN is reached. Returns the string as a Rust String.
///
/// # Arguments
/// * `ptr` - Pointer to the start of the string in userspace
///
/// # Returns
/// * `Ok(String)` containing the copied string (without null terminator)
/// * `Err(14)` (EFAULT) if the pointer is invalid
/// * `Err(36)` (ENAMETOOLONG) if the string exceeds MAX_PATH_LEN
///
/// # Safety
/// This function validates each byte's address before reading.
pub fn copy_cstr_from_user(ptr: u64) -> Result<alloc::string::String, u64> {
    use alloc::string::String;
    use alloc::vec::Vec;

    // Validate that the starting address is in userspace
    if ptr == 0 {
        return Err(14); // EFAULT - null pointer
    }
    if !crate::memory::layout::is_valid_user_range(ptr, 1) {
        return Err(14); // EFAULT - kernel address
    }

    let mut bytes = Vec::with_capacity(256);

    for offset in 0..MAX_PATH_LEN {
        let byte_addr = match ptr.checked_add(offset as u64) {
            // Per-byte validation against the closed allow-list -- see
            // validate_user_ptr_read's comment above (#729 review finding
            // B4). The string's length isn't known up front (we stop at the
            // first NUL below), so this stays a per-byte check rather than
            // a single up-front range check.
            Some(addr) if crate::memory::layout::is_valid_user_range(addr, 1) => addr,
            _ => return Err(14), // EFAULT - overflow or kernel address
        };

        let mut byte = 0u8;
        read_user_bytes(&mut byte, byte_addr, 1)?;

        if byte == 0 {
            // Found null terminator
            return String::from_utf8(bytes).map_err(|_| 22); // EINVAL for invalid UTF-8
        }

        bytes.push(byte);
    }

    // String too long
    Err(36) // ENAMETOOLONG
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
        // Valid userspace address -- inside the code/data region
        // (USERSPACE_BASE..USERSPACE_CODE_DATA_END), which is where a
        // process's own code/data/brk-heap lives.
        let ptr: *const u64 = crate::memory::layout::USERSPACE_BASE as *const u64;
        assert!(validate_user_ptr_read(ptr).is_ok());
    }

    #[test]
    fn test_kernel_pie_candidate_rejected() {
        // #729 review finding B4: both observed x86_64 kernel PIE bases
        // (see gdb_chat.py's KERNEL_BASE_X86 comment) must be refused, not
        // merely "somewhere below the old USER_SPACE_END bound".
        assert!(validate_user_ptr_read(0x0000_0080_0000_0000u64 as *const u64).is_err());
        assert!(validate_user_ptr_read(0x0000_0100_0000_0000u64 as *const u64).is_err());
    }

    #[test]
    fn test_kernel_heap_rejected() {
        assert!(
            validate_user_ptr_read(crate::memory::heap::HEAP_START as *const u64).is_err()
        );
    }

    #[test]
    fn test_boundary_case() {
        // Address right at the edge of the stack region (should fail - a
        // u64 read here would cross out of the allow-listed range).
        let ptr: *const u64 = (crate::memory::layout::USER_STACK_REGION_END - 4) as *const u64;
        assert!(validate_user_ptr_read(ptr).is_err());
    }
}
