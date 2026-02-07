//! x86_64 architecture implementation.
//!
//! This module contains all x86_64-specific code including:
//! - GDT/TSS management
//! - IDT and interrupt handling
//! - Page table operations
//! - Per-CPU data access via GS segment
//! - TSC timer operations
//! - PIC interrupt controller
//!
//! Note: This is a complete Hardware Abstraction Layer (HAL) API.
//! Many items are intentionally defined for API completeness even
//! if not currently used by the kernel.

// HAL modules define complete APIs - not all items are used yet
#[allow(unused_imports)]
pub mod constants;
pub mod cpu;
pub mod cpuinfo;
pub mod interrupt_frame;
pub mod paging;
pub mod percpu;
pub mod pic;
pub mod privilege;
pub mod timer;

// Re-export commonly used items
// These re-exports are part of the complete HAL API
#[allow(unused_imports)]
pub use constants::*;
#[allow(unused_imports)]
pub use cpu::X86Cpu;
#[allow(unused_imports)]
pub use interrupt_frame::X86InterruptFrame;
#[allow(unused_imports)]
pub use paging::{X86PageFlags, X86PageTableOps};
#[allow(unused_imports)]
pub use percpu::X86PerCpu;
#[allow(unused_imports)]
pub use pic::X86Pic;
#[allow(unused_imports)]
pub use privilege::X86PrivilegeLevel;
#[allow(unused_imports)]
pub use timer::X86Timer;
