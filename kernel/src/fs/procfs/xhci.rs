//! Procfs generator for /proc/xhci/trace
//!
//! Exposes the xHCI binary trace buffer through procfs so userspace
//! programs (like btrace) can read it.

use alloc::string::String;

/// Generate the content of /proc/xhci/trace by reading the static xHCI trace buffers.
pub fn generate_xhci_trace() -> String {
    crate::drivers::usb::xhci::format_trace_buffer()
}

/// Generate the content of /proc/xhci/counters.
pub fn generate_xhci_counters() -> String {
    use crate::drivers::usb::hid::NONZERO_KBD_COUNT;
    use crate::tracing::providers::xhci::{
        XHCI_IRQ_ENTRY_TOTAL, XHCI_LOCK_CONTENDED_TOTAL, XHCI_MSI_EVENT_TOTAL,
    };
    use alloc::format;
    use core::sync::atomic::Ordering;

    // KBD_NONZERO_TOTAL: count of non-empty USB-HID keyboard reports seen by
    // the kernel (kernel/src/drivers/usb/hid.rs::NONZERO_KBD_COUNT). Read
    // periodically by userspace heartbeat (see heartbeat.rs) as a
    // guest-observable, monotonic proof that a keystroke actually reached
    // the guest's HID stack -- used by the Parallels launcher-smoke test
    // harness's keyboard-delivery handshake to detect a host-side input
    // wedge before running the real test gesture.
    format!(
        "XHCI_MSI_EVENT_TOTAL={}\nXHCI_IRQ_ENTRY_TOTAL={}\nXHCI_LOCK_CONTENDED_TOTAL={}\nKBD_NONZERO_TOTAL={}\n",
        XHCI_MSI_EVENT_TOTAL.aggregate(),
        XHCI_IRQ_ENTRY_TOTAL.aggregate(),
        XHCI_LOCK_CONTENDED_TOTAL.aggregate(),
        NONZERO_KBD_COUNT.load(Ordering::Relaxed)
    )
}
