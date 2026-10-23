//! A finite correction at 500 parts per million, independent of timer ticks.
//! Negative corrections slow realtime to 0.9995 of monotonic, retaining a positive rate.
pub fn applied(delta_ns: i64, elapsed_ns: u64) -> i64 {
    let magnitude = (elapsed_ns / 2000).min(delta_ns.unsigned_abs());
    if delta_ns < 0 {
        -(magnitude as i64)
    } else {
        magnitude as i64
    }
}
