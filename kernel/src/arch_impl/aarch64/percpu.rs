//! ARM64 per-CPU data access using TPIDR_EL1.
//!
//! On ARM64, TPIDR_EL1 holds the base pointer to the per-CPU data structure.
//! This is similar to x86's GS segment base. Each CPU core sets TPIDR_EL1 to
//! point to its own PerCpuData structure during initialization.
//!
//! Unlike x86 where we can access fields directly via GS:offset, on ARM64 we
//! read TPIDR_EL1 to get the base address and then add offsets manually.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU32, Ordering};
use crate::arch_impl::traits::PerCpuOps;
use crate::arch_impl::aarch64::constants::{
    PERCPU_CPU_ID_OFFSET,
    PERCPU_CURRENT_THREAD_OFFSET,
    PERCPU_KERNEL_STACK_TOP_OFFSET,
    PERCPU_PREEMPT_COUNT_OFFSET,
    HARDIRQ_MASK,
    SOFTIRQ_MASK,
};

pub struct Aarch64PerCpu;

/// Read TPIDR_EL1 (per-CPU data base pointer)
#[inline(always)]
fn read_tpidr_el1() -> u64 {
    let val: u64;
    unsafe {
        core::arch::asm!("mrs {}, tpidr_el1", out(reg) val, options(nomem, nostack, preserves_flags));
    }
    val
}

/// Write TPIDR_EL1 (per-CPU data base pointer)
#[inline(always)]
unsafe fn write_tpidr_el1(val: u64) {
    core::arch::asm!("msr tpidr_el1, {}", in(reg) val, options(nomem, nostack, preserves_flags));
}

/// Read a u64 from per-CPU data at the given offset
#[inline(always)]
fn percpu_read_u64(offset: usize) -> u64 {
    let base = read_tpidr_el1();
    if base == 0 {
        // Per-CPU not yet initialized
        return 0;
    }
    unsafe {
        core::ptr::read_volatile((base as *const u8).add(offset) as *const u64)
    }
}

/// Write a u64 to per-CPU data at the given offset
#[inline(always)]
unsafe fn percpu_write_u64(offset: usize, val: u64) {
    let base = read_tpidr_el1();
    if base == 0 {
        return; // Per-CPU not yet initialized
    }
    core::ptr::write_volatile((base as *mut u8).add(offset) as *mut u64, val);
}

/// Read a u32 from per-CPU data at the given offset
#[inline(always)]
fn percpu_read_u32(offset: usize) -> u32 {
    let base = read_tpidr_el1();
    if base == 0 {
        return 0;
    }
    unsafe {
        core::ptr::read_volatile((base as *const u8).add(offset) as *const u32)
    }
}

/// Get atomic reference to a u32 field in per-CPU data
#[inline(always)]
fn percpu_atomic_u32(offset: usize) -> Option<&'static AtomicU32> {
    let base = read_tpidr_el1();
    if base == 0 {
        return None;
    }
    unsafe {
        Some(&*((base as *const u8).add(offset) as *const AtomicU32))
    }
}

impl PerCpuOps for Aarch64PerCpu {
    /// Get the current CPU ID
    ///
    /// Reads from the per-CPU data structure. If not initialized,
    /// falls back to reading MPIDR_EL1 Aff0 field.
    #[inline]
    fn cpu_id() -> u64 {
        let base = read_tpidr_el1();
        if base == 0 {
            // Per-CPU not yet initialized, read from MPIDR_EL1
            let mpidr: u64;
            unsafe {
                core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
            }
            return mpidr & 0xFF;
        }
        percpu_read_u64(PERCPU_CPU_ID_OFFSET)
    }

    /// Get the current thread pointer
    #[inline]
    fn current_thread_ptr() -> *mut u8 {
        percpu_read_u64(PERCPU_CURRENT_THREAD_OFFSET) as *mut u8
    }

    /// Set the current thread pointer
    #[inline]
    unsafe fn set_current_thread_ptr(ptr: *mut u8) {
        percpu_write_u64(PERCPU_CURRENT_THREAD_OFFSET, ptr as u64);
    }

    /// Get the kernel stack top for this CPU
    #[inline]
    fn kernel_stack_top() -> u64 {
        percpu_read_u64(PERCPU_KERNEL_STACK_TOP_OFFSET)
    }

    /// Set the kernel stack top for this CPU
    #[inline]
    unsafe fn set_kernel_stack_top(addr: u64) {
        percpu_write_u64(PERCPU_KERNEL_STACK_TOP_OFFSET, addr);
    }

    /// Get the preempt count (atomically)
    #[inline]
    fn preempt_count() -> u32 {
        match percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            Some(atomic) => atomic.load(Ordering::Relaxed),
            None => 0,
        }
    }

    /// Disable preemption by incrementing preempt count
    #[inline]
    fn preempt_disable() {
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Enable preemption by decrementing preempt count
    #[inline]
    fn preempt_enable() {
        if let Some(atomic) = percpu_atomic_u32(PERCPU_PREEMPT_COUNT_OFFSET) {
            atomic.fetch_sub(1, Ordering::Release);
        }
    }

    /// Check if we're in any interrupt context (hardirq or softirq)
    #[inline]
    fn in_interrupt() -> bool {
        let count = Self::preempt_count();
        (count & (HARDIRQ_MASK | SOFTIRQ_MASK)) != 0
    }

    /// Check if we're in hardirq context
    #[inline]
    fn in_hardirq() -> bool {
        let count = Self::preempt_count();
        (count & HARDIRQ_MASK) != 0
    }

    /// Check if scheduling is allowed
    ///
    /// Returns true if preempt_count is 0 (no preemption disabled,
    /// not in interrupt context).
    #[inline]
    fn can_schedule() -> bool {
        Self::preempt_count() == 0
    }
}

// =============================================================================
// Per-CPU initialization and setup
// =============================================================================

/// Initialize per-CPU data for the current CPU
///
/// This should be called early in boot for each CPU core.
/// The base pointer should point to a PerCpuData structure.
#[inline]
pub unsafe fn init_percpu(base: u64, cpu_id: u64) {
    // Set TPIDR_EL1 to point to our per-CPU data
    write_tpidr_el1(base);

    // Initialize the CPU ID field
    core::ptr::write_volatile((base as *mut u8).add(PERCPU_CPU_ID_OFFSET) as *mut u64, cpu_id);

    // Initialize preempt_count to 0
    core::ptr::write_volatile((base as *mut u8).add(PERCPU_PREEMPT_COUNT_OFFSET) as *mut u32, 0);
}

/// Get the raw per-CPU base pointer
#[inline]
pub fn percpu_base() -> u64 {
    read_tpidr_el1()
}

/// Check if per-CPU is initialized for this CPU
#[inline]
pub fn percpu_initialized() -> bool {
    read_tpidr_el1() != 0
}
