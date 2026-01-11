//! x86_64 CPU operations.
//!
//! Implements basic CPU control operations like interrupt management and halt.

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
}
