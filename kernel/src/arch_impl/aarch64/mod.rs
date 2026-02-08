//! AArch64 (ARM64) architecture implementation.
//!
//! This module provides the AArch64 Hardware Abstraction Layer (HAL) for
//! Breenix.

#![allow(dead_code)]

// Boot assembly - must be included to link _start and exception vectors
pub mod boot;

// HAL modules define complete APIs - not all items are used yet
#[allow(unused_imports)]
pub mod constants;
pub mod cpu;
pub mod cpuinfo;
pub mod elf;
pub mod exception;
pub mod exception_frame;
pub mod paging;
pub mod percpu;
pub mod gic;
pub mod privilege;
pub mod timer;
pub mod timer_interrupt;
pub mod mmu;
pub mod context;
pub mod context_switch;
pub mod smp;
pub mod syscall_entry;
pub mod trace;

// Re-export commonly used items
// These re-exports are part of the complete HAL API
#[allow(unused_imports)]
pub use constants::*;
#[allow(unused_imports)]
pub use cpu::Aarch64Cpu;
#[allow(unused_imports)]
pub use exception_frame::Aarch64ExceptionFrame;
#[allow(unused_imports)]
pub use paging::{Aarch64PageFlags, Aarch64PageTableOps};
#[allow(unused_imports)]
pub use percpu::Aarch64PerCpu;
#[allow(unused_imports)]
pub use gic::Gicv2;
#[allow(unused_imports)]
pub use privilege::Aarch64PrivilegeLevel;
#[allow(unused_imports)]
pub use timer::Aarch64Timer;
#[allow(unused_imports)]
pub use syscall_entry::{is_el0_confirmed, syscall_return_to_userspace_aarch64};
#[allow(unused_imports)]
pub use context_switch::{
    check_need_resched_and_switch_arm64,
    create_saved_regs_from_frame,
    idle_loop_arm64,
    perform_context_switch,
    switch_to_new_thread,
    switch_to_user,
};

// Re-export interrupt control functions for convenient access
// These provide the ARM64 equivalent of x86_64::instructions::interrupts::*
#[allow(unused_imports)]
pub use cpu::{
    disable_interrupts,
    enable_interrupts,
    interrupts_enabled,
    without_interrupts,
};
