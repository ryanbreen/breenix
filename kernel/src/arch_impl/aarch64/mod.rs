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
pub mod elf;
pub mod exception;
pub mod exception_frame;
pub mod paging;
pub mod percpu;
pub mod gic;
pub mod privilege;
pub mod timer;
pub mod mmu;
pub mod context;

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
