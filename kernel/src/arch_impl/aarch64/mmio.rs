//! ARM64 device-register accesses with a fixed, ESR.ISV-compatible encoding.
//!
//! LLVM may select address writeback for Rust volatile accesses. QEMU HVF
//! cannot emulate those ISV=0 data aborts. These helpers guarantee a single
//! register-indirect access; callers still provide device ordering barriers.

/// Read a 8-bit device register.
///
/// # Safety
/// `address` must be a valid, aligned device-memory pointer for this access.
#[inline]
pub unsafe fn read8(address: *const u8) -> u8 {
    let value: u32;
    core::arch::asm!("ldrb {value:w}, [{address}]", value = out(reg) value,
                     address = in(reg) address, options(nostack, preserves_flags));
    value as u8
}

/// Read a 32-bit device register.
///
/// # Safety
/// `address` must be a valid, aligned device-memory pointer for this access.
#[inline]
pub unsafe fn read32(address: *const u32) -> u32 {
    let value: u32;
    core::arch::asm!("ldr {value:w}, [{address}]", value = out(reg) value,
                     address = in(reg) address, options(nostack, preserves_flags));
    value
}

/// Read a 64-bit device register.
///
/// # Safety
/// `address` must be a valid, aligned device-memory pointer for this access.
#[inline]
pub unsafe fn read64(address: *const u64) -> u64 {
    let value: u64;
    core::arch::asm!("ldr {value}, [{address}]", value = out(reg) value,
                     address = in(reg) address, options(nostack, preserves_flags));
    value
}

/// Write a 8-bit device register.
///
/// # Safety
/// `address` must be a valid, aligned device-memory pointer for this access.
#[inline]
pub unsafe fn write8(address: *mut u8, value: u8) {
    core::arch::asm!("strb {value:w}, [{address}]", value = in(reg) value as u32,
                     address = in(reg) address, options(nostack, preserves_flags));
}

/// Write a 16-bit device register.
///
/// # Safety
/// `address` must be a valid, aligned device-memory pointer for this access.
#[inline]
pub unsafe fn write16(address: *mut u16, value: u16) {
    core::arch::asm!("strh {value:w}, [{address}]", value = in(reg) value as u32,
                     address = in(reg) address, options(nostack, preserves_flags));
}

/// Write a 32-bit device register.
///
/// # Safety
/// `address` must be a valid, aligned device-memory pointer for this access.
#[inline]
pub unsafe fn write32(address: *mut u32, value: u32) {
    core::arch::asm!("str {value:w}, [{address}]", value = in(reg) value,
                     address = in(reg) address, options(nostack, preserves_flags));
}

