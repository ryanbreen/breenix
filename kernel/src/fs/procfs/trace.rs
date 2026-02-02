//! Trace-related procfs entries
//!
//! This module provides content generators for /proc/trace/* entries,
//! exposing the kernel tracing subsystem via procfs.
//!
//! # Entries
//!
//! - `/proc/trace/enable` - Shows tracing enable state (0 or 1)
//! - `/proc/trace/events` - Lists all available trace points
//! - `/proc/trace/buffer` - Dumps trace buffer contents
//! - `/proc/trace/counters` - Shows trace counter values
//! - `/proc/trace/providers` - Lists registered trace providers

use alloc::format;
use alloc::string::String;
use core::sync::atomic::Ordering;

use crate::tracing::{
    TraceEventType, MAX_CPUS, TRACE_BUFFER_SIZE, TRACE_BUFFERS, TRACE_ENABLED, event_type_name,
};
use crate::tracing::counter::{list_counters, TRACE_COUNTER_COUNT};
use crate::tracing::provider::{TRACE_PROVIDERS, TRACE_PROVIDER_COUNT};

/// Generate content for /proc/trace/enable
///
/// Returns "1\n" if tracing is enabled, "0\n" otherwise.
pub fn generate_enable() -> String {
    let enabled = TRACE_ENABLED.load(Ordering::Relaxed);
    if enabled != 0 {
        String::from("1\n")
    } else {
        String::from("0\n")
    }
}

/// Generate content for /proc/trace/events
///
/// Lists all available trace event types with their names.
/// Format: `event_type_hex event_name`
pub fn generate_events() -> String {
    let mut output = String::new();

    // List all known event types
    let event_types: &[(u16, &str)] = &[
        // Context switch events (0x00xx)
        (TraceEventType::CTX_SWITCH_ENTRY, "context switch entry"),
        (TraceEventType::CTX_SWITCH_EXIT, "context switch exit"),
        (TraceEventType::CTX_SWITCH_TO_USER, "switch to user mode"),
        (TraceEventType::CTX_SWITCH_TO_KERNEL, "switch to kernel mode"),
        (TraceEventType::CTX_SWITCH_TO_IDLE, "switch to idle"),
        // Interrupt events (0x01xx)
        (TraceEventType::IRQ_ENTRY, "interrupt entry"),
        (TraceEventType::IRQ_EXIT, "interrupt exit"),
        (TraceEventType::TIMER_TICK, "timer tick"),
        // Scheduler events (0x02xx)
        (TraceEventType::SCHED_PICK, "scheduler pick"),
        (TraceEventType::SCHED_RESCHED, "reschedule"),
        (TraceEventType::SCHED_PREEMPT, "preemption"),
        // Syscall events (0x03xx)
        (TraceEventType::SYSCALL_ENTRY, "syscall entry"),
        (TraceEventType::SYSCALL_EXIT, "syscall exit"),
        // Memory events (0x04xx)
        (TraceEventType::PAGE_FAULT, "page fault"),
        (TraceEventType::TLB_FLUSH, "TLB flush"),
        (TraceEventType::CR3_SWITCH, "CR3/page table switch"),
        // Lock events (0x05xx)
        (TraceEventType::LOCK_ACQUIRE, "lock acquire"),
        (TraceEventType::LOCK_RELEASE, "lock release"),
        (TraceEventType::LOCK_CONTEND, "lock contention"),
        // Debug markers (0xFFxx)
        (TraceEventType::MARKER_A, "debug marker A"),
        (TraceEventType::MARKER_B, "debug marker B"),
        (TraceEventType::MARKER_C, "debug marker C"),
        (TraceEventType::MARKER_D, "debug marker D"),
    ];

    for (event_type, description) in event_types {
        let name = event_type_name(*event_type);
        output.push_str(&format!("0x{:04x} {} - {}\n", event_type, name, description));
    }

    output
}

/// Generate content for /proc/trace/buffer
///
/// Dumps the trace buffer contents in a parseable format.
/// Format: `CPU idx ts type name payload`
pub fn generate_buffer() -> String {
    let mut output = String::new();

    // Header
    let enabled = TRACE_ENABLED.load(Ordering::Relaxed) != 0;
    output.push_str(&format!(
        "# Trace buffer dump (tracing {})\n",
        if enabled { "enabled" } else { "disabled" }
    ));
    output.push_str("# Format: cpu index timestamp type name payload\n\n");

    // Dump each CPU's buffer
    for cpu in 0..MAX_CPUS {
        let buffer = unsafe {
            let buffers_ptr = core::ptr::addr_of!(TRACE_BUFFERS);
            &(*buffers_ptr)[cpu]
        };

        let write_idx = buffer.write_index();
        if write_idx == 0 {
            continue; // Skip empty buffers
        }

        let count = core::cmp::min(write_idx, TRACE_BUFFER_SIZE);
        let start = if write_idx > TRACE_BUFFER_SIZE {
            write_idx & (TRACE_BUFFER_SIZE - 1)
        } else {
            0
        };

        output.push_str(&format!("# CPU {} ({} events)\n", cpu, count));

        // Dump events in chronological order
        for i in 0..count {
            let idx = (start + i) & (TRACE_BUFFER_SIZE - 1);
            if let Some(event) = buffer.get_event(idx) {
                let name = event_type_name(event.event_type);
                output.push_str(&format!(
                    "{} {} {} 0x{:04x} {} {}\n",
                    cpu, idx, event.timestamp, event.event_type, name, event.payload
                ));
            }
        }

        output.push('\n');
    }

    if output.lines().count() <= 3 {
        output.push_str("# (no trace events recorded)\n");
    }

    output
}

/// Generate content for /proc/trace/counters
///
/// Lists all counter values in a parseable format.
/// Format: `counter_name: total_value (cpu0=v0, cpu1=v1, ...)`
pub fn generate_counters() -> String {
    let mut output = String::new();

    let count = TRACE_COUNTER_COUNT.load(Ordering::Relaxed);
    output.push_str(&format!("# {} registered counters\n\n", count));

    for counter in list_counters() {
        let total = counter.aggregate();
        output.push_str(&format!("{}: {}", counter.name, total));

        // Add per-CPU breakdown for non-zero values
        let mut has_percpu = false;
        let mut percpu_str = String::new();

        for cpu in 0..MAX_CPUS {
            let val = counter.get_cpu(cpu);
            if val > 0 {
                if has_percpu {
                    percpu_str.push_str(", ");
                }
                percpu_str.push_str(&format!("cpu{}={}", cpu, val));
                has_percpu = true;
            }
        }

        if has_percpu {
            output.push_str(&format!(" ({})", percpu_str));
        }

        output.push('\n');
    }

    if count == 0 {
        output.push_str("# (no counters registered)\n");
    }

    output
}

/// Generate content for /proc/trace/providers
///
/// Lists all registered trace providers.
/// Format: `provider_name id=0xNN enabled=0xNNNN`
pub fn generate_providers() -> String {
    let mut output = String::new();

    let count = TRACE_PROVIDER_COUNT.load(Ordering::Relaxed);
    output.push_str(&format!("# {} registered providers\n\n", count));

    unsafe {
        let providers_ptr = core::ptr::addr_of!(TRACE_PROVIDERS);
        for i in 0..crate::tracing::provider::MAX_PROVIDERS {
            if let Some(provider) = (*providers_ptr)[i] {
                let enabled = provider.enabled.load(Ordering::Relaxed);
                output.push_str(&format!(
                    "{}: id=0x{:02x} enabled=0x{:016x}\n",
                    provider.name, provider.id, enabled
                ));
            }
        }
    }

    if count == 0 {
        output.push_str("# (no providers registered)\n");
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_generate_enable() {
        let result = generate_enable();
        assert!(result == "0\n" || result == "1\n");
    }

    #[test_case]
    fn test_generate_events_not_empty() {
        let result = generate_events();
        assert!(!result.is_empty());
        assert!(result.contains("SYSCALL_ENTRY"));
    }

    #[test_case]
    fn test_generate_providers_has_header() {
        let result = generate_providers();
        assert!(result.contains("# "));
        assert!(result.contains("registered providers"));
    }
}
