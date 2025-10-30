//! Test checkpoint infrastructure for fast, signal-driven testing
//!
//! Only compiled when cfg(feature = "testing") is enabled.

/// Emit a test checkpoint marker to serial output
/// Format: [CHECKPOINT:name]
#[macro_export]
macro_rules! test_checkpoint {
    ($name:expr) => {
        #[cfg(feature = "testing")]
        {
            log::info!("[CHECKPOINT:{}]", $name);
        }
    };
}
