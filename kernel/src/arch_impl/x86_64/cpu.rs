//! x86_64 CPU operations.
//!
//! Implements basic CPU control operations like interrupt management and halt.
//!
//! Note: This is part of the complete HAL API. The X86Cpu struct
//! implements the CpuOps trait.

#![allow(dead_code)] // HAL type - part of complete API

use crate::arch_impl::traits::CpuOps;

/// x86_64 CPU operations implementation.
pub struct X86Cpu;

impl CpuOps for X86Cpu {
    #[inline(always)]
    unsafe fn enable_interrupts() {
        x86_64::instructions::interrupts::enable();
    }

    #[inline(always)]
    unsafe fn disable_interrupts() {
        x86_64::instructions::interrupts::disable();
    }

    #[inline(always)]
    fn interrupts_enabled() -> bool {
        x86_64::instructions::interrupts::are_enabled()
    }

    #[inline(always)]
    fn halt() {
        x86_64::instructions::hlt();
    }

    #[inline(always)]
    fn halt_with_interrupts() {
        x86_64::instructions::interrupts::enable_and_hlt();
    }

    #[inline(always)]
    fn without_interrupts<F, R>(f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // Delegate to the x86_64 crate's implementation
        x86_64::instructions::interrupts::without_interrupts(f)
    }
}
