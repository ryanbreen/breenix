//! Signal handling infrastructure for Breenix
//!
//! This module implements POSIX-compatible signal handling, including:
//! - Signal constants (SIGKILL, SIGTERM, etc.)
//! - Process dispositions and pending signals, with thread-owned masks and waits
//! - Signal delivery to userspace handlers
//! - Signal trampoline for returning from handlers
//!
//! Signal delivery occurs at the return-to-userspace boundary in
//! `interrupts/context_switch.rs`.

pub mod constants;
pub mod delivery;
pub mod timers;
mod timer_signal;
pub mod trampoline;
pub mod types;

pub use types::*;

/// Minimum space needed for one delivery on this machine.
pub fn minimum_alt_stack() -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch_impl::x86_64::fpu::minimum_stack()
    }
    #[cfg(target_arch = "aarch64")]
    {
        constants::MINSIGSTKSZ.max(types::SignalFrame::SIZE + trampoline::SIGNAL_TRAMPOLINE_SIZE + 30)
    }
}
