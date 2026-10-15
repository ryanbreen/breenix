//! Window interface counters (`docs/gui/breengel.md` section 18), read through
//! `/proc/trace/counters`.

use crate::tracing::counter::{register_counter, TraceCounter};

/// Composites completed.
#[no_mangle]
pub static WINDOW_COMPOSITES: TraceCounter = TraceCounter::new("composites", "Composites completed");

/// Pixels composed, summed over completed composites.
#[no_mangle]
pub static WINDOW_COMPOSITE_PIXELS: TraceCounter =
    TraceCounter::new("composite_pixels", "Pixels composed by completed composites");

/// Damage rectangles recorded by commits.
#[no_mangle]
pub static WINDOW_DAMAGE_RECTS: TraceCounter =
    TraceCounter::new("damage_rects", "Damage rectangles recorded by window commits");

/// Window buffers allocated (create and resize).
#[no_mangle]
pub static WINDOW_BUFFER_ALLOCS: TraceCounter =
    TraceCounter::new("window_buffer_allocs", "Window buffers allocated");

pub fn init() {
    for counter in [
        &WINDOW_COMPOSITES,
        &WINDOW_COMPOSITE_PIXELS,
        &WINDOW_DAMAGE_RECTS,
        &WINDOW_BUFFER_ALLOCS,
    ] {
        assert!(
            register_counter(counter).is_some(),
            "trace counter registry overflow while registering the window counters"
        );
    }
}

#[inline]
pub fn count_composite(pixels: u64) {
    WINDOW_COMPOSITES.increment();
    WINDOW_COMPOSITE_PIXELS.add(pixels);
}

#[inline]
pub fn count_damage_rects(rects: u64) {
    WINDOW_DAMAGE_RECTS.add(rects);
}

#[inline]
pub fn count_buffer_alloc() {
    WINDOW_BUFFER_ALLOCS.increment();
}
