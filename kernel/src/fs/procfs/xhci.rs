//! Procfs generator for /proc/xhci/trace
//!
//! Exposes xHCI runtime state through procfs so userspace
//! programs (like btrace) can read it.

use alloc::string::String;

/// Generate the content of /proc/xhci/trace by reading the clean-room controller.
pub fn generate_xhci_trace() -> String {
    alloc::format!(
        "xhci state={} initialized={} interrupts={}\n",
        crate::drivers::xhci::state(),
        crate::drivers::xhci::is_initialized(),
        crate::drivers::xhci::interrupt_count()
    )
}

/// Generate the content of /proc/xhci/counters.
pub fn generate_xhci_counters() -> String {
    use crate::drivers::hid_boot::NONZERO_KBD_COUNT;
    use crate::tracing::providers::xhci::{
        XHCI_IRQ_ENTRY_TOTAL, XHCI_LOCK_CONTENDED_TOTAL, XHCI_MSI_EVENT_TOTAL,
    };
    use alloc::format;
    use core::sync::atomic::Ordering;

    // The heartbeat reads the nonempty report count from hid_boot::note_report.
    // It observes reports received, not characters consumed by a terminal.
    let result = format!(
        "XHCI_MSI_EVENT_TOTAL={}\nXHCI_IRQ_ENTRY_TOTAL={}\nXHCI_LOCK_CONTENDED_TOTAL={}\nKBD_NONZERO_TOTAL={}\n",
        XHCI_MSI_EVENT_TOTAL.aggregate(),
        XHCI_IRQ_ENTRY_TOTAL.aggregate(),
        XHCI_LOCK_CONTENDED_TOTAL.aggregate(),
        NONZERO_KBD_COUNT.load(Ordering::Relaxed)
    );
    #[cfg(target_arch = "x86_64")]
    {
        let mut result = result;
        use crate::drivers::xhci::host_keys;
        use core::fmt::Write;
        writeln!(result, "HOSTKEY_QUEUED={} HOSTKEY_REPLAYED={} HOSTKEY_DISPATCHED={} HOSTKEY_CHANGED={} HOSTKEY_REPLAY_MAX_NS={}",
            host_keys::QUEUED.load(Ordering::Relaxed), host_keys::REPLAYED.load(Ordering::Relaxed),
            host_keys::DISPATCHED.load(Ordering::Relaxed), host_keys::CHANGED.load(Ordering::Relaxed),
            host_keys::REPLAY_MAX_NS.load(Ordering::Relaxed)).unwrap();
        result
    }
    #[cfg(not(target_arch = "x86_64"))]
    result
}
