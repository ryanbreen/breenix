//! VirtIO GPU PCI command trace provider.
//!
//! Event payload layouts:
//! - `VIRTGPU_CMD_SUBMIT`: `cmd_type[15:0] | seq[31:16]`
//! - `VIRTGPU_CMD_RESOURCE`: `resource_id[31:0]`
//! - `VIRTGPU_Q_NOTIFY`: `head_desc_idx[15:0] | used_idx_before[31:16]`
//! - `VIRTGPU_Q_COMPLETE`: `head_desc_idx[15:0] | used_idx_after[31:16]`
//! - `VIRTGPU_RESPONSE`: `resp_type[31:0]`
//! - `VIRTGPU_STALE_DRAIN`: `entries_drained[15:0] | last_used_idx_after[31:16]`

use crate::tracing::counter::{register_counter, TraceCounter};
use crate::tracing::provider::{register_provider, TraceProvider};
use core::sync::atomic::AtomicU64;

/// Provider ID for VirtIO GPU events (0x07xx range).
pub const PROVIDER_ID: u8 = 0x07;

/// VirtIO GPU trace provider.
///
/// GDB: `print VIRTGPU_PROVIDER`
#[no_mangle]
pub static VIRTGPU_PROVIDER: TraceProvider = TraceProvider {
    name: "virtgpu",
    id: PROVIDER_ID,
    enabled: AtomicU64::new(0),
};

pub const PROBE_CMD_SUBMIT: u8 = 0x00;
pub const PROBE_CMD_RESOURCE: u8 = 0x01;
pub const PROBE_Q_NOTIFY: u8 = 0x02;
pub const PROBE_Q_COMPLETE: u8 = 0x03;
pub const PROBE_RESPONSE: u8 = 0x04;
pub const PROBE_STALE_DRAIN: u8 = 0x05;

pub const VIRTGPU_CMD_SUBMIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CMD_SUBMIT as u16);
pub const VIRTGPU_CMD_RESOURCE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CMD_RESOURCE as u16);
pub const VIRTGPU_Q_NOTIFY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_Q_NOTIFY as u16);
pub const VIRTGPU_Q_COMPLETE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_Q_COMPLETE as u16);
pub const VIRTGPU_RESPONSE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_RESPONSE as u16);
pub const VIRTGPU_STALE_DRAIN: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_STALE_DRAIN as u16);

#[no_mangle]
pub static VIRTGPU_R2_CREATE: TraceCounter = TraceCounter::new(
    "VIRTGPU_R2_CREATE",
    "RESOURCE_CREATE_3D submissions for resource 2",
);

#[no_mangle]
pub static VIRTGPU_R2_ATTACH_BACKING: TraceCounter = TraceCounter::new(
    "VIRTGPU_R2_ATTACH_BACKING",
    "RESOURCE_ATTACH_BACKING submissions for resource 2",
);

#[no_mangle]
pub static VIRTGPU_R2_CTX_ATTACH: TraceCounter = TraceCounter::new(
    "VIRTGPU_R2_CTX_ATTACH",
    "CTX_ATTACH_RESOURCE submissions for resource 2",
);

#[no_mangle]
pub static VIRTGPU_R2_TRANSFER: TraceCounter = TraceCounter::new(
    "VIRTGPU_R2_TRANSFER",
    "TRANSFER_TO_HOST_3D submissions for resource 2",
);

#[no_mangle]
pub static VIRTGPU_R2_SET_SCANOUT: TraceCounter = TraceCounter::new(
    "VIRTGPU_R2_SET_SCANOUT",
    "SET_SCANOUT submissions for resource 2",
);

#[no_mangle]
pub static VIRTGPU_R2_FLUSH_OK: TraceCounter = TraceCounter::new(
    "VIRTGPU_R2_FLUSH_OK",
    "RESOURCE_FLUSH OK responses for resource 2",
);

#[no_mangle]
pub static VIRTGPU_R2_FLUSH_FAIL: TraceCounter = TraceCounter::new(
    "VIRTGPU_R2_FLUSH_FAIL",
    "RESOURCE_FLUSH non-OK responses for resource 2",
);

#[no_mangle]
pub static VIRTGPU_R2_UNREF_OR_DETACH: TraceCounter = TraceCounter::new(
    "VIRTGPU_R2_UNREF_OR_DETACH",
    "UNREF, DETACH_BACKING, or CTX_DETACH submissions for resource 2",
);

/// Register the provider and counters.
pub fn init() {
    register_provider(&VIRTGPU_PROVIDER);
    register_counter(&VIRTGPU_R2_CREATE);
    register_counter(&VIRTGPU_R2_ATTACH_BACKING);
    register_counter(&VIRTGPU_R2_CTX_ATTACH);
    register_counter(&VIRTGPU_R2_TRANSFER);
    register_counter(&VIRTGPU_R2_SET_SCANOUT);
    register_counter(&VIRTGPU_R2_FLUSH_OK);
    register_counter(&VIRTGPU_R2_FLUSH_FAIL);
    register_counter(&VIRTGPU_R2_UNREF_OR_DETACH);
}

#[inline(always)]
pub fn trace_cmd_submit(cmd_type: u32, seq: u16) {
    let payload = ((seq as u32) << 16) | (cmd_type & 0xffff);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_CMD_SUBMIT, payload);
}

#[inline(always)]
pub fn trace_cmd_resource(resource_id: u32) {
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_CMD_RESOURCE, resource_id);
}

#[inline(always)]
pub fn trace_q_notify(head_desc_idx: u16, used_idx_before: u16) {
    let payload = ((used_idx_before as u32) << 16) | (head_desc_idx as u32);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_Q_NOTIFY, payload);
}

#[inline(always)]
pub fn trace_q_complete(head_desc_idx: u16, used_idx_after: u16) {
    let payload = ((used_idx_after as u32) << 16) | (head_desc_idx as u32);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_Q_COMPLETE, payload);
}

#[inline(always)]
pub fn trace_response(resp_type: u32) {
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_RESPONSE, resp_type);
}

#[inline(always)]
pub fn trace_stale_drain(entries_drained: u16, last_used_idx_after: u16) {
    let payload = ((last_used_idx_after as u32) << 16) | (entries_drained as u32);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_STALE_DRAIN, payload);
}
