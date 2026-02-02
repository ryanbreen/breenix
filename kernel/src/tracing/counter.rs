//! Atomic trace counters for kernel statistics.
//!
//! This module provides per-CPU atomic counters optimized for high-frequency
//! increment operations. Counters use lock-free atomics with relaxed ordering
//! for minimal overhead.
//!
//! # Design Principles
//!
//! 1. **Per-CPU storage**: Eliminates contention between CPUs
//! 2. **Lock-free operations**: Safe in any context (interrupts, syscalls, etc.)
//! 3. **Relaxed ordering**: Maximum performance, counters are approximate
//! 4. **GDB-inspectable**: Static symbols visible via GDB
//! 5. **Zero overhead when unused**: No cost if counter is never incremented
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::counter::{define_trace_counter, trace_count};
//!
//! // Define a counter (static)
//! define_trace_counter!(SYSCALL_TOTAL, "Total syscall invocations");
//!
//! // Increment the counter
//! fn handle_syscall() {
//!     trace_count!(SYSCALL_TOTAL);
//!     // ... handle syscall ...
//! }
//!
//! // Query the counter
//! let total = SYSCALL_TOTAL.aggregate();
//! ```
//!
//! # GDB Inspection
//!
//! ```gdb
//! # View counter name and per-CPU values
//! print SYSCALL_TOTAL_COUNTER
//!
//! # View specific per-CPU value
//! print SYSCALL_TOTAL_COUNTER.per_cpu[0]
//! ```

use core::sync::atomic::{AtomicU64, Ordering};

use super::core::MAX_CPUS;

// =============================================================================
// Trace Counter Structure
// =============================================================================

/// A per-CPU atomic counter for tracing statistics.
///
/// Each counter maintains separate values for each CPU to avoid contention.
/// The `aggregate()` function sums all per-CPU values for the total count.
///
/// # Memory Layout
///
/// Per-CPU values are cache-line separated (64 bytes apart) to prevent
/// false sharing between CPUs.
#[repr(C)]
pub struct TraceCounter {
    /// Human-readable name for this counter.
    pub name: &'static str,

    /// Description of what this counter measures.
    pub description: &'static str,

    /// Per-CPU counter values.
    /// Each entry is cache-line padded in the CpuCounterSlot wrapper.
    pub per_cpu: [CpuCounterSlot; MAX_CPUS],
}

/// A cache-line aligned counter slot for a single CPU.
///
/// This structure ensures each CPU's counter is on its own cache line
/// to prevent false sharing.
#[repr(C, align(64))]
pub struct CpuCounterSlot {
    /// The counter value for this CPU.
    pub value: AtomicU64,
    /// Padding to fill the cache line.
    _padding: [u8; 56],
}

impl CpuCounterSlot {
    /// Create a new counter slot with value 0.
    pub const fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
            _padding: [0; 56],
        }
    }
}

impl Default for CpuCounterSlot {
    fn default() -> Self {
        Self::new()
    }
}

// Verify CpuCounterSlot is exactly 64 bytes (one cache line)
const _: () = assert!(
    core::mem::size_of::<CpuCounterSlot>() == 64,
    "CpuCounterSlot must be exactly 64 bytes (one cache line)"
);

impl TraceCounter {
    /// Create a new trace counter with the given name and description.
    ///
    /// All per-CPU values start at 0.
    pub const fn new(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            per_cpu: [const { CpuCounterSlot::new() }; MAX_CPUS],
        }
    }

    /// Increment the counter for the current CPU.
    ///
    /// This is a lock-free atomic add with relaxed ordering.
    /// Designed for hot paths - compiles to a single atomic instruction.
    #[inline(always)]
    pub fn increment(&self) {
        let cpu_id = current_cpu_id();
        if cpu_id < MAX_CPUS {
            self.per_cpu[cpu_id].value.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Increment the counter for the current CPU by a specific amount.
    ///
    /// # Parameters
    ///
    /// - `amount`: The value to add to the counter
    #[inline(always)]
    pub fn add(&self, amount: u64) {
        let cpu_id = current_cpu_id();
        if cpu_id < MAX_CPUS {
            self.per_cpu[cpu_id].value.fetch_add(amount, Ordering::Relaxed);
        }
    }

    /// Increment the counter for a specific CPU.
    ///
    /// Use this when the CPU ID is already known (e.g., in interrupt handlers).
    ///
    /// # Parameters
    ///
    /// - `cpu_id`: The CPU ID to increment the counter for
    #[inline(always)]
    pub fn increment_cpu(&self, cpu_id: usize) {
        if cpu_id < MAX_CPUS {
            self.per_cpu[cpu_id].value.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Get the aggregated value across all CPUs.
    ///
    /// This sums all per-CPU values. Note that this is a point-in-time
    /// snapshot and may not be perfectly consistent if counters are
    /// being incremented concurrently.
    #[inline]
    pub fn aggregate(&self) -> u64 {
        let mut total: u64 = 0;
        for i in 0..MAX_CPUS {
            total = total.wrapping_add(self.per_cpu[i].value.load(Ordering::Relaxed));
        }
        total
    }

    /// Get the value for a specific CPU.
    ///
    /// # Parameters
    ///
    /// - `cpu_id`: The CPU ID to get the value for
    ///
    /// # Returns
    ///
    /// The counter value for the specified CPU, or 0 if cpu_id is invalid.
    #[inline]
    pub fn get_cpu(&self, cpu_id: usize) -> u64 {
        if cpu_id < MAX_CPUS {
            self.per_cpu[cpu_id].value.load(Ordering::Relaxed)
        } else {
            0
        }
    }

    /// Reset the counter to 0 for all CPUs.
    ///
    /// This is not atomic across CPUs - some increments may be lost
    /// if called while the counter is being incremented.
    pub fn reset(&self) {
        for i in 0..MAX_CPUS {
            self.per_cpu[i].value.store(0, Ordering::Relaxed);
        }
    }

    /// Reset the counter for a specific CPU.
    ///
    /// # Parameters
    ///
    /// - `cpu_id`: The CPU ID to reset
    pub fn reset_cpu(&self, cpu_id: usize) {
        if cpu_id < MAX_CPUS {
            self.per_cpu[cpu_id].value.store(0, Ordering::Relaxed);
        }
    }
}

// =============================================================================
// CPU ID Helper
// =============================================================================

/// Get the current CPU ID.
///
/// This function uses architecture-specific mechanisms:
/// - x86-64: GS-relative access to per-CPU data
/// - ARM64: TPIDR_EL1 or MPIDR_EL1 fallback
#[inline(always)]
fn current_cpu_id() -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::arch_impl::current::percpu::X86PerCpu;
        use crate::arch_impl::PerCpuOps;
        X86PerCpu::cpu_id() as usize
    }

    #[cfg(target_arch = "aarch64")]
    {
        use crate::arch_impl::current::percpu::Aarch64PerCpu;
        use crate::arch_impl::PerCpuOps;
        Aarch64PerCpu::cpu_id() as usize
    }
}

// =============================================================================
// Counter Registry
// =============================================================================

/// Maximum number of counters that can be registered.
pub const MAX_COUNTERS: usize = 64;

/// Global registry of trace counters.
///
/// GDB: `print TRACE_COUNTERS`
#[no_mangle]
pub static mut TRACE_COUNTERS: [Option<&'static TraceCounter>; MAX_COUNTERS] = [None; MAX_COUNTERS];

/// Number of registered counters.
#[no_mangle]
pub static TRACE_COUNTER_COUNT: AtomicU64 = AtomicU64::new(0);

/// Register a counter in the global registry.
///
/// This is typically called during static initialization.
/// Returns the slot index if successful, or None if the registry is full.
///
/// # Safety
///
/// This function modifies a global static. It should only be called during
/// single-threaded initialization.
pub fn register_counter(counter: &'static TraceCounter) -> Option<usize> {
    let count = TRACE_COUNTER_COUNT.load(Ordering::Acquire);
    if count as usize >= MAX_COUNTERS {
        return None;
    }

    // Find an empty slot
    unsafe {
        let counters_ptr = core::ptr::addr_of_mut!(TRACE_COUNTERS);
        for i in 0..MAX_COUNTERS {
            if (*counters_ptr)[i].is_none() {
                (*counters_ptr)[i] = Some(counter);
                TRACE_COUNTER_COUNT.fetch_add(1, Ordering::Release);
                return Some(i);
            }
        }
    }

    None
}

/// Get a counter by name.
///
/// # Parameters
///
/// - `name`: The counter's name
///
/// # Returns
///
/// A reference to the counter, or None if not found.
pub fn get_counter(name: &str) -> Option<&'static TraceCounter> {
    unsafe {
        let counters_ptr = core::ptr::addr_of!(TRACE_COUNTERS);
        for i in 0..MAX_COUNTERS {
            if let Some(counter) = (*counters_ptr)[i] {
                if counter.name == name {
                    return Some(counter);
                }
            }
        }
    }
    None
}

/// Get the value of a counter by name.
///
/// # Parameters
///
/// - `name`: The counter's name
///
/// # Returns
///
/// The aggregated counter value, or 0 if the counter is not found.
pub fn get_counter_value(name: &str) -> u64 {
    get_counter(name).map(|c| c.aggregate()).unwrap_or(0)
}

/// Reset a counter by name.
///
/// # Parameters
///
/// - `name`: The counter's name
///
/// # Returns
///
/// `true` if the counter was found and reset, `false` otherwise.
pub fn reset_counter(name: &str) -> bool {
    if let Some(counter) = get_counter(name) {
        counter.reset();
        true
    } else {
        false
    }
}

/// Iterator over registered counters.
pub struct CounterIterator {
    index: usize,
}

impl CounterIterator {
    /// Create a new counter iterator.
    pub fn new() -> Self {
        Self { index: 0 }
    }
}

impl Default for CounterIterator {
    fn default() -> Self {
        Self::new()
    }
}

impl Iterator for CounterIterator {
    type Item = &'static TraceCounter;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            let counters_ptr = core::ptr::addr_of!(TRACE_COUNTERS);
            while self.index < MAX_COUNTERS {
                let current = self.index;
                self.index += 1;
                if let Some(counter) = (*counters_ptr)[current] {
                    return Some(counter);
                }
            }
        }
        None
    }
}

/// List all registered counters.
///
/// Returns an iterator over all registered counters.
pub fn list_counters() -> CounterIterator {
    CounterIterator::new()
}

/// Reset all registered counters.
pub fn reset_all_counters() {
    for counter in list_counters() {
        counter.reset();
    }
}

// =============================================================================
// Counter Summary
// =============================================================================

/// A snapshot of a counter's values.
#[derive(Clone, Copy, Debug)]
pub struct CounterSnapshot {
    /// Counter name.
    pub name: &'static str,
    /// Aggregated value across all CPUs.
    pub total: u64,
    /// Per-CPU values (only the first 8 CPUs for brevity).
    pub per_cpu: [u64; 8],
}

impl CounterSnapshot {
    /// Create a snapshot of a counter.
    pub fn from_counter(counter: &'static TraceCounter) -> Self {
        let mut per_cpu = [0u64; 8];
        for (i, slot) in per_cpu.iter_mut().enumerate() {
            *slot = counter.get_cpu(i);
        }
        Self {
            name: counter.name,
            total: counter.aggregate(),
            per_cpu,
        }
    }
}

/// Get snapshots of all registered counters.
///
/// Returns a vector-like structure with counter snapshots.
/// Limited to MAX_COUNTERS entries.
pub fn snapshot_all_counters() -> ([CounterSnapshot; MAX_COUNTERS], usize) {
    let mut snapshots = [CounterSnapshot {
        name: "",
        total: 0,
        per_cpu: [0; 8],
    }; MAX_COUNTERS];
    let mut count = 0;

    for counter in list_counters() {
        if count < MAX_COUNTERS {
            snapshots[count] = CounterSnapshot::from_counter(counter);
            count += 1;
        }
    }

    (snapshots, count)
}
