//! xHCI trace provider and counters.
//!
//! Counters are lock-free and safe to increment from the xHCI interrupt path.

use crate::tracing::counter::{register_counter, TraceCounter};
use crate::tracing::provider::{register_provider, TraceProvider};
use core::sync::atomic::{AtomicU64, Ordering};

/// Provider ID for xHCI events (0x08xx range).
pub const PROVIDER_ID: u8 = 0x08;

/// xHCI trace provider.
///
/// GDB: `print XHCI_PROVIDER`
#[no_mangle]
pub static XHCI_PROVIDER: TraceProvider = TraceProvider {
    name: "xhci",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

/// Total xHCI MSI-delivered event-ring TRBs handled.
///
/// GDB: `print XHCI_MSI_EVENT_TOTAL`
#[no_mangle]
pub static XHCI_MSI_EVENT_TOTAL: TraceCounter =
    TraceCounter::new("XHCI_MSI_EVENT_TOTAL", "xHCI MSI event-ring TRBs handled");

/// Total xHCI IRQ handler entries.
///
/// GDB: `print XHCI_IRQ_ENTRY_TOTAL`
#[no_mangle]
pub static XHCI_IRQ_ENTRY_TOTAL: TraceCounter =
    TraceCounter::new("XHCI_IRQ_ENTRY_TOTAL", "xHCI IRQ handler entries");

/// Total xHCI IRQ entries that found the controller lock contended.
///
/// GDB: `print XHCI_LOCK_CONTENDED_TOTAL`
#[no_mangle]
pub static XHCI_LOCK_CONTENDED_TOTAL: TraceCounter = TraceCounter::new(
    "XHCI_LOCK_CONTENDED_TOTAL",
    "xHCI IRQ lock contention events",
);

/// Register xHCI provider and counters.
pub fn init() {
    register_provider(&XHCI_PROVIDER);
    register_counter(&XHCI_MSI_EVENT_TOTAL);
    register_counter(&XHCI_IRQ_ENTRY_TOTAL);
    register_counter(&XHCI_LOCK_CONTENDED_TOTAL);
}

#[inline(always)]
pub fn count_irq_entry() {
    XHCI_IRQ_ENTRY_TOTAL.increment();
}

#[inline(always)]
pub fn count_msi_event() {
    XHCI_MSI_EVENT_TOTAL.increment();
}

#[inline(always)]
pub fn count_lock_contended() {
    XHCI_LOCK_CONTENDED_TOTAL.increment();
}
