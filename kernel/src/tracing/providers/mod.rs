//! Built-in trace providers and counters for kernel subsystems.
//!
//! This module contains pre-defined providers for common kernel operations:
//! - `syscall`: System call entry/exit tracing
//! - `sched`: Scheduler and context switch tracing
//! - `irq`: Interrupt tracing
//!
//! It also contains built-in counters for kernel statistics:
//! - `SYSCALL_TOTAL`: Total syscall invocations
//! - `IRQ_TOTAL`: Total interrupt invocations
//! - `CTX_SWITCH_TOTAL`: Total context switches
//! - `TIMER_TICK_TOTAL`: Total timer tick interrupts
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::providers::{SYSCALL_PROVIDER, SCHED_PROVIDER, IRQ_PROVIDER};
//!
//! // Enable all syscall tracing
//! SYSCALL_PROVIDER.enable_all();
//!
//! // Enable specific sched probes
//! SCHED_PROVIDER.enable_probe(0); // CTX_SWITCH
//!
//! // Query counters
//! let total_syscalls = SYSCALL_TOTAL.aggregate();
//! ```

pub mod counters;
pub mod irq;
pub mod process;
pub mod sched;
pub mod syscall;

// Re-export providers for convenient access
pub use irq::IRQ_PROVIDER;
pub use process::PROCESS_PROVIDER;
pub use sched::SCHED_PROVIDER;
pub use syscall::SYSCALL_PROVIDER;

// Re-export built-in counters
pub use counters::{CTX_SWITCH_TOTAL, IRQ_TOTAL, SYSCALL_TOTAL, TIMER_TICK_TOTAL};

/// Initialize all built-in providers and counters.
///
/// This registers all providers and counters with their global registries.
/// Should be called once during kernel initialization.
pub fn init() {
    syscall::init();
    sched::init();
    irq::init();
    process::init();
    counters::init();

    log::info!(
        "Tracing providers initialized: syscall={:#x}, sched={:#x}, irq={:#x}, process={:#x}",
        syscall::PROVIDER_ID,
        sched::PROVIDER_ID,
        irq::PROVIDER_ID,
        process::PROVIDER_ID
    );
}

/// Enable all built-in providers.
#[allow(dead_code)]
pub fn enable_all() {
    SYSCALL_PROVIDER.enable_all();
    SCHED_PROVIDER.enable_all();
    IRQ_PROVIDER.enable_all();
    PROCESS_PROVIDER.enable_all();
}

/// Disable all built-in providers.
#[allow(dead_code)]
pub fn disable_all() {
    SYSCALL_PROVIDER.disable_all();
    SCHED_PROVIDER.disable_all();
    IRQ_PROVIDER.disable_all();
    PROCESS_PROVIDER.disable_all();
}
