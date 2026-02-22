//! Trace output and visualization module.
//!
//! This module provides functions for dumping trace buffers, counters, and events
//! to serial output for post-mortem analysis. It is designed to work safely in
//! panic contexts and is optimized for GDB inspection.
//!
//! # Design Principles
//!
//! 1. **Lock-free output**: Uses raw serial writes to avoid deadlocks in panic
//! 2. **Parseable format**: Output is designed for grep/awk processing
//! 3. **GDB-friendly**: Provides helper functions and documentation for GDB use
//! 4. **Compact output**: Minimizes serial bandwidth while remaining readable
//!
//! # Output Format
//!
//! Trace events are output in a line-oriented format:
//!
//! ```text
//! [TRACE] CPU0 idx=42 ts=1234567890 type=0x0300 SYSCALL_ENTRY syscall=1
//! [TRACE] CPU0 idx=43 ts=1234567900 type=0x0301 SYSCALL_EXIT result=0
//! [COUNTER] SYSCALL_TOTAL: 12345
//! [COUNTER] TIMER_TICK_TOTAL: 98765
//! ```
//!
//! # GDB Inspection
//!
//! ## Quick Start
//!
//! ```gdb
//! # Check if tracing is enabled
//! print TRACE_ENABLED
//!
//! # View CPU 0 write index (how many events recorded)
//! print TRACE_CPU0_WRITE_IDX
//!
//! # Call dump function (requires stopped execution)
//! call trace_dump()
//! ```
//!
//! ## Inspecting Raw Buffers
//!
//! ```gdb
//! # Get pointer to CPU 0's buffer
//! set $buf = &TRACE_BUFFERS[0]
//!
//! # View the write index
//! print $buf.write_idx
//!
//! # View entry 0 (16 bytes: timestamp, event_type, cpu_id, flags, payload)
//! x/2xg &($buf.entries[0])
//!
//! # Decode an entry manually:
//! # First 8 bytes = timestamp
//! # Bytes 8-9 = event_type (u16)
//! # Byte 10 = cpu_id
//! # Byte 11 = flags
//! # Bytes 12-15 = payload (u32)
//!
//! # View 10 entries starting from index 0
//! x/20xg &($buf.entries[0])
//!
//! # View most recent entry (if write_idx > 0)
//! set $last_idx = ($buf.write_idx.v.value - 1) & 0x3FF
//! x/2xg &($buf.entries[$last_idx])
//! ```
//!
//! ## Inspecting Counters
//!
//! ```gdb
//! # View syscall counter
//! print SYSCALL_TOTAL
//!
//! # View per-CPU values
//! print SYSCALL_TOTAL.per_cpu[0].value
//! print SYSCALL_TOTAL.per_cpu[1].value
//!
//! # Sum all CPUs manually (for 4 CPUs)
//! print SYSCALL_TOTAL.per_cpu[0].value.v.value + \
//!       SYSCALL_TOTAL.per_cpu[1].value.v.value + \
//!       SYSCALL_TOTAL.per_cpu[2].value.v.value + \
//!       SYSCALL_TOTAL.per_cpu[3].value.v.value
//! ```
//!
//! ## Inspecting Providers
//!
//! ```gdb
//! # Check if syscall provider is enabled
//! print SYSCALL_PROVIDER.enabled
//!
//! # View all providers
//! print TRACE_PROVIDERS
//!
//! # Enable a provider from GDB
//! set SYSCALL_PROVIDER.enabled.v.value = 0xFFFFFFFFFFFFFFFF
//! ```
//!
//! # Example Analysis Session
//!
//! 1. Start GDB session and hit a breakpoint or panic
//! 2. Check trace state: `print TRACE_ENABLED`, `print TRACE_CPU0_WRITE_IDX`
//! 3. If events exist, examine the last few:
//!    ```gdb
//!    set $idx = (TRACE_CPU0_WRITE_IDX.v.value - 5) & 0x3FF
//!    x/10xg &(TRACE_BUFFERS[0].entries[$idx])
//!    ```
//! 4. Decode event types using the constants in TraceEventType
//! 5. For detailed analysis, use `call trace_dump()` if the kernel is stopped

use super::buffer::TRACE_BUFFER_SIZE;
use super::core::{TraceEvent, TraceEventType, MAX_CPUS, TRACE_BUFFERS, TRACE_ENABLED};
use super::counter::{list_counters, CounterIterator, TRACE_COUNTERS, TRACE_COUNTER_COUNT};
use super::provider::{TRACE_PROVIDERS, TRACE_PROVIDER_COUNT};
use super::timestamp::timestamp_frequency_hz;
use core::sync::atomic::Ordering;

// =============================================================================
// GDB Helper Symbols
// =============================================================================

/// GDB-visible flag indicating a trace dump is in progress.
/// Set to 1 during dump, 0 otherwise.
///
/// GDB: `print TRACE_DUMP_IN_PROGRESS`
#[no_mangle]
pub static mut TRACE_DUMP_IN_PROGRESS: u64 = 0;

/// GDB-visible pointer to the most recently dumped event.
/// Updated during dump for inspection.
///
/// GDB: `print *TRACE_LAST_DUMPED_EVENT`
#[no_mangle]
pub static mut TRACE_LAST_DUMPED_EVENT: *const TraceEvent = core::ptr::null();

/// GDB-visible counter of total events dumped.
///
/// GDB: `print TRACE_DUMP_COUNT`
#[no_mangle]
pub static mut TRACE_DUMP_COUNT: u64 = 0;

// =============================================================================
// Raw Serial Output (Lock-Free)
// =============================================================================

/// Write a single character to serial output without any locks.
///
/// This is safe to call from panic handlers and interrupt contexts.
/// Uses direct port I/O on x86-64, UART on ARM64.
#[inline(always)]
fn raw_serial_char(c: u8) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use x86_64::instructions::port::Port;
        let mut port: Port<u8> = Port::new(0x3F8); // COM1 data port
        port.write(c);
    }

    #[cfg(target_arch = "aarch64")]
    unsafe {
        let uart_addr = crate::platform_config::uart_virt() as *mut u8;
        core::ptr::write_volatile(uart_addr, c);
    }
}

/// Write a string to serial output without any locks.
#[inline(never)]
fn raw_serial_str(s: &str) {
    for c in s.bytes() {
        raw_serial_char(c);
    }
}

/// Write a newline to serial output.
#[inline(always)]
fn raw_serial_newline() {
    raw_serial_char(b'\r');
    raw_serial_char(b'\n');
}

/// Write a u64 value in hexadecimal to serial output.
fn raw_serial_hex(value: u64) {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    raw_serial_str("0x");

    // Handle zero specially
    if value == 0 {
        raw_serial_char(b'0');
        return;
    }

    // Find the highest non-zero nibble
    let mut started = false;
    for i in (0..16).rev() {
        let nibble = ((value >> (i * 4)) & 0xF) as usize;
        if nibble != 0 || started {
            raw_serial_char(HEX_CHARS[nibble]);
            started = true;
        }
    }
}

/// Write a u64 value in decimal to serial output.
fn raw_serial_dec(mut value: u64) {
    if value == 0 {
        raw_serial_char(b'0');
        return;
    }

    // Buffer for digits (max 20 digits for u64)
    let mut buf = [0u8; 20];
    let mut i = 20;

    while value > 0 {
        i -= 1;
        buf[i] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    for j in i..20 {
        raw_serial_char(buf[j]);
    }
}

/// Write a u16 value in hexadecimal with 4 digits (zero-padded).
fn raw_serial_hex16(value: u16) {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    raw_serial_str("0x");
    raw_serial_char(HEX_CHARS[((value >> 12) & 0xF) as usize]);
    raw_serial_char(HEX_CHARS[((value >> 8) & 0xF) as usize]);
    raw_serial_char(HEX_CHARS[((value >> 4) & 0xF) as usize]);
    raw_serial_char(HEX_CHARS[(value & 0xF) as usize]);
}

// =============================================================================
// Event Type Names
// =============================================================================

/// Convert an event type to a human-readable name.
///
/// Returns a static string slice describing the event type.
/// Unknown event types return "UNKNOWN".
///
/// # Parameters
///
/// - `event_type`: The 16-bit event type code
///
/// # Returns
///
/// A static string describing the event type.
#[inline]
pub fn event_type_name(event_type: u16) -> &'static str {
    match event_type {
        // Context switch events (0x00xx)
        TraceEventType::CTX_SWITCH_ENTRY => "CTX_SWITCH_ENTRY",
        TraceEventType::CTX_SWITCH_EXIT => "CTX_SWITCH_EXIT",
        TraceEventType::CTX_SWITCH_TO_USER => "CTX_SWITCH_TO_USER",
        TraceEventType::CTX_SWITCH_TO_KERNEL => "CTX_SWITCH_TO_KERNEL",
        TraceEventType::CTX_SWITCH_TO_IDLE => "CTX_SWITCH_TO_IDLE",

        // Interrupt events (0x01xx)
        TraceEventType::IRQ_ENTRY => "IRQ_ENTRY",
        TraceEventType::IRQ_EXIT => "IRQ_EXIT",
        TraceEventType::TIMER_TICK => "TIMER_TICK",

        // Scheduler events (0x02xx)
        TraceEventType::SCHED_PICK => "SCHED_PICK",
        TraceEventType::SCHED_RESCHED => "SCHED_RESCHED",
        TraceEventType::SCHED_PREEMPT => "SCHED_PREEMPT",

        // Syscall events (0x03xx)
        TraceEventType::SYSCALL_ENTRY => "SYSCALL_ENTRY",
        TraceEventType::SYSCALL_EXIT => "SYSCALL_EXIT",

        // Memory events (0x04xx)
        TraceEventType::PAGE_FAULT => "PAGE_FAULT",
        TraceEventType::TLB_FLUSH => "TLB_FLUSH",
        TraceEventType::CR3_SWITCH => "CR3_SWITCH",

        // Lock events (0x05xx)
        TraceEventType::LOCK_ACQUIRE => "LOCK_ACQUIRE",
        TraceEventType::LOCK_RELEASE => "LOCK_RELEASE",
        TraceEventType::LOCK_CONTEND => "LOCK_CONTEND",

        // Process events (0x06xx)
        TraceEventType::FORK_ENTRY => "FORK_ENTRY",
        TraceEventType::FORK_EXIT => "FORK_EXIT",
        TraceEventType::EXEC_ENTRY => "EXEC_ENTRY",
        TraceEventType::EXEC_EXIT => "EXEC_EXIT",
        TraceEventType::COW_FAULT => "COW_FAULT",
        TraceEventType::COW_COPY => "COW_COPY",
        TraceEventType::STACK_MAP => "STACK_MAP",
        TraceEventType::DATA_ABORT => "DATA_ABORT",
        TraceEventType::PROCESS_EXIT => "PROCESS_EXIT",
        TraceEventType::COW_LOCK_FAIL => "COW_LOCK_FAIL",

        // Debug markers (0xFFxx)
        TraceEventType::MARKER_A => "MARKER_A",
        TraceEventType::MARKER_B => "MARKER_B",
        TraceEventType::MARKER_C => "MARKER_C",
        TraceEventType::MARKER_D => "MARKER_D",

        _ => "UNKNOWN",
    }
}

/// Get the payload description for an event type.
///
/// Returns a hint about what the payload field contains.
fn payload_description(event_type: u16) -> &'static str {
    match event_type {
        TraceEventType::CTX_SWITCH_ENTRY => "old_tid<<16|new_tid",
        TraceEventType::CTX_SWITCH_EXIT => "new_tid",
        TraceEventType::CTX_SWITCH_TO_USER => "tid",
        TraceEventType::CTX_SWITCH_TO_KERNEL => "tid",
        TraceEventType::CTX_SWITCH_TO_IDLE => "0",
        TraceEventType::IRQ_ENTRY => "vector",
        TraceEventType::IRQ_EXIT => "vector",
        TraceEventType::TIMER_TICK => "tick_count",
        TraceEventType::SCHED_PICK => "tid",
        TraceEventType::SCHED_RESCHED => "0",
        TraceEventType::SCHED_PREEMPT => "0",
        TraceEventType::SYSCALL_ENTRY => "syscall_nr",
        TraceEventType::SYSCALL_EXIT => "result",
        TraceEventType::PAGE_FAULT => "error_code",
        TraceEventType::TLB_FLUSH => "0",
        TraceEventType::CR3_SWITCH => "0",
        TraceEventType::LOCK_ACQUIRE => "lock_id",
        TraceEventType::LOCK_RELEASE => "lock_id",
        TraceEventType::LOCK_CONTEND => "lock_id",
        TraceEventType::FORK_ENTRY => "parent_pid",
        TraceEventType::FORK_EXIT => "child_pid",
        TraceEventType::EXEC_ENTRY => "pid",
        TraceEventType::EXEC_EXIT => "pid",
        TraceEventType::COW_FAULT => "pid<<16|page_idx",
        TraceEventType::COW_COPY => "pid<<16|page_idx",
        TraceEventType::STACK_MAP => "pid",
        TraceEventType::DATA_ABORT => "pid<<16|dfsc",
        TraceEventType::PROCESS_EXIT => "pid<<16|exit_code",
        TraceEventType::COW_LOCK_FAIL => "pid",
        TraceEventType::MARKER_A => "user_data",
        TraceEventType::MARKER_B => "user_data",
        TraceEventType::MARKER_C => "user_data",
        TraceEventType::MARKER_D => "user_data",
        _ => "payload",
    }
}

// =============================================================================
// Event Formatting
// =============================================================================

/// Format a trace event to serial output.
///
/// Output format:
/// ```text
/// [TRACE] CPU0 idx=42 ts=1234567890 type=0x0300 SYSCALL_ENTRY syscall_nr=1
/// ```
///
/// # Parameters
///
/// - `event`: The trace event to format
/// - `index`: The index in the ring buffer
fn format_event_to_serial(event: &TraceEvent, index: usize) {
    raw_serial_str("[TRACE] CPU");
    raw_serial_dec(event.cpu_id as u64);
    raw_serial_str(" idx=");
    raw_serial_dec(index as u64);
    raw_serial_str(" ts=");
    raw_serial_dec(event.timestamp);
    raw_serial_str(" type=");
    raw_serial_hex16(event.event_type);
    raw_serial_char(b' ');
    raw_serial_str(event_type_name(event.event_type));
    raw_serial_char(b' ');
    raw_serial_str(payload_description(event.event_type));
    raw_serial_char(b'=');
    raw_serial_dec(event.payload as u64);
    if event.flags != 0 {
        raw_serial_str(" flags=");
        raw_serial_hex(event.flags as u64);
    }
    raw_serial_newline();
}

// =============================================================================
// Buffer Dump Functions
// =============================================================================

/// Dump a single CPU's trace buffer to serial output.
///
/// This function is lock-free and safe to call from panic handlers.
/// Events are output in chronological order (oldest to newest).
///
/// # Parameters
///
/// - `cpu_id`: The CPU ID (0 to MAX_CPUS-1)
///
/// # Safety
///
/// This function accesses static mutable data (TRACE_BUFFERS).
/// It should only be called when tracing is disabled or during panic.
pub fn dump_buffer(cpu_id: usize) {
    if cpu_id >= MAX_CPUS {
        raw_serial_str("[TRACE] Invalid CPU ID: ");
        raw_serial_dec(cpu_id as u64);
        raw_serial_newline();
        return;
    }

    // Access the buffer
    let buffer = unsafe {
        let buffers_ptr = core::ptr::addr_of!(TRACE_BUFFERS);
        &(*buffers_ptr)[cpu_id]
    };

    let write_idx = buffer.write_index();
    let count = core::cmp::min(write_idx, TRACE_BUFFER_SIZE);
    let dropped = buffer.dropped_count();

    // Header
    raw_serial_str("[TRACE] === CPU ");
    raw_serial_dec(cpu_id as u64);
    raw_serial_str(" Buffer ===");
    raw_serial_newline();
    raw_serial_str("[TRACE] Events: ");
    raw_serial_dec(count as u64);
    raw_serial_str(" (write_idx=");
    raw_serial_dec(write_idx as u64);
    raw_serial_str(", dropped=");
    raw_serial_dec(dropped);
    raw_serial_char(b')');
    raw_serial_newline();

    if count == 0 {
        raw_serial_str("[TRACE] (empty)");
        raw_serial_newline();
        return;
    }

    // Calculate start index for chronological order
    let start = if write_idx > TRACE_BUFFER_SIZE {
        write_idx & (TRACE_BUFFER_SIZE - 1)
    } else {
        0
    };

    // Dump events in chronological order
    unsafe {
        let dump_progress_ptr = core::ptr::addr_of_mut!(TRACE_DUMP_IN_PROGRESS);
        let dump_count_ptr = core::ptr::addr_of_mut!(TRACE_DUMP_COUNT);
        let last_event_ptr = core::ptr::addr_of_mut!(TRACE_LAST_DUMPED_EVENT);

        *dump_progress_ptr = 1;
        *dump_count_ptr = 0;

        for i in 0..count {
            let idx = (start + i) & (TRACE_BUFFER_SIZE - 1);
            if let Some(event) = buffer.get_event(idx) {
                format_event_to_serial(event, idx);
                *last_event_ptr = event as *const TraceEvent;
                *dump_count_ptr += 1;
            }
        }

        *dump_progress_ptr = 0;
    }
}

/// Dump all CPU trace buffers to serial output.
///
/// This function iterates over all CPU buffers and dumps them in order.
/// It is lock-free and safe to call from panic handlers.
pub fn dump_all_buffers() {
    raw_serial_str("[TRACE] ====== TRACE BUFFER DUMP ======");
    raw_serial_newline();
    raw_serial_str("[TRACE] Tracing enabled: ");
    raw_serial_str(if TRACE_ENABLED.load(Ordering::Relaxed) != 0 {
        "yes"
    } else {
        "no"
    });
    raw_serial_newline();

    // Output timestamp frequency for offline conversion
    let freq = timestamp_frequency_hz();
    if freq != 0 {
        raw_serial_str("[TRACE] Timestamp frequency: ");
        raw_serial_dec(freq);
        raw_serial_str(" Hz");
        raw_serial_newline();
    }

    for cpu in 0..MAX_CPUS {
        dump_buffer(cpu);
    }

    raw_serial_str("[TRACE] ====== END TRACE DUMP ======");
    raw_serial_newline();
}

/// Dump the most recent N events across all CPUs.
///
/// Events are merged by timestamp to show a unified view.
/// This is useful for seeing what happened immediately before a panic.
///
/// # Parameters
///
/// - `n`: Maximum number of events to dump (across all CPUs)
pub fn dump_latest_events(n: usize) {
    raw_serial_str("[TRACE] === Latest ");
    raw_serial_dec(n as u64);
    raw_serial_str(" Events ===");
    raw_serial_newline();

    // Collect the most recent event from each CPU
    // We use a simple O(n*CPUs) algorithm since n is typically small
    let mut remaining = n;

    // For each iteration, find the most recent event we haven't output yet
    // Track the last timestamp we output to avoid duplicates
    let mut max_ts_seen = u64::MAX;

    while remaining > 0 {
        let mut best_event: Option<(&TraceEvent, usize, usize)> = None; // (event, cpu, idx)
        let mut best_ts = 0u64;

        // Find the most recent event (highest timestamp) that's less than max_ts_seen
        for cpu in 0..MAX_CPUS {
            let buffer = unsafe {
                let buffers_ptr = core::ptr::addr_of!(TRACE_BUFFERS);
                &(*buffers_ptr)[cpu]
            };

            let write_idx = buffer.write_index();
            if write_idx == 0 {
                continue;
            }

            let count = core::cmp::min(write_idx, TRACE_BUFFER_SIZE);
            let start = if write_idx > TRACE_BUFFER_SIZE {
                write_idx & (TRACE_BUFFER_SIZE - 1)
            } else {
                0
            };

            // Scan this CPU's buffer for the best candidate
            for i in 0..count {
                let idx = (start + i) & (TRACE_BUFFER_SIZE - 1);
                if let Some(event) = buffer.get_event(idx) {
                    if event.timestamp < max_ts_seen && event.timestamp > best_ts {
                        best_ts = event.timestamp;
                        best_event = Some((event, cpu, idx));
                    }
                }
            }
        }

        match best_event {
            Some((event, _cpu, idx)) => {
                format_event_to_serial(event, idx);
                max_ts_seen = event.timestamp;
                remaining -= 1;
            }
            None => break, // No more events
        }
    }

    raw_serial_str("[TRACE] === End Latest Events ===");
    raw_serial_newline();
}

// =============================================================================
// Counter Dump Functions
// =============================================================================

/// Dump all counter values to serial output.
///
/// Output format:
/// ```text
/// [COUNTER] SYSCALL_TOTAL: 12345
/// [COUNTER] IRQ_TOTAL: 98765
/// ```
pub fn dump_counters() {
    raw_serial_str("[COUNTER] ====== COUNTER DUMP ======");
    raw_serial_newline();

    let count = TRACE_COUNTER_COUNT.load(Ordering::Relaxed);
    raw_serial_str("[COUNTER] Registered counters: ");
    raw_serial_dec(count);
    raw_serial_newline();

    for counter in list_counters() {
        raw_serial_str("[COUNTER] ");
        raw_serial_str(counter.name);
        raw_serial_str(": ");
        raw_serial_dec(counter.aggregate());

        // Show per-CPU breakdown for non-zero values
        let mut has_percpu = false;
        for cpu in 0..MAX_CPUS {
            let val = counter.get_cpu(cpu);
            if val > 0 {
                if !has_percpu {
                    raw_serial_str(" (");
                    has_percpu = true;
                } else {
                    raw_serial_str(", ");
                }
                raw_serial_str("cpu");
                raw_serial_dec(cpu as u64);
                raw_serial_char(b'=');
                raw_serial_dec(val);
            }
        }
        if has_percpu {
            raw_serial_char(b')');
        }
        raw_serial_newline();
    }

    raw_serial_str("[COUNTER] ====== END COUNTERS ======");
    raw_serial_newline();
}

// =============================================================================
// Provider Dump Functions
// =============================================================================

/// Dump all registered providers and their enable state.
pub fn dump_providers() {
    raw_serial_str("[PROVIDER] ====== PROVIDER DUMP ======");
    raw_serial_newline();

    let count = TRACE_PROVIDER_COUNT.load(Ordering::Relaxed);
    raw_serial_str("[PROVIDER] Registered providers: ");
    raw_serial_dec(count);
    raw_serial_newline();

    unsafe {
        let providers_ptr = core::ptr::addr_of!(TRACE_PROVIDERS);
        for i in 0..super::provider::MAX_PROVIDERS {
            if let Some(provider) = (*providers_ptr)[i] {
                raw_serial_str("[PROVIDER] ");
                raw_serial_str(provider.name);
                raw_serial_str(" (id=");
                raw_serial_hex(provider.id as u64);
                raw_serial_str("): enabled=");
                raw_serial_hex(provider.enabled.load(Ordering::Relaxed));
                raw_serial_newline();
            }
        }
    }

    raw_serial_str("[PROVIDER] ====== END PROVIDERS ======");
    raw_serial_newline();
}

// =============================================================================
// Event Analysis Functions
// =============================================================================

/// Count events by type across all CPUs.
///
/// Returns an array of counts indexed by event type category (upper byte).
/// For example, index 0x03 contains the count of all syscall events (0x03xx).
///
/// # Returns
///
/// An array of 256 u64 values, where index i is the count of events
/// with event_type in the range [i<<8, (i+1)<<8).
pub fn count_events_by_category() -> [u64; 256] {
    let mut counts = [0u64; 256];

    for cpu in 0..MAX_CPUS {
        let buffer = unsafe {
            let buffers_ptr = core::ptr::addr_of!(TRACE_BUFFERS);
            &(*buffers_ptr)[cpu]
        };

        let write_idx = buffer.write_index();
        let count = core::cmp::min(write_idx, TRACE_BUFFER_SIZE);
        let start = if write_idx > TRACE_BUFFER_SIZE {
            write_idx & (TRACE_BUFFER_SIZE - 1)
        } else {
            0
        };

        for i in 0..count {
            let idx = (start + i) & (TRACE_BUFFER_SIZE - 1);
            if let Some(event) = buffer.get_event(idx) {
                let category = (event.event_type >> 8) as usize;
                counts[category] = counts[category].saturating_add(1);
            }
        }
    }

    counts
}

/// Count events in a timestamp range across all CPUs.
///
/// # Parameters
///
/// - `start_ts`: Start timestamp (inclusive)
/// - `end_ts`: End timestamp (inclusive)
///
/// # Returns
///
/// The number of events with timestamps in [start_ts, end_ts].
pub fn count_events_in_range(start_ts: u64, end_ts: u64) -> u64 {
    let mut count = 0u64;

    for cpu in 0..MAX_CPUS {
        let buffer = unsafe {
            let buffers_ptr = core::ptr::addr_of!(TRACE_BUFFERS);
            &(*buffers_ptr)[cpu]
        };

        for event in buffer.iter_events() {
            if event.timestamp >= start_ts && event.timestamp <= end_ts {
                count = count.saturating_add(1);
            }
        }
    }

    count
}

/// Print a summary of event counts by category.
pub fn dump_event_summary() {
    raw_serial_str("[SUMMARY] ====== EVENT SUMMARY ======");
    raw_serial_newline();

    let counts = count_events_by_category();

    // Only print non-zero categories
    let categories = [
        (0x00, "Context Switch"),
        (0x01, "Interrupt"),
        (0x02, "Scheduler"),
        (0x03, "Syscall"),
        (0x04, "Memory"),
        (0x05, "Lock"),
        (0x06, "Process"),
        (0xFF, "Marker/Debug"),
    ];

    for (cat, name) in categories.iter() {
        let count = counts[*cat as usize];
        if count > 0 {
            raw_serial_str("[SUMMARY] ");
            raw_serial_str(name);
            raw_serial_str(": ");
            raw_serial_dec(count);
            raw_serial_newline();
        }
    }

    // Total events
    let total: u64 = counts.iter().sum();
    raw_serial_str("[SUMMARY] Total events: ");
    raw_serial_dec(total);
    raw_serial_newline();

    raw_serial_str("[SUMMARY] ====== END SUMMARY ======");
    raw_serial_newline();
}

// =============================================================================
// GDB Callable Functions
// =============================================================================

/// Dump all trace data (buffers, counters, providers).
///
/// This function is designed to be called from GDB:
/// ```gdb
/// call trace_dump()
/// ```
///
/// It outputs a complete trace dump to serial, which can be captured
/// in the serial log file.
#[no_mangle]
pub extern "C" fn trace_dump() {
    dump_all_buffers();
    dump_counters();
    dump_providers();
    dump_event_summary();
}

/// Dump just the latest N events (GDB callable).
///
/// ```gdb
/// call trace_dump_latest(20)
/// ```
#[no_mangle]
pub extern "C" fn trace_dump_latest(n: usize) {
    dump_latest_events(n);
}

/// Dump a specific CPU's buffer (GDB callable).
///
/// ```gdb
/// call trace_dump_cpu(0)
/// ```
#[no_mangle]
pub extern "C" fn trace_dump_cpu(cpu_id: usize) {
    dump_buffer(cpu_id);
}

/// Dump all counters (GDB callable).
///
/// ```gdb
/// call trace_dump_counters()
/// ```
#[no_mangle]
pub extern "C" fn trace_dump_counters() {
    dump_counters();
}

// =============================================================================
// Panic Handler Integration
// =============================================================================

/// Dump trace data during a panic.
///
/// This function should be called from the panic handler to capture
/// trace state before the system halts. It outputs a condensed view
/// showing the most recent events and counter values.
///
/// # Safety
///
/// This function uses lock-free serial output and is safe to call
/// from panic handlers. However, trace data may be inconsistent
/// if the panic occurred during trace recording.
pub fn dump_on_panic() {
    raw_serial_newline();
    raw_serial_str("====== PANIC TRACE DUMP ======");
    raw_serial_newline();

    // Disable further tracing to prevent race conditions
    super::disable();

    // Dump the last 32 events across all CPUs
    dump_latest_events(32);

    // Dump counter summary
    dump_counters();

    // Dump provider state
    dump_providers();

    raw_serial_str("====== END PANIC DUMP ======");
    raw_serial_newline();
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_event_type_name() {
        assert_eq!(event_type_name(TraceEventType::SYSCALL_ENTRY), "SYSCALL_ENTRY");
        assert_eq!(event_type_name(TraceEventType::SYSCALL_EXIT), "SYSCALL_EXIT");
        assert_eq!(event_type_name(TraceEventType::TIMER_TICK), "TIMER_TICK");
        assert_eq!(event_type_name(0xFFFF), "UNKNOWN");
    }

    #[test_case]
    fn test_payload_description() {
        assert_eq!(payload_description(TraceEventType::SYSCALL_ENTRY), "syscall_nr");
        assert_eq!(payload_description(TraceEventType::CTX_SWITCH_ENTRY), "old_tid<<16|new_tid");
    }
}
