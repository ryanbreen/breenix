#[macro_export]
macro_rules! delay {
    ($millis:expr) => {{
        // Convert milliseconds to ticks (10Hz = 100ms per tick)
        let ticks_to_wait = ($millis + 99) / 100; // Round up
        let start = $crate::time::monotonic_clock();
        let target = start + ticks_to_wait;
        while $crate::time::monotonic_clock() < target {
            core::hint::spin_loop();
        }
    }};
}
