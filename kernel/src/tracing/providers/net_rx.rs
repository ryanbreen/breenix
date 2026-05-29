//! VirtIO-net RX trace provider and counters.
//!
//! Counters are lock-free and safe to increment from the network interrupt
//! path. They are intentionally coarse-grained so RX triage can identify the
//! first stage where inbound packets stop flowing.

use crate::tracing::counter::{register_counter, TraceCounter};
use crate::tracing::provider::{register_provider, TraceProvider};
use core::sync::atomic::{AtomicU64, Ordering};

/// Provider ID for network RX events (0x09xx range).
pub const PROVIDER_ID: u8 = 0x09;

/// Network RX trace provider.
///
/// GDB: `print NET_RX_PROVIDER`
#[no_mangle]
pub static NET_RX_PROVIDER: TraceProvider = TraceProvider {
    name: "net_rx",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

/// VirtIO-net MSI-X RX interrupt handler entries.
#[no_mangle]
pub static NET_RX_MSI_TOTAL: TraceCounter =
    TraceCounter::new("NET_RX_MSI_TOTAL", "VirtIO-net RX MSI handler entries");

/// VirtIO-net RX used-ring entries consumed.
#[no_mangle]
pub static NET_RX_RING_DRAIN_TOTAL: TraceCounter = TraceCounter::new(
    "NET_RX_RING_DRAIN_TOTAL",
    "VirtIO-net RX used-ring entries drained",
);

/// Ethernet frames dispatched upward from the RX path.
#[no_mangle]
pub static NET_RX_FRAME_TOTAL: TraceCounter =
    TraceCounter::new("NET_RX_FRAME_TOTAL", "Network RX Ethernet frames parsed");

/// ARP frames delivered to the ARP handler.
#[no_mangle]
pub static NET_RX_ARP_TOTAL: TraceCounter =
    TraceCounter::new("NET_RX_ARP_TOTAL", "Network RX ARP handler entries");

/// Parsed Ethernet frames whose EtherType is not ARP.
#[no_mangle]
pub static NET_RX_ETHERTYPE_OTHER_TOTAL: TraceCounter = TraceCounter::new(
    "NET_RX_ETHERTYPE_OTHER_TOTAL",
    "Network RX non-ARP EtherType frames",
);

/// NetRx softirq handler entries.
#[no_mangle]
pub static NET_RX_SOFTIRQ_ENTRY_TOTAL: TraceCounter =
    TraceCounter::new("NET_RX_SOFTIRQ_ENTRY_TOTAL", "NetRx softirq entries");

/// NetRx softirq handler exits.
#[no_mangle]
pub static NET_RX_SOFTIRQ_EXIT_TOTAL: TraceCounter =
    TraceCounter::new("NET_RX_SOFTIRQ_EXIT_TOTAL", "NetRx softirq exits");

/// NetRx drains skipped because another drain owns RX_PROCESSING.
#[no_mangle]
pub static NET_RX_REENTRANT_SKIP_TOTAL: TraceCounter = TraceCounter::new(
    "NET_RX_REENTRANT_SKIP_TOTAL",
    "NetRx drains skipped by RX_PROCESSING guard",
);

/// RX guard releases after a completed drain.
#[no_mangle]
pub static NET_RX_GUARD_RELEASE_TOTAL: TraceCounter =
    TraceCounter::new("NET_RX_GUARD_RELEASE_TOTAL", "RX_PROCESSING guard releases");

/// RX callback re-arm checks.
#[no_mangle]
pub static NET_RX_REARM_CHECK_TOTAL: TraceCounter =
    TraceCounter::new("NET_RX_REARM_CHECK_TOTAL", "VirtIO-net RX re-arm checks");

/// RX callback re-arm checks that found pending used entries.
#[no_mangle]
pub static NET_RX_REARM_RACE_TOTAL: TraceCounter = TraceCounter::new(
    "NET_RX_REARM_RACE_TOTAL",
    "VirtIO-net RX re-arm checks with pending used entries",
);

/// RX callback re-arm checks that left callbacks enabled.
#[no_mangle]
pub static NET_RX_REARM_ARMED_TOTAL: TraceCounter = TraceCounter::new(
    "NET_RX_REARM_ARMED_TOTAL",
    "VirtIO-net RX re-arm checks leaving callbacks enabled",
);

/// Register network RX provider and counters.
pub fn init() {
    register_provider(&NET_RX_PROVIDER);
    register_counter(&NET_RX_MSI_TOTAL);
    register_counter(&NET_RX_RING_DRAIN_TOTAL);
    register_counter(&NET_RX_FRAME_TOTAL);
    register_counter(&NET_RX_ARP_TOTAL);
    register_counter(&NET_RX_ETHERTYPE_OTHER_TOTAL);
    register_counter(&NET_RX_SOFTIRQ_ENTRY_TOTAL);
    register_counter(&NET_RX_SOFTIRQ_EXIT_TOTAL);
    register_counter(&NET_RX_REENTRANT_SKIP_TOTAL);
    register_counter(&NET_RX_GUARD_RELEASE_TOTAL);
    register_counter(&NET_RX_REARM_CHECK_TOTAL);
    register_counter(&NET_RX_REARM_RACE_TOTAL);
    register_counter(&NET_RX_REARM_ARMED_TOTAL);
}

#[inline(always)]
pub fn count_msi() {
    crate::trace_count!(NET_RX_MSI_TOTAL);
}

#[inline(always)]
pub fn count_ring_drain() {
    crate::trace_count!(NET_RX_RING_DRAIN_TOTAL);
}

#[inline(always)]
pub fn count_frame() {
    crate::trace_count!(NET_RX_FRAME_TOTAL);
}

#[inline(always)]
pub fn count_arp() {
    crate::trace_count!(NET_RX_ARP_TOTAL);
}

#[inline(always)]
pub fn count_ethertype_other() {
    crate::trace_count!(NET_RX_ETHERTYPE_OTHER_TOTAL);
}

#[inline(always)]
pub fn count_softirq_entry() {
    crate::trace_count!(NET_RX_SOFTIRQ_ENTRY_TOTAL);
}

#[inline(always)]
pub fn count_softirq_exit() {
    crate::trace_count!(NET_RX_SOFTIRQ_EXIT_TOTAL);
}

#[inline(always)]
pub fn count_reentrant_skip() {
    crate::trace_count!(NET_RX_REENTRANT_SKIP_TOTAL);
}

#[inline(always)]
pub fn count_guard_release() {
    crate::trace_count!(NET_RX_GUARD_RELEASE_TOTAL);
}

#[inline(always)]
pub fn count_rearm_check() {
    crate::trace_count!(NET_RX_REARM_CHECK_TOTAL);
}

#[inline(always)]
pub fn count_rearm_race() {
    crate::trace_count!(NET_RX_REARM_RACE_TOTAL);
}

#[inline(always)]
pub fn count_rearm_armed() {
    crate::trace_count!(NET_RX_REARM_ARMED_TOTAL);
}
