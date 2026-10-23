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

/// A gauge: the longest interval, in microseconds, the display copy has held IRQs masked
/// since boot or the last `DISARM_ALL` (sections 18 and 21). It lives in CPU 0's slot, so the
/// sum `/proc/trace/counters` prints is the maximum itself.
#[no_mangle]
pub static DISPLAY_IRQ_MASKED_MAX_US: TraceCounter = TraceCounter::new(
    "display_irq_masked_max_us",
    "Longest IRQ-masked interval in the display copy, microseconds",
);

pub fn init() {
    for counter in [
        &WINDOW_COMPOSITES,
        &WINDOW_COMPOSITE_PIXELS,
        &WINDOW_DAMAGE_RECTS,
        &WINDOW_BUFFER_ALLOCS,
        &DISPLAY_IRQ_MASKED_MAX_US,
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

/// The display copy held IRQs masked for `us` microseconds: raise the gauge to it. Display
/// copies run one at a time (composites are serialized), so the read and the raise do not race.
#[inline]
pub fn record_irq_masked(us: u64) {
    let now = DISPLAY_IRQ_MASKED_MAX_US.get_cpu(0);
    if us > now {
        DISPLAY_IRQ_MASKED_MAX_US.add_cpu(0, us - now);
    }
}

/// Restart the IRQ-masked gauge at 0.
#[inline]
pub fn reset_irq_masked() {
    DISPLAY_IRQ_MASKED_MAX_US.reset();
}
