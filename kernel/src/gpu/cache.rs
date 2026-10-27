//! Architecture-specific GPU cache and CPU publication operations.
/// The data cache line size from CTR_EL0 (Arm Architecture Reference Manual, D19.2.34).
#[cfg(target_arch = "aarch64")]
pub fn dcache_line() -> u64 {
    let ctr: u64;
    // SAFETY: CTR_EL0 is readable at EL1.
    unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    4 << ((ctr >> 16) & 0xF)
}

/// Clean `[va, va + len)` to the point of coherency, so the device reads what the CPU wrote.
pub fn clean(va: u64, len: usize) {
    maintain(va, len, false);
}

/// Clean and invalidate `[va, va + len)`, so the CPU reads what the device wrote.
pub fn invalidate(va: u64, len: usize) {
    maintain(va, len, true);
}

#[cfg(target_arch = "aarch64")]
fn maintain(va: u64, len: usize, invalidate: bool) {
    if len == 0 {
        return;
    }
    let line = dcache_line();
    let mut at = va & !(line - 1);
    let end = va + len as u64;
    while at < end {
        // SAFETY: the range is mapped kernel memory owned by the caller.
        unsafe {
            if invalidate {
                core::arch::asm!("dc civac, {}", in(reg) at, options(nostack));
            } else {
                core::arch::asm!("dc cvac, {}", in(reg) at, options(nostack));
            }
        }
        at += line;
    }
    // SAFETY: barrier only.
    unsafe { core::arch::asm!("dsb sy", options(nostack)) };
}

/// Coherent x86 system RAM needs ordering, not ARM cache-line instructions.
#[cfg(target_arch = "x86_64")]
pub fn dcache_line() -> u64 {
    64
}
#[cfg(target_arch = "x86_64")]
fn maintain(_va: u64, len: usize, invalidate: bool) {
    if len == 0 {
        return;
    }
    // SAFETY: barriers only; the compiler memory clobber also orders accesses.
    unsafe {
        if invalidate {
            core::arch::asm!("mfence", options(nostack, preserves_flags));
        } else {
            core::arch::asm!("sfence", options(nostack, preserves_flags));
        }
    }
}
/// Drain WC writes before MMIO publication, window rebinding or a doorbell.
pub fn drain_wc() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("sfence", options(nostack, preserves_flags));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
}
