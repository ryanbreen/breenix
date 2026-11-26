//! ARM64 Boot Assembly Module
//!
//! This module includes the ARM64 boot assembly code using global_asm!.
//! The assembly provides:
//! - Entry point (_start)
//! - EL2 to EL1 transition
//! - BSS zeroing
//! - Exception vector table setup
//! - Jump to kernel_main

#[cfg(not(breenix_bas_kernel_asm))]
use core::arch::global_asm;

// Include the shared resume-PC guard macros and boot assembly as one unit.
#[cfg(not(breenix_bas_kernel_asm))]
global_asm!(concat!(
    include_str!("resume_pc_guard.inc"),
    include_str!("boot.S")
));
