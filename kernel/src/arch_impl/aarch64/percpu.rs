//! ARM64 per-CPU data access using TPIDR_EL1.

#![allow(dead_code)]

use crate::arch_impl::traits::PerCpuOps;

pub struct Aarch64PerCpu;

impl PerCpuOps for Aarch64PerCpu {
    fn cpu_id() -> u64 {
        unimplemented!("ARM64: cpu_id not yet implemented")
    }

    fn current_thread_ptr() -> *mut u8 {
        unimplemented!("ARM64: current_thread_ptr not yet implemented")
    }

    unsafe fn set_current_thread_ptr(ptr: *mut u8) {
        let _ = ptr;
        unimplemented!("ARM64: set_current_thread_ptr not yet implemented")
    }

    fn kernel_stack_top() -> u64 {
        unimplemented!("ARM64: kernel_stack_top not yet implemented")
    }

    unsafe fn set_kernel_stack_top(addr: u64) {
        let _ = addr;
        unimplemented!("ARM64: set_kernel_stack_top not yet implemented")
    }

    fn preempt_count() -> u32 {
        unimplemented!("ARM64: preempt_count not yet implemented")
    }

    fn preempt_disable() {
        unimplemented!("ARM64: preempt_disable not yet implemented")
    }

    fn preempt_enable() {
        unimplemented!("ARM64: preempt_enable not yet implemented")
    }

    fn in_interrupt() -> bool {
        unimplemented!("ARM64: in_interrupt not yet implemented")
    }

    fn in_hardirq() -> bool {
        unimplemented!("ARM64: in_hardirq not yet implemented")
    }

    fn can_schedule() -> bool {
        unimplemented!("ARM64: can_schedule not yet implemented")
    }
}
