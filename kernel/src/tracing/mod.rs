//! DTrace-style lock-free tracing framework for kernel observability.
//!
//! This module provides a comprehensive, lock-free tracing system designed for
//! critical kernel paths including interrupt handlers, syscalls, and context switches.
//!
//! # Design Principles
//!
//! 1. **Lock-free operation**: Safe in interrupt handlers, context switches, and syscall paths
//! 2. **Minimal overhead**: Near-zero cost when disabled; minimal impact when enabled
//! 3. **Cross-architecture support**: Works on both x86-64 and ARM64
//! 4. **Per-CPU ring buffers**: Eliminates contention, enables lockless local writes
//! 5. **Provider/probe model**: Subsystems define their own trace points
//! 6. **GDB-first debugging**: Primary inspection via GDB, secondary via serial
//!
//! # Architecture
//!
//! ```text
//! +-------------------+     +-------------------+     +-------------------+
//! |  SYSCALL PROVIDER |     |   SCHED PROVIDER  |     |   IRQ PROVIDER    |
//! |  - sys_entry      |     |  - ctx_switch     |     |  - irq_entry      |
//! |  - sys_exit       |     |  - schedule       |     |  - timer_tick     |
//! +--------+----------+     +--------+----------+     +--------+----------+
//!          |                         |                         |
//!          v                         v                         v
//! +------------------------------------------------------------------------+
//! |                        TRACING CORE                                    |
//! |  Per-CPU ring buffers | Atomic enable bitmaps | Lock-free recording   |
//! +------------------------------------------------------------------------+
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use kernel::tracing::{trace_event, is_enabled};
//! use kernel::tracing::providers::SYSCALL_PROVIDER;
//! use kernel::tracing::providers::syscall::{SYSCALL_ENTRY, SYSCALL_EXIT};
//!
//! // Enable tracing globally and for syscalls
//! kernel::tracing::enable();
//! SYSCALL_PROVIDER.enable_all();
//!
//! // Record events (only if tracing is enabled)
//! trace_event!(SYSCALL_PROVIDER, SYSCALL_ENTRY, syscall_num as u32);
//! // ... handle syscall ...
//! trace_event!(SYSCALL_PROVIDER, SYSCALL_EXIT, result as u32);
//! ```
//!
//! # GDB Inspection
//!
//! ```gdb
//! # View raw trace buffer for CPU 0
//! x/1024xg &TRACE_CPU0_BUFFER
//!
//! # View write index
//! print TRACE_CPU0_WRITE_IDX
//!
//! # Check if tracing is enabled
//! print TRACE_ENABLED
//!
//! # Check provider enable state
//! print SYSCALL_PROVIDER.enabled
//!
//! # Dump all trace data to serial
//! call trace_dump()
//!
//! # Dump latest 20 events
//! call trace_dump_latest(20)
//!
//! # Dump counters only
//! call trace_dump_counters()
//! ```
//!
//! # Serial Output Format
//!
//! Trace events are output in a parseable format:
//!
//! ```text
//! [TRACE] CPU0 idx=42 ts=1234567890 type=0x0300 SYSCALL_ENTRY syscall_nr=1
//! [TRACE] CPU0 idx=43 ts=1234567900 type=0x0301 SYSCALL_EXIT result=0
//! [COUNTER] SYSCALL_TOTAL: 12345
//! [COUNTER] TIMER_TICK_TOTAL: 98765
//! ```

// Allow dead code for public APIs not yet integrated into subsystems
#![allow(dead_code)]
#![allow(unused_imports)]

mod buffer;
pub mod counter;
mod core;
pub mod macros;
pub mod output;
pub mod provider;
pub mod providers;
mod timestamp;

// Re-export public API from core
pub use self::buffer::{TraceCpuBuffer, TRACE_BUFFER_SIZE};
pub use self::core::{
    TraceEvent, TraceEventType, MAX_CPUS, TRACE_BUFFERS, TRACE_ENABLED, disable, enable, init,
    is_enabled, record_event, record_event_2,
};
pub use self::timestamp::trace_timestamp;

// Re-export provider types
pub use self::provider::{TraceProbe, TraceProvider};

// Re-export macros for convenient use
pub use self::macros::{
    define_trace_counter, define_trace_probes, define_trace_provider, trace_count, trace_count_add,
    trace_event, trace_event_2,
};

// Re-export output functions for convenience
pub use self::output::{
    dump_all_buffers, dump_buffer, dump_counters, dump_event_summary, dump_latest_events,
    dump_on_panic, dump_providers, event_type_name,
};

// GDB-inspectable symbols are defined in core.rs with #[no_mangle]

/// Initialize the complete tracing subsystem including providers.
///
/// This should be called once during kernel initialization, after per-CPU
/// data is set up.
pub fn init_full() {
    // Initialize core tracing infrastructure
    init();

    // Initialize built-in providers
    providers::init();

    log::info!("Full tracing subsystem initialized with providers");
}
