//! Testing-profile queue trace for issue 2645; no writes to the console while recording.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use super::correlation::{Record, Trace};

pub const PUBLISH: u16 = 1;
pub const KICK_ENTER: u16 = 2;
pub const KICK_RETURN: u16 = 3;
pub const OBSERVE: u16 = 4;
pub const USED: u16 = 5;
pub const IRQ: u16 = 6;
pub const TIMEOUT: u16 = 7;
pub const RESET: u16 = 8;
const KEPT: usize = 128;
static TRACE: Trace<KEPT> = Trace::new();
static EPOCH: AtomicU64 = AtomicU64::new(0);
static FROZEN: AtomicBool = AtomicBool::new(false);
static POSTED: AtomicBool = AtomicBool::new(false);
static ENABLED: crate::log_notice::Node = crate::log_notice::Node::new();
static NODES: [crate::log_notice::Node; KEPT * 5 + 1] =
    [const { crate::log_notice::Node::new() }; KEPT * 5 + 1];

pub fn epoch() -> u64 { EPOCH.load(Ordering::Acquire) }

pub fn event(stage: u16, epoch: u64, entry: u64, detail: u64, used: u16, taken: u16) {
    TRACE.record(Record { at_ns: crate::signal::monotonic_nanos(), epoch, entry,
        fence: 0, serial: 0, context: 0, command: 0, head: u16::MAX,
        stage, detail, used, taken });
}

pub fn reset(made: u64, desc: u64) {
    let epoch = EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    event(RESET, epoch, made, desc, 0, 0);
    if epoch == 1 {
        ENABLED.post_again("GQ enabled v=1 kept=128 feature=gpu_queue_trace");
    }
}

pub fn request(stage: u16, entry: u64, head: u16, req: &super::Request,
    detail: u64, used: u16, taken: u16) {
    let serial = match req.tag {
        super::Tag::Sync(s) | super::Tag::Submit { serial: s, .. } => s,
        super::Tag::Background => 0,
    };
    TRACE.record(Record { at_ns: crate::signal::monotonic_nanos(), epoch: epoch(), entry,
        fence: req.fence.map_or(0, |_| req.hdr.u64_at(super::wire::HDR_FENCE)), serial,
        detail, context: req.hdr.u32_at(super::wire::HDR_CTX), command: req.command_type(),
        head, stage, used, taken });
}

pub fn freeze() {
    if TRACE.freeze() { FROZEN.store(true, Ordering::Release); }
}

/// Called by kgpu after releasing the core lock; nodes drain through normal logging.
pub fn post_frozen() {
    use core::fmt::Write;
    if !FROZEN.load(Ordering::Acquire) || POSTED.swap(true, Ordering::AcqRel) { return; }
    let records = TRACE.snapshot();
    let mut line = crate::gpu::NoticeText { bytes: [0; 80], len: 0 };
    let _ = write!(line, "GQ frozen v=1 written={} lost={} kept={}", TRACE.written(), TRACE.lost(), KEPT);
    NODES[0].post_again(core::str::from_utf8(&line.bytes[..line.len]).unwrap_or("GQ format error"));
    for (i, (ticket, r)) in records.into_iter().flatten().enumerate() {
        for part in 0..5 {
            let mut line = crate::gpu::NoticeText { bytes: [0; 80], len: 0 };
            let _ = match part {
                0 => write!(line, "GQ {} time={} stage={}", ticket, r.at_ns, r.stage),
                1 => write!(line, "GQ {} epoch={} entry={}", ticket, r.epoch, r.entry),
                2 => write!(line, "GQ {} fence={} serial={}", ticket, r.fence, r.serial),
                3 => write!(line, "GQ {} ctx={} cmd={:#x} head={}", ticket, r.context, r.command, r.head),
                _ => write!(line, "GQ {} used={} taken={} detail={:#x}", ticket, r.used, r.taken, r.detail),
            };
            NODES[1 + i * 5 + part].post_again(core::str::from_utf8(&line.bytes[..line.len]).unwrap_or("GQ format error"));
        }
    }
}
