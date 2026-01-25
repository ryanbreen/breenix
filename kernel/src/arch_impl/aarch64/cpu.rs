//! ARM64 CPU operations.
#![allow(dead_code)]

use crate::arch_impl::traits::CpuOps;

pub struct Aarch64Cpu;

impl CpuOps for Aarch64Cpu {
    unsafe fn enable_interrupts() {
        unimplemented!("ARM64: enable_interrupts not yet implemented")
    }

    unsafe fn disable_interrupts() {
        unimplemented!("ARM64: disable_interrupts not yet implemented")
    }

    fn interrupts_enabled() -> bool {
        unimplemented!("ARM64: interrupts_enabled not yet implemented")
    }

    fn halt() {
        unimplemented!("ARM64: halt not yet implemented")
    }

    fn halt_with_interrupts() {
        unimplemented!("ARM64: halt_with_interrupts not yet implemented")
    }
}
