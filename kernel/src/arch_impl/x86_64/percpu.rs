//! x86_64 per-CPU data access.
//!
//! Implements the PerCpuOps trait using GS-segment relative addressing.
//! On x86_64, the GS segment base points to the PerCpuData structure,
//! allowing fast access to per-CPU data without locks.
//!
//! This module provides all x86_64-specific per-CPU operations. The kernel's
//! per_cpu.rs module delegates to these functions for architecture-specific
//! operations.
//!
//! Note: This is part of the complete HAL API. Many operations are defined
//! for API completeness (e.g., NMI context, softirq operations).

#![allow(dead_code)] // HAL module - complete API for x86_64 per-CPU operations

use crate::arch_impl::traits::PerCpuOps;
use crate::arch_impl::x86_64::constants::*;
use core::arch::asm;
use core::sync::atomic::{compiler_fence, Ordering};

/// x86_64 per-CPU operations implementation.
pub struct X86PerCpu;

impl PerCpuOps for X86PerCpu {
    #[inline(always)]
    fn cpu_id() -> u64 {
        let id: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) id,
                offset = const PERCPU_CPU_ID_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        id
    }

    #[inline(always)]
    fn current_thread_ptr() -> *mut u8 {
        let ptr: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) ptr,
                offset = const PERCPU_CURRENT_THREAD_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        ptr as *mut u8
    }

    #[inline(always)]
    unsafe fn set_current_thread_ptr(ptr: *mut u8) {
        asm!(
            "mov gs:[{offset}], {}",
            in(reg) ptr as u64,
            offset = const PERCPU_CURRENT_THREAD_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    #[inline(always)]
    fn kernel_stack_top() -> u64 {
        let top: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) top,
                offset = const PERCPU_KERNEL_STACK_TOP_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        top
    }

    #[inline(always)]
    unsafe fn set_kernel_stack_top(addr: u64) {
        asm!(
            "mov gs:[{offset}], {}",
            in(reg) addr,
            offset = const PERCPU_KERNEL_STACK_TOP_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    #[inline(always)]
    fn preempt_count() -> u32 {
        let count: u32;
        unsafe {
            asm!(
                "mov {:e}, gs:[{offset}]",
                out(reg) count,
                offset = const PERCPU_PREEMPT_COUNT_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        count
    }

    #[inline(always)]
    fn preempt_disable() {
        unsafe {
            asm!(
                "add dword ptr gs:[{offset}], 1",
                offset = const PERCPU_PREEMPT_COUNT_OFFSET,
                options(nostack, preserves_flags)
            );
        }
    }

    #[inline(always)]
    fn preempt_enable() {
        unsafe {
            asm!(
                "sub dword ptr gs:[{offset}], 1",
                offset = const PERCPU_PREEMPT_COUNT_OFFSET,
                options(nostack, preserves_flags)
            );
        }
        // Note: The full preempt_enable() in per_cpu.rs also checks
        // need_resched and potentially triggers rescheduling.
        // This is the minimal version for the trait.
    }

    #[inline(always)]
    fn in_interrupt() -> bool {
        let count = Self::preempt_count();
        // In interrupt if HARDIRQ, SOFTIRQ, or NMI bits are set
        (count & (HARDIRQ_MASK | SOFTIRQ_MASK | NMI_MASK)) != 0
    }

    #[inline(always)]
    fn in_hardirq() -> bool {
        let count = Self::preempt_count();
        (count & HARDIRQ_MASK) != 0
    }

    #[inline(always)]
    fn can_schedule() -> bool {
        // Can schedule if preempt count is 0 (no preemption disable, not in interrupt)
        Self::preempt_count() == 0
    }
}

// Additional x86-specific per-CPU helpers

impl X86PerCpu {
    /// Get the need_resched flag.
    #[inline(always)]
    pub fn need_resched() -> bool {
        let flag: u8;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg_byte) flag,
                offset = const PERCPU_NEED_RESCHED_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        flag != 0
    }

    /// Set the need_resched flag.
    #[inline(always)]
    pub unsafe fn set_need_resched(need: bool) {
        let val: u8 = if need { 1 } else { 0 };
        asm!(
            "mov gs:[{offset}], {}",
            in(reg_byte) val,
            offset = const PERCPU_NEED_RESCHED_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Get the next_cr3 value (for context switching).
    #[inline(always)]
    pub fn next_cr3() -> u64 {
        let cr3: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) cr3,
                offset = const PERCPU_NEXT_CR3_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        cr3
    }

    /// Set the next_cr3 value.
    #[inline(always)]
    pub unsafe fn set_next_cr3(cr3: u64) {
        asm!(
            "mov gs:[{offset}], {}",
            in(reg) cr3,
            offset = const PERCPU_NEXT_CR3_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Get the saved process CR3.
    #[inline(always)]
    pub fn saved_process_cr3() -> u64 {
        let cr3: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) cr3,
                offset = const PERCPU_SAVED_PROCESS_CR3_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        cr3
    }

    /// Set the saved process CR3.
    #[inline(always)]
    pub unsafe fn set_saved_process_cr3(cr3: u64) {
        asm!(
            "mov gs:[{offset}], {}",
            in(reg) cr3,
            offset = const PERCPU_SAVED_PROCESS_CR3_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Get the kernel CR3.
    #[inline(always)]
    pub fn kernel_cr3() -> u64 {
        let cr3: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) cr3,
                offset = const PERCPU_KERNEL_CR3_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        cr3
    }

    /// Enter hard IRQ context (increment HARDIRQ count).
    #[inline(always)]
    pub unsafe fn irq_enter() {
        // Compiler barrier before incrementing - ensures all prior operations complete
        // before we mark ourselves as being in interrupt context
        compiler_fence(Ordering::Acquire);
        asm!(
            "add dword ptr gs:[{offset}], {inc}",
            offset = const PERCPU_PREEMPT_COUNT_OFFSET,
            inc = const (1 << HARDIRQ_SHIFT),
            options(nostack, preserves_flags)
        );
        // Compiler barrier after incrementing - ensures the context transition is
        // visible before any interrupt handling code runs
        compiler_fence(Ordering::Release);
    }

    /// Exit hard IRQ context (decrement HARDIRQ count).
    #[inline(always)]
    pub unsafe fn irq_exit() {
        // Compiler barrier before decrementing - ensures all interrupt handling
        // operations complete before we exit interrupt context
        compiler_fence(Ordering::Acquire);
        asm!(
            "sub dword ptr gs:[{offset}], {dec}",
            offset = const PERCPU_PREEMPT_COUNT_OFFSET,
            dec = const (1 << HARDIRQ_SHIFT),
            options(nostack, preserves_flags)
        );
        // Compiler barrier after decrementing - ensures the context transition is
        // visible before we potentially reschedule or return to interrupted code
        compiler_fence(Ordering::Release);
    }

    /// Set the PREEMPT_ACTIVE flag.
    #[inline(always)]
    pub unsafe fn set_preempt_active() {
        asm!(
            "or dword ptr gs:[{offset}], {flag}",
            offset = const PERCPU_PREEMPT_COUNT_OFFSET,
            flag = const PREEMPT_ACTIVE,
            options(nostack, preserves_flags)
        );
    }

    /// Clear the PREEMPT_ACTIVE flag.
    #[inline(always)]
    pub unsafe fn clear_preempt_active() {
        asm!(
            "and dword ptr gs:[{offset}], {mask}",
            offset = const PERCPU_PREEMPT_COUNT_OFFSET,
            mask = const (!PREEMPT_ACTIVE),
            options(nostack, preserves_flags)
        );
    }

    /// Get the idle thread pointer.
    #[inline(always)]
    pub fn idle_thread_ptr() -> *mut u8 {
        let ptr: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) ptr,
                offset = const PERCPU_IDLE_THREAD_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        ptr as *mut u8
    }

    /// Set the idle thread pointer.
    #[inline(always)]
    pub unsafe fn set_idle_thread_ptr(ptr: *mut u8) {
        asm!(
            "mov gs:[{offset}], {}",
            in(reg) ptr as u64,
            offset = const PERCPU_IDLE_THREAD_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Get the TSS pointer.
    #[inline(always)]
    pub fn tss_ptr() -> *mut u8 {
        let ptr: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) ptr,
                offset = const PERCPU_TSS_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        ptr as *mut u8
    }

    /// Set the TSS pointer.
    #[inline(always)]
    pub unsafe fn set_tss_ptr(ptr: *mut u8) {
        asm!(
            "mov gs:[{offset}], {}",
            in(reg) ptr as u64,
            offset = const PERCPU_TSS_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Get the user RSP scratch value.
    #[inline(always)]
    pub fn user_rsp_scratch() -> u64 {
        let rsp: u64;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg) rsp,
                offset = const PERCPU_USER_RSP_SCRATCH_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        rsp
    }

    /// Set the user RSP scratch value.
    #[inline(always)]
    pub unsafe fn set_user_rsp_scratch(rsp: u64) {
        asm!(
            "mov gs:[{offset}], {}",
            in(reg) rsp,
            offset = const PERCPU_USER_RSP_SCRATCH_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Get the softirq pending bitmap.
    #[inline(always)]
    pub fn softirq_pending() -> u32 {
        let pending: u32;
        unsafe {
            asm!(
                "mov {:e}, gs:[{offset}]",
                out(reg) pending,
                offset = const PERCPU_SOFTIRQ_PENDING_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        pending
    }

    /// Set a softirq pending bit.
    #[inline(always)]
    pub unsafe fn raise_softirq(nr: u32) {
        debug_assert!(nr < 32, "Invalid softirq number");
        let bit = 1u32 << nr;
        asm!(
            "or dword ptr gs:[{offset}], {:e}",
            in(reg) bit,
            offset = const PERCPU_SOFTIRQ_PENDING_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Clear a softirq pending bit.
    #[inline(always)]
    pub unsafe fn clear_softirq(nr: u32) {
        debug_assert!(nr < 32, "Invalid softirq number");
        let mask = !(1u32 << nr);
        asm!(
            "and dword ptr gs:[{offset}], {:e}",
            in(reg) mask,
            offset = const PERCPU_SOFTIRQ_PENDING_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Get exception cleanup context flag.
    #[inline(always)]
    pub fn exception_cleanup_context() -> bool {
        let val: u8;
        unsafe {
            asm!(
                "mov {}, gs:[{offset}]",
                out(reg_byte) val,
                offset = const PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET,
                options(nostack, preserves_flags, readonly)
            );
        }
        val != 0
    }

    /// Set exception cleanup context flag.
    #[inline(always)]
    pub unsafe fn set_exception_cleanup_context(value: bool) {
        let val: u8 = if value { 1 } else { 0 };
        asm!(
            "mov gs:[{offset}], {}",
            in(reg_byte) val,
            offset = const PERCPU_EXCEPTION_CLEANUP_CONTEXT_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Set the kernel CR3 in per-CPU data.
    #[inline(always)]
    pub unsafe fn set_kernel_cr3(cr3: u64) {
        asm!(
            "mov gs:[{offset}], {}",
            in(reg) cr3,
            offset = const PERCPU_KERNEL_CR3_OFFSET,
            options(nostack, preserves_flags)
        );
    }

    /// Enter softirq context.
    #[inline(always)]
    pub unsafe fn softirq_enter() {
        compiler_fence(Ordering::Acquire);
        asm!(
            "add dword ptr gs:[{offset}], {inc}",
            offset = const PERCPU_PREEMPT_COUNT_OFFSET,
            inc = const (1 << SOFTIRQ_SHIFT),
            options(nostack, preserves_flags)
        );
        compiler_fence(Ordering::Release);
    }

    /// Exit softirq context.
    #[inline(always)]
    pub unsafe fn softirq_exit() {
        compiler_fence(Ordering::Acquire);
        asm!(
            "sub dword ptr gs:[{offset}], {dec}",
            offset = const PERCPU_PREEMPT_COUNT_OFFSET,
            dec = const (1 << SOFTIRQ_SHIFT),
            options(nostack, preserves_flags)
        );
        compiler_fence(Ordering::Release);
    }

    /// Enter NMI context.
    #[inline(always)]
    pub unsafe fn nmi_enter() {
        compiler_fence(Ordering::Acquire);
        asm!(
            "add dword ptr gs:[{offset}], {inc}",
            offset = const PERCPU_PREEMPT_COUNT_OFFSET,
            inc = const (1 << NMI_SHIFT),
            options(nostack, preserves_flags)
        );
        compiler_fence(Ordering::Release);
    }

    /// Exit NMI context.
    #[inline(always)]
    pub unsafe fn nmi_exit() {
        compiler_fence(Ordering::Acquire);
        asm!(
            "sub dword ptr gs:[{offset}], {dec}",
            offset = const PERCPU_PREEMPT_COUNT_OFFSET,
            dec = const (1 << NMI_SHIFT),
            options(nostack, preserves_flags)
        );
        compiler_fence(Ordering::Release);
    }

    /// Check if in softirq context.
    #[inline(always)]
    pub fn in_softirq() -> bool {
        let count = Self::preempt_count();
        (count & SOFTIRQ_MASK) != 0
    }

    /// Check if in NMI context.
    #[inline(always)]
    pub fn in_nmi() -> bool {
        let count = Self::preempt_count();
        (count & NMI_MASK) != 0
    }
}

// GS base register operations (x86_64-specific)

/// Read the current GS_BASE MSR value.
#[inline(always)]
pub fn read_gs_base() -> u64 {
    let base: u64;
    unsafe {
        asm!(
            "rdgsbase {}",
            out(reg) base,
            options(nostack, preserves_flags)
        );
    }
    base
}

/// Write a new value to the GS_BASE MSR.
///
/// # Safety
/// The caller must ensure the address points to valid per-CPU data.
#[inline(always)]
pub unsafe fn write_gs_base(addr: u64) {
    asm!(
        "wrgsbase {}",
        in(reg) addr,
        options(nostack, preserves_flags)
    );
}

/// Read the KERNEL_GS_BASE MSR (for SWAPGS).
#[inline(always)]
pub fn read_kernel_gs_base() -> u64 {
    // KERNEL_GS_BASE MSR = 0xC0000102
    const KERNEL_GS_BASE_MSR: u32 = 0xC0000102;
    let low: u32;
    let high: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") KERNEL_GS_BASE_MSR,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

/// Write to the KERNEL_GS_BASE MSR (for SWAPGS).
///
/// # Safety
/// The caller must ensure the address points to valid per-CPU data.
#[inline(always)]
pub unsafe fn write_kernel_gs_base(addr: u64) {
    // KERNEL_GS_BASE MSR = 0xC0000102
    const KERNEL_GS_BASE_MSR: u32 = 0xC0000102;
    let low = (addr & 0xFFFF_FFFF) as u32;
    let high = (addr >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") KERNEL_GS_BASE_MSR,
        in("eax") low,
        in("edx") high,
        options(nostack, preserves_flags)
    );
}

/// Alternative: use x86_64 crate for GS base (fallback if RDGSBASE/WRGSBASE not available).
/// This module is compiled only when the feature for CPU instructions is available.
pub mod msr {
    use super::*;

    /// Read GS_BASE via MSR.
    #[inline(always)]
    pub fn read_gs_base_msr() -> u64 {
        const GS_BASE_MSR: u32 = 0xC0000101;
        let low: u32;
        let high: u32;
        unsafe {
            asm!(
                "rdmsr",
                in("ecx") GS_BASE_MSR,
                out("eax") low,
                out("edx") high,
                options(nostack, preserves_flags)
            );
        }
        ((high as u64) << 32) | (low as u64)
    }

    /// Write GS_BASE via MSR.
    #[inline(always)]
    pub unsafe fn write_gs_base_msr(addr: u64) {
        const GS_BASE_MSR: u32 = 0xC0000101;
        let low = (addr & 0xFFFF_FFFF) as u32;
        let high = (addr >> 32) as u32;
        asm!(
            "wrmsr",
            in("ecx") GS_BASE_MSR,
            in("eax") low,
            in("edx") high,
            options(nostack, preserves_flags)
        );
    }
}

// Implement the trait for the type alias
impl X86PerCpu {
    /// Get preempt count from the trait.
    #[inline(always)]
    pub fn preempt_count() -> u32 {
        <Self as PerCpuOps>::preempt_count()
    }
}
