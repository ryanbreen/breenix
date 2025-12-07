//! Signal handling infrastructure for Breenix
//!
//! This module implements POSIX-compatible signal handling, including:
//! - Signal constants (SIGKILL, SIGTERM, etc.)
//! - Per-process signal state (pending, blocked, handlers)
//! - Signal delivery to userspace handlers
//! - Signal trampoline for returning from handlers
//!
//! Signal delivery occurs at the return-to-userspace boundary in
//! `interrupts/context_switch.rs`.

pub mod constants;
pub mod delivery;
pub mod trampoline;
pub mod types;

pub use types::*;
