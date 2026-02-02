//! Provider/Probe registration and management.
//!
//! This module implements the DTrace-style provider/probe model for trace events.
//! Providers represent kernel subsystems (syscall, sched, irq), and probes are
//! individual trace points within those subsystems.
//!
//! # Design Principles
//!
//! 1. **Static registration**: Providers and probes are statically defined at compile time
//! 2. **Lock-free enable/disable**: Uses atomic bitmaps for zero-contention control
//! 3. **Minimal overhead**: Single atomic load to check if a probe is enabled
//! 4. **GDB-inspectable**: All state is visible via GDB symbols
//!
//! # Usage
//!
//! ```rust,ignore
//! // Define a provider (typically in a providers/*.rs file)
//! pub static SYSCALL_PROVIDER: TraceProvider = TraceProvider::new(
//!     "syscall",
//!     0x03,  // Provider ID (matches TraceEventType::SYSCALL_* range)
//! );
//!
//! // Check if provider is enabled before recording
//! if SYSCALL_PROVIDER.is_enabled() {
//!     record_event(TraceEventType::SYSCALL_ENTRY, 0, syscall_nr as u32);
//! }
//! ```

use core::sync::atomic::{AtomicU64, Ordering};

// =============================================================================
// Provider Structure
// =============================================================================

/// A trace provider represents a kernel subsystem that emits trace events.
///
/// Providers are identified by:
/// - A human-readable name (e.g., "syscall", "sched", "irq")
/// - A unique provider ID that forms the upper byte of event types (0x00-0xFF)
///
/// Each provider has a 64-bit enable bitmap where each bit corresponds to
/// a probe within that provider (up to 64 probes per provider).
#[repr(C)]
pub struct TraceProvider {
    /// Provider name (e.g., "syscall", "sched", "irq").
    /// Used for human-readable output and GDB inspection.
    pub name: &'static str,

    /// Unique provider ID (0x00-0xFF).
    /// This forms the upper byte of event types: (provider_id << 8) | probe_id.
    pub id: u8,

    /// Enable bitmap for this provider's probes.
    /// Bit N corresponds to probe ID N within this provider.
    /// A value of 0 means the provider is entirely disabled.
    /// A value of u64::MAX means all probes are enabled.
    pub enabled: AtomicU64,
}

impl TraceProvider {
    /// Create a new trace provider (const for static initialization).
    ///
    /// Providers start disabled (enabled = 0).
    #[inline]
    pub const fn new(name: &'static str, id: u8) -> Self {
        Self {
            name,
            id,
            enabled: AtomicU64::new(0),
        }
    }

    /// Check if the provider has any probes enabled.
    ///
    /// This is an O(1) atomic load operation.
    #[inline(always)]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed) != 0
    }

    /// Check if a specific probe is enabled.
    ///
    /// # Parameters
    ///
    /// - `probe_id`: The probe ID within this provider (0-63)
    ///
    /// # Returns
    ///
    /// `true` if the probe is enabled, `false` otherwise.
    #[inline(always)]
    pub fn is_probe_enabled(&self, probe_id: u8) -> bool {
        if probe_id >= 64 {
            return false;
        }
        let mask = 1u64 << probe_id;
        (self.enabled.load(Ordering::Relaxed) & mask) != 0
    }

    /// Enable all probes in this provider.
    #[inline]
    pub fn enable_all(&self) {
        self.enabled.store(u64::MAX, Ordering::Release);
    }

    /// Disable all probes in this provider.
    #[inline]
    pub fn disable_all(&self) {
        self.enabled.store(0, Ordering::Release);
    }

    /// Enable a specific probe.
    ///
    /// # Parameters
    ///
    /// - `probe_id`: The probe ID within this provider (0-63)
    #[inline]
    pub fn enable_probe(&self, probe_id: u8) {
        if probe_id < 64 {
            let mask = 1u64 << probe_id;
            self.enabled.fetch_or(mask, Ordering::Release);
        }
    }

    /// Disable a specific probe.
    ///
    /// # Parameters
    ///
    /// - `probe_id`: The probe ID within this provider (0-63)
    #[inline]
    pub fn disable_probe(&self, probe_id: u8) {
        if probe_id < 64 {
            let mask = 1u64 << probe_id;
            self.enabled.fetch_and(!mask, Ordering::Release);
        }
    }

    /// Enable multiple probes using a bitmask.
    ///
    /// # Parameters
    ///
    /// - `mask`: Bitmask where bit N enables probe N
    #[inline]
    pub fn enable_probes(&self, mask: u64) {
        self.enabled.fetch_or(mask, Ordering::Release);
    }

    /// Get the full event type for a probe within this provider.
    ///
    /// Event types are constructed as: (provider_id << 8) | probe_id
    ///
    /// # Parameters
    ///
    /// - `probe_id`: The probe ID within this provider (0-255)
    ///
    /// # Returns
    ///
    /// The full 16-bit event type.
    #[inline(always)]
    pub const fn event_type(&self, probe_id: u8) -> u16 {
        ((self.id as u16) << 8) | (probe_id as u16)
    }
}

// =============================================================================
// Probe Structure
// =============================================================================

/// A trace probe is a specific instrumentation point within a provider.
///
/// Probes are lightweight structures that simply define metadata about
/// a trace point. The actual tracing is done via `record_event()`.
#[derive(Clone, Copy)]
pub struct TraceProbe {
    /// Probe name (e.g., "entry", "exit", "clock_gettime").
    pub name: &'static str,

    /// Probe ID within the provider (0-255).
    /// Combined with provider ID to form the event type.
    pub id: u8,

    /// Full event type: (provider_id << 8) | probe_id.
    /// Pre-computed for efficiency.
    pub event_type: u16,
}

impl TraceProbe {
    /// Create a new trace probe.
    #[inline]
    pub const fn new(name: &'static str, id: u8, provider_id: u8) -> Self {
        Self {
            name,
            id,
            event_type: ((provider_id as u16) << 8) | (id as u16),
        }
    }
}

// =============================================================================
// Global Provider Registry
// =============================================================================

/// Maximum number of providers that can be registered.
pub const MAX_PROVIDERS: usize = 16;

/// Global registry of trace providers.
/// Providers register themselves statically; this array holds references.
///
/// GDB: `print TRACE_PROVIDERS`
#[no_mangle]
pub static mut TRACE_PROVIDERS: [Option<&'static TraceProvider>; MAX_PROVIDERS] = [None; MAX_PROVIDERS];

/// Number of registered providers.
#[no_mangle]
pub static TRACE_PROVIDER_COUNT: AtomicU64 = AtomicU64::new(0);

/// Register a provider in the global registry.
///
/// This is typically called during static initialization.
/// Returns the slot index if successful, or None if the registry is full.
///
/// # Safety
///
/// This function modifies a global static. It should only be called during
/// single-threaded initialization.
pub fn register_provider(provider: &'static TraceProvider) -> Option<usize> {
    let count = TRACE_PROVIDER_COUNT.load(Ordering::Acquire);
    if count as usize >= MAX_PROVIDERS {
        return None;
    }

    // Find an empty slot
    unsafe {
        let providers_ptr = core::ptr::addr_of_mut!(TRACE_PROVIDERS);
        for i in 0..MAX_PROVIDERS {
            if (*providers_ptr)[i].is_none() {
                (*providers_ptr)[i] = Some(provider);
                TRACE_PROVIDER_COUNT.fetch_add(1, Ordering::Release);
                return Some(i);
            }
        }
    }

    None
}

/// Get a provider by its ID.
///
/// # Parameters
///
/// - `provider_id`: The provider's unique ID
///
/// # Returns
///
/// A reference to the provider, or None if not found.
#[allow(dead_code)]
pub fn get_provider(provider_id: u8) -> Option<&'static TraceProvider> {
    unsafe {
        let providers_ptr = core::ptr::addr_of!(TRACE_PROVIDERS);
        for i in 0..MAX_PROVIDERS {
            if let Some(provider) = (*providers_ptr)[i] {
                if provider.id == provider_id {
                    return Some(provider);
                }
            }
        }
    }
    None
}

/// Enable all probes for a provider by ID.
#[allow(dead_code)]
pub fn enable_provider_by_id(provider_id: u8) {
    if let Some(provider) = get_provider(provider_id) {
        provider.enable_all();
    }
}

/// Disable all probes for a provider by ID.
#[allow(dead_code)]
pub fn disable_provider_by_id(provider_id: u8) {
    if let Some(provider) = get_provider(provider_id) {
        provider.disable_all();
    }
}

/// Enable all registered providers.
#[allow(dead_code)]
pub fn enable_all_providers() {
    unsafe {
        let providers_ptr = core::ptr::addr_of!(TRACE_PROVIDERS);
        for i in 0..MAX_PROVIDERS {
            if let Some(provider) = (*providers_ptr)[i] {
                provider.enable_all();
            }
        }
    }
}

/// Disable all registered providers.
#[allow(dead_code)]
pub fn disable_all_providers() {
    unsafe {
        let providers_ptr = core::ptr::addr_of!(TRACE_PROVIDERS);
        for i in 0..MAX_PROVIDERS {
            if let Some(provider) = (*providers_ptr)[i] {
                provider.disable_all();
            }
        }
    }
}

// =============================================================================
// Combined Enable Check
// =============================================================================

/// Check if a specific event type is enabled.
///
/// This combines the global tracing enable check with the provider/probe check.
/// Designed for use in hot paths - returns early if tracing is globally disabled.
///
/// # Parameters
///
/// - `event_type`: The full 16-bit event type (provider_id << 8 | probe_id)
///
/// # Returns
///
/// `true` if the event should be recorded, `false` otherwise.
#[inline(always)]
pub fn is_event_enabled(event_type: u16) -> bool {
    // Fast path: check global enable first
    if !super::is_enabled() {
        return false;
    }

    // Extract provider ID and probe ID
    let provider_id = (event_type >> 8) as u8;
    let probe_id = (event_type & 0xFF) as u8;

    // Look up provider and check probe enable bit
    if let Some(provider) = get_provider(provider_id) {
        provider.is_probe_enabled(probe_id)
    } else {
        // Provider not registered - allow the event if global tracing is enabled
        // This supports ad-hoc events that don't use the provider system
        true
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Test provider - not registered globally to avoid interference
    static TEST_PROVIDER: TraceProvider = TraceProvider::new(0xFE, "test_provider");

    #[test_case]
    fn test_provider_new() {
        assert_eq!(TEST_PROVIDER.id, 0xFE);
        assert_eq!(TEST_PROVIDER.name, "test_provider");
        // Initially all probes are disabled
        assert_eq!(TEST_PROVIDER.enabled.load(Ordering::Relaxed), 0);
    }

    #[test_case]
    fn test_provider_enable_probe() {
        TEST_PROVIDER.disable_all(); // Reset state

        // Enable probe 0
        TEST_PROVIDER.enable_probe(0);
        assert!(TEST_PROVIDER.is_probe_enabled(0));
        assert!(!TEST_PROVIDER.is_probe_enabled(1));

        // Enable probe 5
        TEST_PROVIDER.enable_probe(5);
        assert!(TEST_PROVIDER.is_probe_enabled(0));
        assert!(TEST_PROVIDER.is_probe_enabled(5));
        assert!(!TEST_PROVIDER.is_probe_enabled(2));
    }

    #[test_case]
    fn test_provider_disable_probe() {
        TEST_PROVIDER.enable_all();

        // All probes should be enabled
        assert!(TEST_PROVIDER.is_probe_enabled(0));
        assert!(TEST_PROVIDER.is_probe_enabled(63));

        // Disable probe 10
        TEST_PROVIDER.disable_probe(10);
        assert!(TEST_PROVIDER.is_probe_enabled(0));
        assert!(!TEST_PROVIDER.is_probe_enabled(10));
        assert!(TEST_PROVIDER.is_probe_enabled(11));
    }

    #[test_case]
    fn test_provider_enable_all() {
        TEST_PROVIDER.disable_all();
        assert_eq!(TEST_PROVIDER.enabled.load(Ordering::Relaxed), 0);

        TEST_PROVIDER.enable_all();
        assert_eq!(TEST_PROVIDER.enabled.load(Ordering::Relaxed), u64::MAX);

        // All probes should be enabled
        for i in 0..64 {
            assert!(TEST_PROVIDER.is_probe_enabled(i));
        }
    }

    #[test_case]
    fn test_provider_disable_all() {
        TEST_PROVIDER.enable_all();
        assert_eq!(TEST_PROVIDER.enabled.load(Ordering::Relaxed), u64::MAX);

        TEST_PROVIDER.disable_all();
        assert_eq!(TEST_PROVIDER.enabled.load(Ordering::Relaxed), 0);

        // All probes should be disabled
        for i in 0..64 {
            assert!(!TEST_PROVIDER.is_probe_enabled(i));
        }
    }

    #[test_case]
    fn test_probe_id_overflow() {
        TEST_PROVIDER.disable_all();

        // Probe IDs >= 64 should wrap (modulo 64)
        TEST_PROVIDER.enable_probe(64); // Same as probe 0
        assert!(TEST_PROVIDER.is_probe_enabled(0));

        TEST_PROVIDER.enable_probe(65); // Same as probe 1
        assert!(TEST_PROVIDER.is_probe_enabled(1));
    }

    #[test_case]
    fn test_trace_probe_struct() {
        let probe = TraceProbe::new(0x0300, "SYSCALL_ENTRY");
        assert_eq!(probe.id, 0x0300);
        assert_eq!(probe.name, "SYSCALL_ENTRY");
        assert_eq!(probe.provider_id(), 0x03);
        assert_eq!(probe.probe_index(), 0x00);
    }

    #[test_case]
    fn test_trace_probe_provider_extraction() {
        // Test various event type values
        let probe1 = TraceProbe::new(0x0102, "test1");
        assert_eq!(probe1.provider_id(), 0x01);
        assert_eq!(probe1.probe_index(), 0x02);

        let probe2 = TraceProbe::new(0xFF3F, "test2");
        assert_eq!(probe2.provider_id(), 0xFF);
        assert_eq!(probe2.probe_index(), 0x3F);
    }
}
