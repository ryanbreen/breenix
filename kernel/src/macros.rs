#[macro_export]
macro_rules! delay {
    ($ticks:expr) => {{
        let start = $crate::time::monotonic_clock();
        let target = start + $ticks;
        while $crate::time::monotonic_clock() < target {
            core::hint::spin_loop();
        }
    }};
}