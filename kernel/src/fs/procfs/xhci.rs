//! Procfs generator for /proc/xhci/trace
//!
//! Exposes the xHCI binary trace buffer through procfs so userspace
//! programs (like btrace) can read it.

use alloc::string::String;

/// Generate the content of /proc/xhci/trace by reading the static xHCI trace buffers.
pub fn generate_xhci_trace() -> String {
    crate::drivers::usb::xhci::format_trace_buffer()
}
