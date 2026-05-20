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
    use crate::tracing::providers::counters::{
        SCHED_STALE_QUEUE_CURRENT, SCHED_STALE_QUEUE_DEFERRED, SCHED_STALE_QUEUE_NOT_READY,
    };
    use crate::tracing::providers::xhci::{
        XHCI_IRQ_ENTRY_TOTAL, XHCI_LOCK_CONTENDED_TOTAL, XHCI_MSI_EVENT_TOTAL,
    };
    use alloc::format;

    format!(
        "XHCI_MSI_EVENT_TOTAL={}\nXHCI_IRQ_ENTRY_TOTAL={}\nXHCI_LOCK_CONTENDED_TOTAL={}\nSCHED_STALE_QUEUE_NOT_READY={}\nSCHED_STALE_QUEUE_CURRENT={}\nSCHED_STALE_QUEUE_DEFERRED={}\n",
        XHCI_MSI_EVENT_TOTAL.aggregate(),
        XHCI_IRQ_ENTRY_TOTAL.aggregate(),
        XHCI_LOCK_CONTENDED_TOTAL.aggregate(),
        SCHED_STALE_QUEUE_NOT_READY.aggregate(),
        SCHED_STALE_QUEUE_CURRENT.aggregate(),
        SCHED_STALE_QUEUE_DEFERRED.aggregate()
    )
}
