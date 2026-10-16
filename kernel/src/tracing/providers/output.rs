//! Kernel output accounting's global counter (`docs/gui/breengel.md` section 22), read through
//! `/proc/trace/counters`. Per-thread counts live with each thread (`crate::output_accounting`).

use crate::tracing::counter::{register_counter, TraceCounter};

/// Bytes committed for output where the producing thread could not be read safely.
#[no_mangle]
pub static KERNEL_OUTPUT_UNATTRIBUTED: TraceCounter = TraceCounter::new(
    "kernel_output_unattributed",
    "Kernel output bytes with no thread to account them to",
);

pub fn init() {
    assert!(
        register_counter(&KERNEL_OUTPUT_UNATTRIBUTED).is_some(),
        "trace counter registry overflow while registering kernel_output_unattributed"
    );
    crate::output_accounting::init();
}

#[inline]
pub fn count_unattributed(bytes: u64) {
    KERNEL_OUTPUT_UNATTRIBUTED.add(bytes);
}
