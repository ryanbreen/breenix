//! Bounds for the timer ticks hidden among timer-gate IPIs.
pub fn arrival_in_bounds(entries: u64, ipis: u64, expected: u64, tolerance: u64) -> bool {
    entries.saturating_sub(ipis).saturating_add(tolerance) >= expected
        && entries <= expected.saturating_add(tolerance)
}
