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
pub mod cpu0_timer_forensics;
pub mod irq;
pub mod net_rx;
pub mod process;
pub mod sched;
pub mod syscall;
pub mod virtgpu;
pub mod xhci;
// #[cfg(feature = "btrt")]
// pub mod boot_test;

// Re-export providers for convenient access
pub use cpu0_timer_forensics::CPU0_TIMER_FORENSICS_PROVIDER;
pub use irq::IRQ_PROVIDER;
pub use net_rx::NET_RX_PROVIDER;
pub use process::PROCESS_PROVIDER;
pub use sched::SCHED_PROVIDER;
pub use syscall::SYSCALL_PROVIDER;
pub use virtgpu::VIRTGPU_PROVIDER;
pub use xhci::XHCI_PROVIDER;
// #[cfg(feature = "btrt")]
// pub use boot_test::BOOT_TEST_PROVIDER;

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
    net_rx::init();
    process::init();
    virtgpu::init();
    xhci::init();
    // #[cfg(feature = "btrt")]
    // boot_test::init();
    counters::init();

    log::info!(
        "Tracing providers initialized: syscall={:#x}, sched={:#x}, irq={:#x}, net_rx={:#x}, process={:#x}, virtgpu={:#x}, xhci={:#x}",
        syscall::PROVIDER_ID,
        sched::PROVIDER_ID,
        irq::PROVIDER_ID,
        net_rx::PROVIDER_ID,
        process::PROVIDER_ID,
        virtgpu::PROVIDER_ID,
        xhci::PROVIDER_ID
    );
}

/// Enable all built-in providers.
#[allow(dead_code)]
pub fn enable_all() {
    SYSCALL_PROVIDER.enable_all();
    SCHED_PROVIDER.enable_all();
    IRQ_PROVIDER.enable_all();
    NET_RX_PROVIDER.enable_all();
    PROCESS_PROVIDER.enable_all();
    VIRTGPU_PROVIDER.enable_all();
    XHCI_PROVIDER.enable_all();
    // #[cfg(feature = "btrt")]
    // BOOT_TEST_PROVIDER.enable_all();
}

/// Disable all built-in providers.
#[allow(dead_code)]
pub fn disable_all() {
    SYSCALL_PROVIDER.disable_all();
    SCHED_PROVIDER.disable_all();
    IRQ_PROVIDER.disable_all();
    NET_RX_PROVIDER.disable_all();
    PROCESS_PROVIDER.disable_all();
    VIRTGPU_PROVIDER.disable_all();
    XHCI_PROVIDER.disable_all();
    // #[cfg(feature = "btrt")]
    // BOOT_TEST_PROVIDER.disable_all();
}
