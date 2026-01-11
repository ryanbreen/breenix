//! x86_64 architecture implementation.
//!
//! This module contains all x86_64-specific code including:
//! - GDT/TSS management
//! - IDT and interrupt handling
//! - Page table operations
//! - Per-CPU data access via GS segment
//! - TSC timer operations
//! - PIC interrupt controller

pub mod constants;
pub mod cpu;
pub mod paging;
pub mod percpu;
pub mod timer;

// Re-export commonly used items
pub use constants::*;
pub use cpu::X86Cpu;
pub use paging::{X86PageFlags, X86PageTableOps};
pub use percpu::X86PerCpu;
pub use timer::X86Timer;
