//! Tracing macros for low-overhead event recording and counting.
//!
//! This module provides macros that compile to minimal code when tracing is disabled
//! and emit efficient trace events when enabled. It also provides counter macros
//! for atomic statistics tracking.
//!
//! # Design Principles
//!
//! 1. **Near-zero overhead when disabled**: Single atomic load to check enable state
//! 2. **Minimal code when enabled**: ~5-10 instructions for basic trace_event!
//! 3. **No allocations**: All data is stack-based or static
//! 4. **No formatting**: Payloads are raw integers, not formatted strings
//! 5. **Compile-time provider binding**: Provider reference resolved at compile time
//!
//! # Usage - Trace Events
//!
//! ```rust,ignore
//! use kernel::tracing::{trace_event, trace_event_2};
//! use kernel::tracing::providers::SYSCALL_PROVIDER;
//! use kernel::tracing::TraceEventType;
//!
//! // Basic event with 32-bit payload
//! trace_event!(SYSCALL_PROVIDER, TraceEventType::SYSCALL_ENTRY, syscall_nr as u32);
//!
//! // Event with two 16-bit values packed into payload
//! trace_event_2!(SCHED_PROVIDER, TraceEventType::CTX_SWITCH_ENTRY, old_tid as u16, new_tid as u16);
//! ```
//!
//! # Usage - Counters
//!
//! ```rust,ignore
//! use kernel::tracing::counter::{define_trace_counter, trace_count};
//!
//! // Define a counter
//! define_trace_counter!(SYSCALL_TOTAL, "Total syscall invocations");
//!
//! // Increment the counter
//! fn handle_syscall() {
//!     trace_count!(SYSCALL_TOTAL);
//!     // ...
//! }
//!
//! // Query the counter
//! let total = SYSCALL_TOTAL.aggregate();
//! ```

/// Record a trace event if the provider is enabled.
///
/// This macro performs a fast check of the provider's enable state before
/// recording the event. If tracing is globally disabled or the provider
/// is disabled, no event is recorded.
///
/// # Parameters
///
/// - `$provider`: Reference to a `TraceProvider` static
/// - `$event_type`: Event type constant (u16)
/// - `$payload`: 32-bit payload value
///
/// # Example
///
/// ```rust,ignore
/// trace_event!(SYSCALL_PROVIDER, TraceEventType::SYSCALL_ENTRY, syscall_nr as u32);
/// ```
///
/// # Generated Code
///
/// Approximately 5-10 instructions when the fast path (disabled) is taken:
/// 1. Load global enable flag (atomic relaxed)
/// 2. Test and branch (skip if disabled)
/// 3. Load provider enable bitmap (atomic relaxed)
/// 4. Test and branch (skip if disabled)
/// 5. Call record_event (inline)
#[macro_export]
macro_rules! trace_event {
    ($provider:expr, $event_type:expr, $payload:expr) => {
        // Fast path: check if provider is enabled (single atomic load)
        if $provider.is_enabled() && $crate::tracing::is_enabled() {
            $crate::tracing::record_event($event_type, 0, $payload);
        }
    };
    ($provider:expr, $event_type:expr, $flags:expr, $payload:expr) => {
        if $provider.is_enabled() && $crate::tracing::is_enabled() {
            $crate::tracing::record_event($event_type, $flags, $payload);
        }
    };
}

/// Record a trace event with two 16-bit values packed into the payload.
///
/// This is a convenience macro for events that need two small values,
/// such as context switch (old_tid, new_tid) or syscall (number, errno).
///
/// The values are packed as: (val1 << 16) | val2
///
/// # Parameters
///
/// - `$provider`: Reference to a `TraceProvider` static
/// - `$event_type`: Event type constant (u16)
/// - `$val1`: First 16-bit value (stored in upper 16 bits)
/// - `$val2`: Second 16-bit value (stored in lower 16 bits)
///
/// # Example
///
/// ```rust,ignore
/// trace_event_2!(SCHED_PROVIDER, TraceEventType::CTX_SWITCH_ENTRY, old_tid, new_tid);
/// ```
#[macro_export]
macro_rules! trace_event_2 {
    ($provider:expr, $event_type:expr, $val1:expr, $val2:expr) => {
        if $provider.is_enabled() && $crate::tracing::is_enabled() {
            $crate::tracing::record_event_2($event_type, $val1 as u16, $val2 as u16);
        }
    };
}

/// Define a trace provider with associated probes.
///
/// This macro creates a static `TraceProvider` and associated probe constants.
/// The provider starts disabled; call `enable_all()` or `enable_probe()` to
/// enable tracing.
///
/// # Parameters
///
/// - `$vis`: Visibility modifier (pub, pub(crate), etc.)
/// - `$name`: Provider name (identifier, becomes the static name)
/// - `$display_name`: Human-readable name (string literal)
/// - `$provider_id`: Provider ID (u8, should be unique)
///
/// # Example
///
/// ```rust,ignore
/// define_trace_provider!(
///     pub SYSCALL_PROVIDER,
///     "syscall",
///     0x03
/// );
/// ```
#[macro_export]
macro_rules! define_trace_provider {
    ($vis:vis $name:ident, $display_name:expr, $provider_id:expr) => {
        $vis static $name: $crate::tracing::provider::TraceProvider =
            $crate::tracing::provider::TraceProvider::new($display_name, $provider_id);
    };
}

/// Define probe constants for a provider.
///
/// This macro creates constants for probe IDs and their corresponding event types.
/// It's typically used alongside `define_trace_provider!`.
///
/// # Parameters
///
/// - `$provider_id`: The provider's ID (must match the provider definition)
/// - `$probe_name`: Name of the probe constant
/// - `$probe_id`: Probe ID within the provider (0-255)
///
/// # Example
///
/// ```rust,ignore
/// // Define probes for the syscall provider (provider_id = 0x03)
/// define_trace_probes! {
///     provider_id: 0x03,
///     SYSCALL_ENTRY => 0x00,
///     SYSCALL_EXIT => 0x01,
/// }
/// ```
#[macro_export]
macro_rules! define_trace_probes {
    (
        provider_id: $provider_id:expr,
        $($probe_name:ident => $probe_id:expr),* $(,)?
    ) => {
        $(
            pub const $probe_name: u16 = (($provider_id as u16) << 8) | ($probe_id as u16);
        )*
    };
}

// =============================================================================
// Counter Macros
// =============================================================================

/// Define a trace counter with the given name and description.
///
/// This macro creates a static `TraceCounter` that can be incremented
/// with `trace_count!`. The counter uses per-CPU storage to avoid
/// contention and compiles to a single atomic add operation.
///
/// # Parameters
///
/// - `$name`: Counter name (identifier, becomes the static name with _COUNTER suffix)
/// - `$description`: Human-readable description (string literal)
///
/// # Example
///
/// ```rust,ignore
/// define_trace_counter!(SYSCALL_TOTAL, "Total syscall invocations");
///
/// // The counter can be accessed as SYSCALL_TOTAL_COUNTER
/// let total = SYSCALL_TOTAL_COUNTER.aggregate();
/// ```
///
/// # GDB Inspection
///
/// ```gdb
/// print SYSCALL_TOTAL_COUNTER
/// print SYSCALL_TOTAL_COUNTER.per_cpu[0].value
/// ```
#[macro_export]
macro_rules! define_trace_counter {
    ($name:ident, $description:expr) => {
        /// Trace counter for statistics tracking.
        #[no_mangle]
        pub static $name: $crate::tracing::counter::TraceCounter =
            $crate::tracing::counter::TraceCounter::new(stringify!($name), $description);
    };
    ($vis:vis $name:ident, $description:expr) => {
        /// Trace counter for statistics tracking.
        #[no_mangle]
        $vis static $name: $crate::tracing::counter::TraceCounter =
            $crate::tracing::counter::TraceCounter::new(stringify!($name), $description);
    };
}

/// Increment a trace counter.
///
/// This macro performs an atomic increment of the counter for the current CPU.
/// It compiles to a single atomic add instruction with relaxed ordering.
///
/// # Parameters
///
/// - `$counter`: Reference to a `TraceCounter` static
///
/// # Example
///
/// ```rust,ignore
/// define_trace_counter!(SYSCALL_TOTAL, "Total syscall invocations");
///
/// fn handle_syscall() {
///     trace_count!(SYSCALL_TOTAL);
///     // ... handle syscall ...
/// }
/// ```
///
/// # Generated Code
///
/// Compiles to approximately 3-5 instructions:
/// 1. Get current CPU ID
/// 2. Calculate per-CPU slot address
/// 3. Atomic add (lock xadd on x86)
#[macro_export]
macro_rules! trace_count {
    ($counter:expr) => {
        $counter.increment();
    };
}

/// Increment a trace counter by a specific amount.
///
/// Like `trace_count!` but adds a custom value instead of 1.
///
/// # Parameters
///
/// - `$counter`: Reference to a `TraceCounter` static
/// - `$amount`: The value to add (u64)
///
/// # Example
///
/// ```rust,ignore
/// define_trace_counter!(BYTES_READ, "Total bytes read from disk");
///
/// fn read_block(size: usize) {
///     trace_count_add!(BYTES_READ, size as u64);
///     // ... read data ...
/// }
/// ```
#[macro_export]
macro_rules! trace_count_add {
    ($counter:expr, $amount:expr) => {
        $counter.add($amount);
    };
}

// Re-export macros at crate level for convenient use
pub use define_trace_counter;
pub use define_trace_probes;
pub use define_trace_provider;
pub use trace_count;
pub use trace_count_add;
pub use trace_event;
pub use trace_event_2;
