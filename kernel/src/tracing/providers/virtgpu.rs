//! VirtIO GPU PCI command trace provider.
//!
//! Event payload layouts:
//! - `VIRTGPU_CMD_SUBMIT`: `cmd_type[15:0] | seq[31:16]`
//! - `VIRTGPU_CMD_RESOURCE`: `resource_id[31:0]`
//! - `VIRTGPU_Q_NOTIFY`: `head_desc_idx[15:0] | used_idx_before[31:16]`
//! - `VIRTGPU_Q_COMPLETE`: `head_desc_idx[15:0] | used_idx_after[31:16]`
//! - `VIRTGPU_RESPONSE`: `resp_type[31:0]`
//! - `VIRTGPU_STALE_DRAIN`: `entries_drained[15:0] | last_used_idx_after[31:16]`
//! - `VIRTGPU_FLUSH_CONSTRUCT`: `caller_tag[7:0] | helper_id[15:8] | resource_id_arg[31:16]`
//! - `VIRTGPU_FLUSH_BUFFER_PRE_NOTIFY`: `resource_id_on_wire[31:0]`
//! - `VIRTGPU_FLUSH_READBACK_MISMATCH`: `expected_resource_id[15:0] | readback_resource_id[31:16]`
//! - `VIRTGPU_WAIT_TIMEOUT`: `cmd_type[15:0] | resource_id[31:16]`
//! - `BWM_COMPOSITE_FRAME_ENTER`: `width[31:16] | height[15:0]`
//! - `BWM_COMPOSITE_FRAME_EXIT`: `0 on success, 1 on failure`
//! - `VIRTGPU_FLUSH_ENTER/EXIT`: `resource_id[31:16] | helper_id[15:8] | caller_tag[7:0]`
//! - `VIRTGPU_SUBMIT_3D_ENTER`: `submit_id[31:16] | dwords[15:0]`
//! - `VIRTGPU_SUBMIT_3D_EXIT`: `submit_id[31:16] | resp_type[15:0]`
//! - `VIRTGPU_WAIT_COMPLETION_ENTER/EXIT`: `resource_id[31:16] | cmd_type[15:0]`
//!
//! EXIT event flags: 0 = success, 1 = failure for flush/submit. Wait-completion
//! flags use bits 0-6 for path and bit 7 for failure/timeout.

use crate::tracing::counter::{register_counter, TraceCounter};
use crate::tracing::provider::{register_provider, TraceProvider};
use core::sync::atomic::{AtomicU64, Ordering};

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
pub const PROBE_FLUSH_CONSTRUCT: u8 = 0x06;
pub const PROBE_FLUSH_BUFFER_PRE_NOTIFY: u8 = 0x07;
pub const PROBE_FLUSH_READBACK_MISMATCH: u8 = 0x08;
pub const PROBE_WAIT_TIMEOUT: u8 = 0x09;
pub const PROBE_BWM_COMPOSITE_FRAME_ENTER: u8 = 0x0A;
pub const PROBE_BWM_COMPOSITE_FRAME_EXIT: u8 = 0x0B;
pub const PROBE_FLUSH_ENTER: u8 = 0x0C;
pub const PROBE_FLUSH_EXIT: u8 = 0x0D;
pub const PROBE_SUBMIT_3D_ENTER: u8 = 0x0E;
pub const PROBE_SUBMIT_3D_EXIT: u8 = 0x0F;
pub const PROBE_WAIT_COMPLETION_ENTER: u8 = 0x10;
pub const PROBE_WAIT_COMPLETION_EXIT: u8 = 0x11;
pub const VIRTGPU_CMD_SUBMIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CMD_SUBMIT as u16);
pub const VIRTGPU_CMD_RESOURCE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_CMD_RESOURCE as u16);
pub const VIRTGPU_Q_NOTIFY: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_Q_NOTIFY as u16);
pub const VIRTGPU_Q_COMPLETE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_Q_COMPLETE as u16);
pub const VIRTGPU_RESPONSE: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_RESPONSE as u16);
pub const VIRTGPU_STALE_DRAIN: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_STALE_DRAIN as u16);
pub const VIRTGPU_FLUSH_CONSTRUCT: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_FLUSH_CONSTRUCT as u16);
pub const VIRTGPU_FLUSH_BUFFER_PRE_NOTIFY: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_FLUSH_BUFFER_PRE_NOTIFY as u16);
pub const VIRTGPU_FLUSH_READBACK_MISMATCH: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_FLUSH_READBACK_MISMATCH as u16);
pub const VIRTGPU_WAIT_TIMEOUT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_WAIT_TIMEOUT as u16);
pub const BWM_COMPOSITE_FRAME_ENTER: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_BWM_COMPOSITE_FRAME_ENTER as u16);
pub const BWM_COMPOSITE_FRAME_EXIT: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_BWM_COMPOSITE_FRAME_EXIT as u16);
pub const VIRTGPU_FLUSH_ENTER: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_FLUSH_ENTER as u16);
pub const VIRTGPU_FLUSH_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_FLUSH_EXIT as u16);
pub const VIRTGPU_SUBMIT_3D_ENTER: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_SUBMIT_3D_ENTER as u16);
pub const VIRTGPU_SUBMIT_3D_EXIT: u16 = ((PROVIDER_ID as u16) << 8) | (PROBE_SUBMIT_3D_EXIT as u16);
pub const VIRTGPU_WAIT_COMPLETION_ENTER: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_WAIT_COMPLETION_ENTER as u16);
pub const VIRTGPU_WAIT_COMPLETION_EXIT: u16 =
    ((PROVIDER_ID as u16) << 8) | (PROBE_WAIT_COMPLETION_EXIT as u16);
pub const FLUSH_HELPER_3D: u8 = 0;
pub const FLUSH_HELPER_2D: u8 = 1;

pub const WAIT_PATH_2DESC: u8 = 0;
pub const WAIT_PATH_3DESC_MSI: u8 = 1;
pub const WAIT_PATH_3DESC_POLL: u8 = 2;

pub const FLUSH_CALLER_2D_FULL: u8 = 1;
pub const FLUSH_CALLER_2D_RECT: u8 = 2;
pub const FLUSH_CALLER_2D_ONLY: u8 = 3;
pub const FLUSH_CALLER_VIRGL_RENDER_FRAME: u8 = 4;
pub const FLUSH_CALLER_VIRGL_RENDER_RECTS: u8 = 5;
pub const FLUSH_CALLER_VIRGL_COMPOSITE_FRAME: u8 = 6;
pub const FLUSH_CALLER_VIRGL_COMPOSITE_FRAME_TEXTURED: u8 = 7;
pub const FLUSH_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD: u8 = 8;
pub const FLUSH_CALLER_VIRGL_FLUSH: u8 = 9;
pub const FLUSH_CALLER_VIRGL_INIT_PRIME: u8 = 10;
pub const FLUSH_CALLER_VIRGL_INIT_STEP7: u8 = 11;
pub const FLUSH_CALLER_VIRGL_INIT_STEP10: u8 = 12;
pub const FLUSH_CALLER_VIRGL_TEST_TEXTURED_QUAD: u8 = 13;

#[no_mangle]
pub static VIRTGPU_SUBMIT_TOTAL: TraceCounter =
    TraceCounter::new("VIRTGPU_SUBMIT_TOTAL", "VirtIO GPU command submissions");

#[no_mangle]
pub static VIRTGPU_COMPLETE_TOTAL: TraceCounter = TraceCounter::new(
    "VIRTGPU_COMPLETE_TOTAL",
    "VirtIO GPU command queue completions",
);

#[no_mangle]
pub static VIRTGPU_FAIL_TOTAL: TraceCounter =
    TraceCounter::new("VIRTGPU_FAIL_TOTAL", "VirtIO GPU command failures");

#[no_mangle]
pub static VIRTGPU_WAIT_TIMEOUT_COUNT: TraceCounter = TraceCounter::new(
    "VIRTGPU_WAIT_TIMEOUT_COUNT",
    "VirtIO GPU command wait timeouts",
);

#[no_mangle]
pub static BWM_COMPOSITE_FRAME_ENTER_TOTAL: TraceCounter = TraceCounter::new(
    "BWM_COMPOSITE_FRAME_ENTER_TOTAL",
    "BWM op10 composite frame entries",
);

#[no_mangle]
pub static BWM_COMPOSITE_FRAME_EXIT_TOTAL: TraceCounter = TraceCounter::new(
    "BWM_COMPOSITE_FRAME_EXIT_TOTAL",
    "BWM op10 composite frame exits",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_ENTER_TOTAL: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_ENTER_TOTAL",
    "VirtIO GPU RESOURCE_FLUSH path entries",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_EXIT_TOTAL: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_EXIT_TOTAL",
    "VirtIO GPU RESOURCE_FLUSH path exits",
);

#[no_mangle]
pub static VIRTGPU_SUBMIT_3D_ENTER_TOTAL: TraceCounter = TraceCounter::new(
    "VIRTGPU_SUBMIT_3D_ENTER_TOTAL",
    "VirtIO GPU SUBMIT_3D entries",
);

#[no_mangle]
pub static VIRTGPU_SUBMIT_3D_EXIT_TOTAL: TraceCounter =
    TraceCounter::new("VIRTGPU_SUBMIT_3D_EXIT_TOTAL", "VirtIO GPU SUBMIT_3D exits");

#[no_mangle]
pub static VIRTGPU_WAIT_COMPLETION_ENTER_TOTAL: TraceCounter = TraceCounter::new(
    "VIRTGPU_WAIT_COMPLETION_ENTER_TOTAL",
    "VirtIO GPU used-ring wait entries",
);

#[no_mangle]
pub static VIRTGPU_WAIT_COMPLETION_EXIT_TOTAL: TraceCounter = TraceCounter::new(
    "VIRTGPU_WAIT_COMPLETION_EXIT_TOTAL",
    "VirtIO GPU used-ring wait exits",
);

#[no_mangle]
pub static VIRTGPU_LAST_COMPLETION_MS: AtomicU64 = AtomicU64::new(0);

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

#[no_mangle]
pub static VIRTGPU_FLUSH_WITH_ZERO_RES: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_WITH_ZERO_RES",
    "RESOURCE_FLUSH constructed or submitted with resource_id 0",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_2D_FULL: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_2D_FULL",
    "2D full framebuffer flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_2D_RECT: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_2D_RECT",
    "2D rectangular flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_2D_ONLY: TraceCounter =
    TraceCounter::new("VIRTGPU_FLUSH_BY_CALLER_2D_ONLY", "2D flush-only calls");

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_RENDER_FRAME: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_RENDER_FRAME",
    "VirGL circle render frame flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_RENDER_RECTS: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_RENDER_RECTS",
    "VirGL rectangle render flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_FRAME: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_FRAME",
    "Direct CPU composite frame flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_FRAME_TEXTURED: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_FRAME_TEXTURED",
    "Textured composite frame flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD",
    "Single-quad compositor flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_FLUSH: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_FLUSH",
    "Public virgl_flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_PRIME: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_PRIME",
    "VirGL init resource prime flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_STEP7: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_STEP7",
    "VirGL init step 7 flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_STEP10: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_STEP10",
    "VirGL init step 10 flush calls",
);

#[no_mangle]
pub static VIRTGPU_FLUSH_BY_CALLER_VIRGL_TEST_TEXTURED_QUAD: TraceCounter = TraceCounter::new(
    "VIRTGPU_FLUSH_BY_CALLER_VIRGL_TEST_TEXTURED_QUAD",
    "VirGL textured quad test flush calls",
);

/// Register the provider and counters.
pub fn init() {
    register_provider(&VIRTGPU_PROVIDER);
    register_counter(&VIRTGPU_SUBMIT_TOTAL);
    register_counter(&VIRTGPU_COMPLETE_TOTAL);
    register_counter(&VIRTGPU_FAIL_TOTAL);
    register_counter(&VIRTGPU_WAIT_TIMEOUT_COUNT);
    register_counter(&BWM_COMPOSITE_FRAME_ENTER_TOTAL);
    register_counter(&BWM_COMPOSITE_FRAME_EXIT_TOTAL);
    register_counter(&VIRTGPU_FLUSH_ENTER_TOTAL);
    register_counter(&VIRTGPU_FLUSH_EXIT_TOTAL);
    register_counter(&VIRTGPU_SUBMIT_3D_ENTER_TOTAL);
    register_counter(&VIRTGPU_SUBMIT_3D_EXIT_TOTAL);
    register_counter(&VIRTGPU_WAIT_COMPLETION_ENTER_TOTAL);
    register_counter(&VIRTGPU_WAIT_COMPLETION_EXIT_TOTAL);
    register_counter(&VIRTGPU_R2_CREATE);
    register_counter(&VIRTGPU_R2_ATTACH_BACKING);
    register_counter(&VIRTGPU_R2_CTX_ATTACH);
    register_counter(&VIRTGPU_R2_TRANSFER);
    register_counter(&VIRTGPU_R2_SET_SCANOUT);
    register_counter(&VIRTGPU_R2_FLUSH_OK);
    register_counter(&VIRTGPU_R2_FLUSH_FAIL);
    register_counter(&VIRTGPU_R2_UNREF_OR_DETACH);
    register_counter(&VIRTGPU_FLUSH_WITH_ZERO_RES);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_2D_FULL);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_2D_RECT);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_2D_ONLY);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_RENDER_FRAME);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_RENDER_RECTS);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_FRAME);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_FRAME_TEXTURED);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_FLUSH);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_PRIME);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_STEP7);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_STEP10);
    register_counter(&VIRTGPU_FLUSH_BY_CALLER_VIRGL_TEST_TEXTURED_QUAD);
}

#[inline(always)]
pub fn trace_cmd_submit(cmd_type: u32, seq: u16) {
    VIRTGPU_SUBMIT_TOTAL.increment();
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
    VIRTGPU_COMPLETE_TOTAL.increment();
    VIRTGPU_LAST_COMPLETION_MS.store(now_ms(), Ordering::Relaxed);
    let payload = ((used_idx_after as u32) << 16) | (head_desc_idx as u32);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_Q_COMPLETE, payload);
}

#[inline(always)]
pub fn trace_response(resp_type: u32) {
    if resp_type >= 0x1200 {
        VIRTGPU_FAIL_TOTAL.increment();
    }
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_RESPONSE, resp_type);
}

#[inline(always)]
pub fn trace_stale_drain(entries_drained: u16, last_used_idx_after: u16) {
    let payload = ((last_used_idx_after as u32) << 16) | (entries_drained as u32);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_STALE_DRAIN, payload);
}

#[inline(always)]
pub fn trace_flush_construct_if_unexpected(
    caller_tag: u8,
    helper_id: u8,
    resource_id_arg: u32,
    unexpected: bool,
) {
    if resource_id_arg == 0 {
        VIRTGPU_FLUSH_WITH_ZERO_RES.increment();
    }
    if !unexpected {
        return;
    }

    let payload =
        ((resource_id_arg & 0xffff) << 16) | ((helper_id as u32) << 8) | (caller_tag as u32);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_FLUSH_CONSTRUCT, payload);
    count_flush_caller(caller_tag);
}

#[inline(always)]
pub fn trace_flush_buffer_pre_notify_if_zero(resource_id_on_wire: u32) {
    if resource_id_on_wire != 0 {
        return;
    }

    VIRTGPU_FLUSH_WITH_ZERO_RES.increment();
    crate::trace_event!(
        VIRTGPU_PROVIDER,
        VIRTGPU_FLUSH_BUFFER_PRE_NOTIFY,
        resource_id_on_wire
    );
}

#[inline(always)]
pub fn trace_flush_readback_mismatch(expected: u32, readback: u32) {
    let payload = ((readback & 0xffff) << 16) | (expected & 0xffff);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_FLUSH_READBACK_MISMATCH, payload);
}

#[inline(always)]
pub fn trace_wait_timeout(cmd_type: u32, resource_id: u32) {
    VIRTGPU_WAIT_TIMEOUT_COUNT.increment();
    VIRTGPU_FAIL_TOTAL.increment();
    let payload = ((resource_id & 0xffff) << 16) | (cmd_type & 0xffff);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_WAIT_TIMEOUT, payload);
}

#[inline(always)]
pub fn trace_bwm_composite_frame_enter(width: u32, height: u32) {
    crate::trace_count!(BWM_COMPOSITE_FRAME_ENTER_TOTAL);
    let payload = ((width & 0xffff) << 16) | (height & 0xffff);
    crate::trace_event!(VIRTGPU_PROVIDER, BWM_COMPOSITE_FRAME_ENTER, payload);
}

#[inline(always)]
pub fn trace_bwm_composite_frame_exit(status: u32) {
    crate::trace_count!(BWM_COMPOSITE_FRAME_EXIT_TOTAL);
    crate::trace_event!(VIRTGPU_PROVIDER, BWM_COMPOSITE_FRAME_EXIT, status);
}

#[inline(always)]
pub fn trace_flush_enter(helper_id: u8, caller_tag: u8, resource_id: u32) {
    crate::trace_count!(VIRTGPU_FLUSH_ENTER_TOTAL);
    let payload = pack_flush_payload(helper_id, caller_tag, resource_id);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_FLUSH_ENTER, payload);
}

#[inline(always)]
pub fn trace_flush_exit(helper_id: u8, caller_tag: u8, resource_id: u32, ok: bool) {
    crate::trace_count!(VIRTGPU_FLUSH_EXIT_TOTAL);
    let payload = pack_flush_payload(helper_id, caller_tag, resource_id);
    crate::trace_event!(
        VIRTGPU_PROVIDER,
        VIRTGPU_FLUSH_EXIT,
        trace_result_flag(ok),
        payload
    );
}

#[inline(always)]
pub fn trace_submit_3d_enter(submit_id: u64, dwords: u32) {
    crate::trace_count!(VIRTGPU_SUBMIT_3D_ENTER_TOTAL);
    let payload = (((submit_id as u32) & 0xffff) << 16) | (dwords & 0xffff);
    crate::trace_event!(VIRTGPU_PROVIDER, VIRTGPU_SUBMIT_3D_ENTER, payload);
}

#[inline(always)]
pub fn trace_submit_3d_exit(submit_id: u64, resp_type: u32, ok: bool) {
    crate::trace_count!(VIRTGPU_SUBMIT_3D_EXIT_TOTAL);
    let payload = (((submit_id as u32) & 0xffff) << 16) | (resp_type & 0xffff);
    crate::trace_event!(
        VIRTGPU_PROVIDER,
        VIRTGPU_SUBMIT_3D_EXIT,
        trace_result_flag(ok),
        payload
    );
}

#[inline(always)]
pub fn trace_wait_completion_enter(cmd_type: u32, resource_id: u32, path: u8) {
    crate::trace_count!(VIRTGPU_WAIT_COMPLETION_ENTER_TOTAL);
    let payload = pack_cmd_resource_payload(cmd_type, resource_id);
    crate::trace_event!(
        VIRTGPU_PROVIDER,
        VIRTGPU_WAIT_COMPLETION_ENTER,
        path,
        payload
    );
}

#[inline(always)]
pub fn trace_wait_completion_exit(cmd_type: u32, resource_id: u32, path: u8, ok: bool) {
    crate::trace_count!(VIRTGPU_WAIT_COMPLETION_EXIT_TOTAL);
    let payload = pack_cmd_resource_payload(cmd_type, resource_id);
    let flags = path | if ok { 0 } else { 0x80 };
    crate::trace_event!(
        VIRTGPU_PROVIDER,
        VIRTGPU_WAIT_COMPLETION_EXIT,
        flags,
        payload
    );
}

#[inline]
pub fn freeze_watch_snapshot() -> (u64, u64, u64, u64, u64, u64) {
    (
        VIRTGPU_SUBMIT_TOTAL.aggregate(),
        VIRTGPU_COMPLETE_TOTAL.aggregate(),
        VIRTGPU_FAIL_TOTAL.aggregate(),
        VIRTGPU_LAST_COMPLETION_MS.load(Ordering::Relaxed),
        VIRTGPU_R2_FLUSH_OK.aggregate(),
        VIRTGPU_WAIT_TIMEOUT_COUNT.aggregate(),
    )
}

#[inline(always)]
fn now_ms() -> u64 {
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    secs.saturating_mul(1000) + nanos / 1_000_000
}

#[inline(always)]
fn count_flush_caller(caller_tag: u8) {
    match caller_tag {
        FLUSH_CALLER_2D_FULL => VIRTGPU_FLUSH_BY_CALLER_2D_FULL.increment(),
        FLUSH_CALLER_2D_RECT => VIRTGPU_FLUSH_BY_CALLER_2D_RECT.increment(),
        FLUSH_CALLER_2D_ONLY => VIRTGPU_FLUSH_BY_CALLER_2D_ONLY.increment(),
        FLUSH_CALLER_VIRGL_RENDER_FRAME => VIRTGPU_FLUSH_BY_CALLER_VIRGL_RENDER_FRAME.increment(),
        FLUSH_CALLER_VIRGL_RENDER_RECTS => VIRTGPU_FLUSH_BY_CALLER_VIRGL_RENDER_RECTS.increment(),
        FLUSH_CALLER_VIRGL_COMPOSITE_FRAME => {
            VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_FRAME.increment()
        }
        FLUSH_CALLER_VIRGL_COMPOSITE_FRAME_TEXTURED => {
            VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_FRAME_TEXTURED.increment()
        }
        FLUSH_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD => {
            VIRTGPU_FLUSH_BY_CALLER_VIRGL_COMPOSITE_SINGLE_QUAD.increment()
        }
        FLUSH_CALLER_VIRGL_FLUSH => VIRTGPU_FLUSH_BY_CALLER_VIRGL_FLUSH.increment(),
        FLUSH_CALLER_VIRGL_INIT_PRIME => VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_PRIME.increment(),
        FLUSH_CALLER_VIRGL_INIT_STEP7 => VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_STEP7.increment(),
        FLUSH_CALLER_VIRGL_INIT_STEP10 => VIRTGPU_FLUSH_BY_CALLER_VIRGL_INIT_STEP10.increment(),
        FLUSH_CALLER_VIRGL_TEST_TEXTURED_QUAD => {
            VIRTGPU_FLUSH_BY_CALLER_VIRGL_TEST_TEXTURED_QUAD.increment()
        }
        _ => {}
    }
}

#[inline(always)]
fn pack_flush_payload(helper_id: u8, caller_tag: u8, resource_id: u32) -> u32 {
    ((resource_id & 0xffff) << 16) | ((helper_id as u32) << 8) | (caller_tag as u32)
}

#[inline(always)]
fn pack_cmd_resource_payload(cmd_type: u32, resource_id: u32) -> u32 {
    ((resource_id & 0xffff) << 16) | (cmd_type & 0xffff)
}

#[inline(always)]
fn trace_result_flag(ok: bool) -> u8 {
    if ok {
        0
    } else {
        1
    }
}
