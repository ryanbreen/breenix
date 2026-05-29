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

use crate::tracing::counter::{list_counters, TRACE_COUNTER_COUNT};
use crate::tracing::provider::{TRACE_PROVIDERS, TRACE_PROVIDER_COUNT};
use crate::tracing::providers::virtgpu;
use crate::tracing::{
    event_type_name, TraceEventType, MAX_CPUS, TRACE_BUFFERS, TRACE_BUFFER_SIZE, TRACE_ENABLED,
};

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
        (
            TraceEventType::CTX_SWITCH_TO_KERNEL,
            "switch to kernel mode",
        ),
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
        // VirtIO GPU PCI events (0x07xx)
        (virtgpu::VIRTGPU_CMD_SUBMIT, "GPU command submit"),
        (virtgpu::VIRTGPU_CMD_RESOURCE, "GPU command resource id"),
        (virtgpu::VIRTGPU_Q_NOTIFY, "GPU virtqueue notify"),
        (virtgpu::VIRTGPU_Q_COMPLETE, "GPU virtqueue completion"),
        (virtgpu::VIRTGPU_RESPONSE, "GPU command response"),
        (virtgpu::VIRTGPU_STALE_DRAIN, "GPU stale completion drain"),
        (
            virtgpu::VIRTGPU_FLUSH_CONSTRUCT,
            "GPU RESOURCE_FLUSH command construction",
        ),
        (
            virtgpu::VIRTGPU_FLUSH_BUFFER_PRE_NOTIFY,
            "GPU RESOURCE_FLUSH wire resource id",
        ),
        (
            virtgpu::VIRTGPU_FLUSH_READBACK_MISMATCH,
            "GPU RESOURCE_FLUSH readback mismatch",
        ),
        (virtgpu::VIRTGPU_WAIT_TIMEOUT, "GPU used-ring wait timeout"),
        (
            virtgpu::BWM_COMPOSITE_FRAME_ENTER,
            "BWM op10 composite frame entry",
        ),
        (
            virtgpu::BWM_COMPOSITE_FRAME_EXIT,
            "BWM op10 composite frame exit",
        ),
        (virtgpu::VIRTGPU_FLUSH_ENTER, "GPU RESOURCE_FLUSH entry"),
        (virtgpu::VIRTGPU_FLUSH_EXIT, "GPU RESOURCE_FLUSH exit"),
        (virtgpu::VIRTGPU_SUBMIT_3D_ENTER, "GPU SUBMIT_3D entry"),
        (virtgpu::VIRTGPU_SUBMIT_3D_EXIT, "GPU SUBMIT_3D exit"),
        (
            virtgpu::VIRTGPU_WAIT_COMPLETION_ENTER,
            "GPU used-ring wait entry",
        ),
        (
            virtgpu::VIRTGPU_WAIT_COMPLETION_EXIT,
            "GPU used-ring wait exit",
        ),
        // Debug markers (0xFFxx)
        (TraceEventType::MARKER_A, "debug marker A"),
        (TraceEventType::MARKER_B, "debug marker B"),
        (TraceEventType::MARKER_C, "debug marker C"),
        (TraceEventType::MARKER_D, "debug marker D"),
    ];

    for (event_type, description) in event_types {
        let name = event_type_name(*event_type);
        output.push_str(&format!(
            "0x{:04x} {} - {}\n",
            event_type, name, description
        ));
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

    #[cfg(target_arch = "aarch64")]
    append_gicv2m_diag(&mut output);
    #[cfg(target_arch = "aarch64")]
    append_net_pci_diag(&mut output);

    if count == 0 {
        output.push_str("# (no counters registered)\n");
    }

    output
}

#[cfg(target_arch = "aarch64")]
fn append_gicv2m_diag(output: &mut String) {
    let Some(irq) = crate::drivers::virtio::net_pci::get_irq() else {
        return;
    };

    if let Some(frame) = crate::platform_config::gicv2m_diag_snapshot() {
        output.push_str(&format!("GICV2M_BASE_PHYS: {:#x}\n", frame.base_phys));
        output.push_str(&format!(
            "GICV2M_DOORBELL_PHYS: {:#x}\n",
            frame.doorbell_phys
        ));
        output.push_str(&format!("GICV2M_MSI_TYPER: {:#x}\n", frame.msi_typer));
        output.push_str(&format!("GICV2M_SPI_BASE: {}\n", frame.spi_base));
        output.push_str(&format!("GICV2M_SPI_COUNT: {}\n", frame.spi_count));
        output.push_str(&format!("GICV2M_NEXT_INDEX: {}\n", frame.next_index));
    }

    append_spi_diag(output, "GIC_SPI54", 54);
    append_spi_diag(output, "GIC_SPI55", irq);

    let manual = crate::drivers::virtio::net_pci::manual_gicv2m_diag_snapshot();
    output.push_str(&format!("MANUAL_GICV2M_TEST_DONE: {}\n", manual.done as u8));
    output.push_str(&format!("MANUAL_GICV2M_TEST_IRQ: {}\n", manual.irq));
    output.push_str(&format!(
        "MANUAL_GICV2M_TEST_DOORBELL: {:#x}\n",
        manual.doorbell_phys
    ));
    output.push_str(&format!(
        "MANUAL_GICV2M_TEST_BEFORE_PEND: {:#x}\n",
        manual.before_pend
    ));
    output.push_str(&format!(
        "MANUAL_GICV2M_TEST_AFTER_WRITE_PEND: {:#x}\n",
        manual.after_write_pend
    ));
    output.push_str(&format!(
        "MANUAL_GICV2M_TEST_AFTER_WAIT_PEND: {:#x}\n",
        manual.after_wait_pend
    ));
    output.push_str(&format!(
        "MANUAL_GICV2M_TEST_ACK_BEFORE: {}\n",
        manual.ack_before
    ));
    output.push_str(&format!(
        "MANUAL_GICV2M_TEST_ACK_AFTER: {}\n",
        manual.ack_after
    ));
    output.push_str(&format!(
        "MANUAL_GICV2M_TEST_MSI_BEFORE: {}\n",
        manual.msi_before
    ));
    output.push_str(&format!(
        "MANUAL_GICV2M_TEST_MSI_AFTER: {}\n",
        manual.msi_after
    ));
}

#[cfg(target_arch = "aarch64")]
fn append_spi_diag(output: &mut String, prefix: &str, irq: u32) {
    if let Some(spi) = crate::arch_impl::aarch64::gic::spi_diag_snapshot(irq) {
        output.push_str(&format!("{}_IRQ: {}\n", prefix, spi.irq));
        output.push_str(&format!("{}_VERSION: {}\n", prefix, spi.version));
        output.push_str(&format!(
            "{}_ISENABLER_BIT: {:#x}\n",
            prefix, spi.isenabler_bit
        ));
        output.push_str(&format!("{}_ISPENDR_BIT: {:#x}\n", prefix, spi.ispendr_bit));
        output.push_str(&format!(
            "{}_ISACTIVER_BIT: {:#x}\n",
            prefix, spi.isactiver_bit
        ));
        output.push_str(&format!("{}_IGROUPR_BIT: {:#x}\n", prefix, spi.igroupr_bit));
        output.push_str(&format!("{}_PRIORITY: {:#x}\n", prefix, spi.priority));
        output.push_str(&format!("{}_ICFGR_REG: {:#x}\n", prefix, spi.icfgr_reg));
        output.push_str(&format!("{}_IROUTER: {:#x}\n", prefix, spi.irouter));
        output.push_str(&format!(
            "{}_ITARGETSR_BYTE: {:#x}\n",
            prefix, spi.itargetsr_byte
        ));
        output.push_str(&format!("{}_GICD_CTLR: {:#x}\n", prefix, spi.gicd_ctlr));
    }
}

#[cfg(target_arch = "aarch64")]
fn append_net_pci_diag(output: &mut String) {
    let Some(net) = crate::drivers::virtio::net_pci::diag_snapshot() else {
        return;
    };

    output.push_str(&format!(
        "NET_PCI_DEVICE_STATUS: {:#x}\n",
        net.device_status
    ));
    output.push_str(&format!("NET_PCI_ISR_STATUS: {:#x}\n", net.isr_status));
    output.push_str(&format!(
        "NET_PCI_DEVICE_FEATURES: {:#x}\n",
        net.device_features
    ));
    output.push_str(&format!(
        "NET_PCI_GUEST_FEATURES: {:#x}\n",
        net.guest_features
    ));
    output.push_str(&format!("NET_PCI_RX_QUEUE_PFN: {:#x}\n", net.rx_queue_pfn));
    output.push_str(&format!("NET_PCI_TX_QUEUE_PFN: {:#x}\n", net.tx_queue_pfn));
    output.push_str(&format!("NET_PCI_RX_QUEUE_SIZE: {}\n", net.rx_queue_size));
    output.push_str(&format!("NET_PCI_TX_QUEUE_SIZE: {}\n", net.tx_queue_size));
    output.push_str(&format!("NET_PCI_RX_QUEUE_ALIGN: {}\n", net.rx_queue_align));
    output.push_str(&format!(
        "NET_PCI_RX_QUEUE_VECTOR: {:#x}\n",
        net.rx_queue_vector
    ));
    output.push_str(&format!(
        "NET_PCI_TX_QUEUE_VECTOR: {:#x}\n",
        net.tx_queue_vector
    ));
    output.push_str(&format!(
        "NET_PCI_RX_AVAIL_FLAGS: {:#x}\n",
        net.rx_avail_flags
    ));
    output.push_str(&format!("NET_PCI_RX_AVAIL_IDX: {}\n", net.rx_avail_idx));
    output.push_str(&format!(
        "NET_PCI_RX_USED_FLAGS: {:#x}\n",
        net.rx_used_flags
    ));
    output.push_str(&format!("NET_PCI_RX_USED_IDX: {}\n", net.rx_used_idx));
    output.push_str(&format!(
        "NET_PCI_RX_LAST_USED_IDX: {}\n",
        net.rx_last_used_idx
    ));
    output.push_str(&format!("NET_PCI_RX_POSTED_GAP: {}\n", net.rx_posted_gap));
    output.push_str(&format!(
        "NET_PCI_RX_DESC0: addr={:#x} len={} flags={:#x}\n",
        net.rx_desc0_addr, net.rx_desc0_len, net.rx_desc0_flags
    ));
    output.push_str(&format!(
        "NET_PCI_RX_DESC1: addr={:#x} len={} flags={:#x}\n",
        net.rx_desc1_addr, net.rx_desc1_len, net.rx_desc1_flags
    ));
    output.push_str(&format!(
        "NET_PCI_RX_DESC2: addr={:#x} len={} flags={:#x}\n",
        net.rx_desc2_addr, net.rx_desc2_len, net.rx_desc2_flags
    ));
    output.push_str(&format!(
        "NET_PCI_RX_DESC3: addr={:#x} len={} flags={:#x}\n",
        net.rx_desc3_addr, net.rx_desc3_len, net.rx_desc3_flags
    ));
    output.push_str(&format!(
        "NET_PCI_RX_RING_HEADS: {},{},{},{}\n",
        net.rx_ring0, net.rx_ring1, net.rx_ring2, net.rx_ring3
    ));
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
