//! WASM32 Hardware Abstraction Layer
//!
//! Provides platform-specific implementations for the wasm32 target.
//! In the browser, there are no interrupts, no real timer hardware,
//! and serial output routes to the terminal pane.

/// Execute a closure with interrupts disabled.
/// On wasm32, there are no interrupts -- just execute the closure directly.
pub fn arch_without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    f()
}

/// Get the current time in milliseconds since epoch.
/// Uses JavaScript `Date.now()` via js-sys.
pub fn current_time_ms() -> f64 {
    js_sys::Date::now()
}

/// Get the current Unix timestamp in seconds.
pub fn current_unix_time() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}
