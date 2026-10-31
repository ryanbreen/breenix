//! Bounded post-update EVO snapshots for the two G20 logical modes.
//! Register semantics are the in-tree native/modeset encodings; no new hardware
//! methods are introduced. Formatting and digest output run at upload time.
use super::{present::{Receipt, Surface}, Registers};
use alloc::{format, string::String, vec::Vec, collections::VecDeque};
use core::fmt::Write;
use core::sync::atomic::{AtomicU64, Ordering};

static BASELINE: AtomicU64 = AtomicU64::new(u64::MAX);
static STATE: spin::Mutex<State> = spin::Mutex::new(State { sequence: 0, modes: [None, None], samples: [None, None], sampled: [false; 2], dropped: 0, transitions: VecDeque::new() });
struct State { sequence: u64, modes: [Option<Snapshot>; 2], samples: [Option<Sample>; 2], sampled: [bool; 2], dropped: u64, transitions: VecDeque<Transition> }
struct Transition { snapshot: Snapshot, allocation: [u64; 2], console: [u64; 2], event: &'static str }
#[derive(Clone, Copy)]
struct Sample { offset: usize, expected: u32, actual: u32 }
#[derive(Clone, Copy)]
struct Snapshot {
    sequence: u64, mode: usize, resource: u32, requested: u64, head: u32,
    pitch: u32, block_height: u32, format: u32, notifier: u32, active: [u32; 5], armed: [u32; 5],
    viewport_active: [u32; 8], viewport_armed: [u32; 8], raster: [u32; 2],
    firmware_writes: u64,
}

pub(super) fn baseline(value: u64) { BASELINE.store(value, Ordering::Release); }

/// Called after native::present confirmed completion, both banks and output health.
pub(super) fn record(regs: &mut Registers, head: u32, surface: Surface, receipt: &Receipt) {
    let mode = match (surface.width, surface.height) {
        (1280, 800) => 0, (2560, 1600) => 1, _ => return,
    };
    let mut snapshot = snapshot(regs, head, surface, receipt, mode);
    let mut state = STATE.lock();
    state.sequence += 1;
    snapshot.sequence = state.sequence;
    state.modes[mode] = Some(snapshot);
}

fn snapshot(regs: &Registers, head: u32, surface: Surface, receipt: &Receipt, mode: usize) -> Snapshot {
    let h = head as usize * 0x300;
    let methods = [0x42c, 0x494, 0x4b0, 0x4b8, 0x4bc, 0x4c0, 0x4c4, 0x4c8];
    let viewport_active = methods.map(|m| regs.read(0x640000 + m + h));
    let viewport_armed = methods.map(|m| regs.read(0x660000 + m + h));
    let raster = [regs.read(0x64041c + h), regs.read(0x640420 + h)];
    let firmware_writes = crate::firmware_writes::count()
        .checked_sub(BASELINE.load(Ordering::Acquire)).unwrap_or(u64::MAX);
    Snapshot { sequence: 0, mode, resource: surface.resource, requested: surface.address, head,
        pitch: surface.pitch, block_height: surface.block_height, format: surface.format,
        notifier: receipt.notifier, active: receipt.active, armed: receipt.armed,
        viewport_active, viewport_armed, raster, firmware_writes }
}

/// Retain handoff, logical mode switch, flip and console receipt snapshots
/// until the next G12 upload. Observer pressure drops records instead of failing
/// the display; the exported count makes that loss visible.
pub(super) fn transition(regs: &mut Registers, head: u32, surface: Surface, receipt: &Receipt,
    allocation: [u64; 2], console: [u64; 2], event: &'static str) -> Result<u64, &'static str> {
    let mode = match (surface.width, surface.height) {
        (1280, 800) => 0, (2560, 1600) => 1, _ => return Ok(0),
    };
    let mut snapshot = snapshot(regs, head, surface, receipt, mode);
    let mut state = STATE.lock();
    state.sequence += 1;
    snapshot.sequence = state.sequence;
    if state.transitions.len() == 2048 {
        state.transitions.pop_front();
        state.dropped = state.dropped.saturating_add(1);
    }
    if state.transitions.try_reserve(1).is_err() {
        state.dropped = state.dropped.saturating_add(1);
        return Ok(state.sequence);
    }
    state.transitions.push_back(Transition { snapshot, allocation, console, event });
    Ok(state.sequence)
}

pub(super) fn sample(stage: usize, offset: usize, expected: u32, actual: u32) {
    let mut state = STATE.lock();
    if stage < state.samples.len() && !state.sampled[stage] {
        state.samples[stage] = Some(Sample { offset, expected, actual });
        state.sampled[stage] = true;
    }
}

pub(super) fn export() -> Vec<(String, String)> {
    let (snapshots, samples) = {
        let mut state = STATE.lock();
        (core::mem::take(&mut state.modes), core::mem::take(&mut state.samples))
    };
    let mut rows: Vec<_> = snapshots.into_iter().flatten().collect();
    rows.sort_by_key(|r| r.sequence);
    let gpu_active = crate::gpu::display_active();
    let retired = crate::gpu::console::active();
    let mut result: Vec<_> = rows.into_iter().map(|r| {
        let row = format!(concat!("GT710_DISPLAY_V1 {{\"schema\":1,\"sequence\":{},\"mode\":{},",
            "\"resource\":{},\"requested\":{},\"head\":{},\"pitch\":{},\"block_height\":{},\"format\":{},\"notifier\":{},",
            "\"active\":{:?},\"armed\":{:?},\"viewport_active\":{:?},\"viewport_armed\":{:?},",
            "\"raster\":{:?},\"firmware_writes\":{},\"gpu_active\":{},\"retired\":{}}}"), r.sequence, r.mode,
            r.resource, r.requested, r.head, r.pitch, r.block_height, r.format, r.notifier, r.active, r.armed,
            r.viewport_active, r.viewport_armed, r.raster, r.firmware_writes, gpu_active, retired);
        let digest = crate::block::install::sha256::sha256(row.as_bytes());
        let anchor = format!("GT710_DISPLAY_BULK sequence={} bytes={} sha256={}",
            r.sequence, row.len(), super::hex(&digest));
        (row, anchor)
    }).collect();
    for (stage, sample) in samples.into_iter().enumerate() {
        if let Some(s) = sample {
            let row = format!("GT710_CONSOLE_SAMPLE_V1 {{\"stage\":{},\"offset\":{},\"expected\":{},\"actual\":{}}}",
                stage, s.offset, s.expected, s.actual);
            let digest = crate::block::install::sha256::sha256(row.as_bytes());
            let anchor = format!("GT710_CONSOLE_SAMPLE_BULK stage={} bytes={} sha256={}", stage, row.len(), super::hex(&digest));
            result.push((row, anchor));
        }
    }
    result
}

/// Format the bounded history at upload time, outside the display transaction.
pub(super) fn export_transitions() -> Result<Vec<(String, String)>, &'static str> {
    let (transitions, dropped) = {
        let mut state = STATE.lock();
        (core::mem::take(&mut state.transitions), state.dropped)
    };
    let mut result = Vec::new();
    result.try_reserve_exact(transitions.len()).map_err(|_| "console evidence export allocation")?;
    for t in transitions {
        let r = t.snapshot;
        // Fixed field counts and integer widths fit within this reservation.
        let mut row = String::new();
        row.try_reserve_exact(1536).map_err(|_| "console evidence row allocation")?;
        write!(row, concat!("GT710_CONSOLE_V1 {{\"schema\":1,\"dropped\":{},\"sequence\":{},\"event\":\"{}\",\"mode\":{},",
            "\"resource\":{},\"requested\":{},\"head\":{},\"pitch\":{},\"block_height\":{},\"format\":{},\"notifier\":{},",
            "\"active\":{:?},\"armed\":{:?},\"viewport_active\":{:?},\"viewport_armed\":{:?},",
            "\"raster\":{:?},\"firmware_writes\":{},\"allocation\":{:?},\"console\":{:?}}}"), dropped, r.sequence, t.event, r.mode,
            r.resource, r.requested, r.head, r.pitch, r.block_height, r.format, r.notifier, r.active, r.armed,
            r.viewport_active, r.viewport_armed, r.raster, r.firmware_writes, t.allocation, t.console).map_err(|_| "console evidence formatting")?;
        let digest = crate::block::install::sha256::sha256(row.as_bytes());
        let mut anchor = String::new();
        anchor.try_reserve_exact(160).map_err(|_| "console evidence anchor allocation")?;
        write!(anchor,"GT710_CONSOLE_BULK sequence={} bytes={} sha256=", r.sequence, row.len())
            .map_err(|_| "console evidence anchor formatting")?;
        for byte in digest { write!(anchor,"{byte:02x}").map_err(|_| "console evidence digest formatting")?; }
        result.push((row, anchor));
    }
    Ok(result)
}
