//! Real-time hang watchdogs and fork-shared case diagnostics.
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
pub const HANG_MS: u64 = 10_000;
#[derive(Clone, Copy)]
pub enum Wait { CompositorRegistration, SupervisorReady, Configure, HoldWindow, InputAck, Stderr, Launch, AppOutput, AppExit, Stopped, ChannelSend, SupervisorTeardown, InputGrab, CollectorReply, DiscardReply, ControllerReply, DiagnosticUpload, InputDelivery, TreeCapture }
const WAITS: [Wait; 19] = [Wait::CompositorRegistration, Wait::SupervisorReady, Wait::Configure, Wait::HoldWindow, Wait::InputAck, Wait::Stderr, Wait::Launch, Wait::AppOutput, Wait::AppExit, Wait::Stopped, Wait::ChannelSend, Wait::SupervisorTeardown, Wait::InputGrab, Wait::CollectorReply, Wait::DiscardReply, Wait::ControllerReply, Wait::DiagnosticUpload, Wait::InputDelivery, Wait::TreeCapture];
const NAMES: [&str; 19] = ["CompositorRegistration", "SupervisorReady", "Configure", "HoldWindow", "InputAck", "Stderr", "Launch", "AppOutput", "AppExit", "Stopped", "ChannelSend", "SupervisorTeardown", "InputGrab", "CollectorReply", "DiscardReply", "ControllerReply", "DiagnosticUpload", "InputDelivery", "TreeCapture"];
const CELLS: usize = 6;
// Twenty boundaries for the SMB interruption case; no formatting or output on
// the measured path. Each stamp retains cumulative AppOutput time as well.
const PHASES: &[&str] = &[
    "case-begin", "launch-begin", "launch-done", "stroke-done", "save-requested",
    "save-done", "close-done", "reopen-done", "edit-done", "interrupted-save-begin",
    "stop-issued", "stop-written", "eio-seen", "failed-write", "start-issued",
    "start-written", "recovery", "ui-status", "watcher-done", "finish-done",
];
const PHASE_BASE: usize = WAITS.len() * CELLS;
const APP_OUTPUT_CLOCK: usize = PHASE_BASE + PHASES.len() * 2;
#[path = "suite_observations.rs"]
pub mod observations;
#[path = "suite_mutation_costs.rs"]
mod mutation_costs;
#[path = "suite_markers.rs"]
pub mod markers;
const USED_CELLS: usize = markers::USED_CELLS;
static PAGE: AtomicUsize = AtomicUsize::new(0);
fn now_ms() -> Result<u64, String> {
    libbreenix::time::now_monotonic().map(|t| (t.as_nanos()/1_000_000) as u64).map_err(|e| e.to_string())
}
fn cell(page: usize, wait: Wait, field: usize) -> &'static AtomicU64 {
    // The case scope outlives its children and their watchdogs.
    unsafe { &*((page as *const AtomicU64).add(wait as usize * CELLS + field)) }
}
fn phase_cell(page: usize, index: usize, field: usize) -> &'static AtomicU64 {
    unsafe { &*((page as *const AtomicU64).add(PHASE_BASE + index * 2 + field)) }
}
fn app_output_clock(page: usize) -> &'static AtomicU64 {
    unsafe { &*((page as *const AtomicU64).add(APP_OUTPUT_CLOCK)) }
}
fn app_output_ms(page: usize, at: u64) -> u64 {
    // AppOutput waits on this case's App are sequential. One atomic word keeps
    // a watcher-thread phase from observing a half-updated start/end pair:
    // even = completed time * 2; odd = (start - completed time) * 2 + 1.
    let clock = app_output_clock(page).load(Ordering::Acquire);
    if clock & 1 == 0 { clock >> 1 } else { at.saturating_sub(clock >> 1) }
}
/// Record the first occurrence only, in the case's existing shared diagnostics.
pub fn phase(name: &str) {
    let page = PAGE.load(Ordering::Acquire);
    if page == 0 { return; }
    if let Some(index) = PHASES.iter().position(|&phase| phase == name) {
        if let Ok(at) = now_ms() {
            let total = app_output_ms(page, at);
            if phase_cell(page, index, 0).compare_exchange(0, at, Ordering::Release, Ordering::Relaxed).is_ok() {
                phase_cell(page, index, 1).store(total, Ordering::Release);
            }
        }
    }
}
pub struct Watchdog { wait: Wait, start: u64, page: usize, phased: bool }
impl Watchdog {
    pub fn new(wait: Wait) -> Result<Self, String> {
        let start = now_ms()?;
        let page = PAGE.load(Ordering::Acquire);
        if page != 0 {
            cell(page, wait, 0).fetch_add(1, Ordering::Relaxed);
            cell(page, wait, 4).store(start, Ordering::Relaxed);
        }
        let phased = page != 0 && matches!(wait, Wait::AppOutput)
            && phase_cell(page, 0, 0).load(Ordering::Acquire) != 0;
        if phased {
            let total = app_output_ms(page, start);
            app_output_clock(page).store((start.saturating_sub(total) << 1) | 1, Ordering::Release);
        }
        Ok(Self { wait, start, page, phased })
    }
    pub fn deadline_ms(&self) -> i128 { i128::from(self.start.saturating_add(HANG_MS)) }
    pub fn deadline_ns(&self) -> u64 { self.start.saturating_add(HANG_MS).saturating_mul(1_000_000) }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        if self.page == 0 { return; }
        match now_ms() {
            Ok(end) if end >= self.start => {
                if self.phased {
                    app_output_clock(self.page).store(app_output_ms(self.page, end) << 1, Ordering::Release);
                }
                let elapsed = end - self.start;
                cell(self.page, self.wait, 2).fetch_add(elapsed, Ordering::Relaxed);
                cell(self.page, self.wait, 3).fetch_max(elapsed, Ordering::Relaxed);
            }
            _ => { cell(self.page, self.wait, 5).fetch_add(1, Ordering::Relaxed); }
        }
        cell(self.page, self.wait, 1).fetch_add(1, Ordering::Release);
    }
}
extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}
pub(super) struct Scope { page: usize, previous: usize, mutations: mutation_costs::Snapshot }
impl Scope {
    pub(super) fn new() -> Result<Self, String> {
        #[cfg(target_os="macos")] const ANON: i32 = 0x1000;
        #[cfg(not(target_os="macos"))] const ANON: i32 = 0x20;
        assert!(USED_CELLS * core::mem::size_of::<AtomicU64>() <= 4096);
        let ptr = unsafe { mmap(core::ptr::null_mut(), 4096, 3, 1 | ANON, -1, 0) };
        if ptr as isize == -1 { return Err("control diagnostics mmap failed".into()); }
        for i in 0..USED_CELLS {
            unsafe { ptr.cast::<AtomicU64>().add(i).write(AtomicU64::new(0)); }
        }
        let page = ptr as usize;
        let previous = PAGE.swap(page, Ordering::AcqRel);
        Ok(Self { page, previous, mutations: mutation_costs::Snapshot::read().unwrap_or_default() })
    }
    pub(super) fn failure_records(&self, suite: &str, case: &str, failed: bool) -> Vec<String> {
        let mut records = observations::records(self.page, suite, case, failed);
        records.extend(markers::records(self.page, suite, case, failed));
        records.extend(self.mutations.records(suite, case, failed, mutation_costs::Snapshot::read));
        records
    }
    pub(super) fn report(&self, suite: &str, case: &str) -> Vec<String> {
        WAITS.iter().enumerate().filter_map(|(i, &wait)| {
            let values: Vec<_> = (0..CELLS).map(|f| cell(self.page, wait, f).load(Ordering::Acquire)).collect();
            (values[0] != 0).then(|| format!("UI_CONTROL {suite} {case} wait={} started={} completed={} total_ms={} max_ms={} last_start_ms={} clock_errors={}", NAMES[i], values[0], values[1], values[2], values[3], values[4], values[5]))
        }).collect()
    }
    pub(super) fn phases(&self, suite: &str, case: &str) -> Vec<String> {
        if suite != "gui-apps" || case != "guskit/smb-share-gone"
            || phase_cell(self.page, 0, 0).load(Ordering::Acquire) == 0 { return Vec::new(); }
        let mut stamps: Vec<_> = PHASES.iter().enumerate().map(|(index, name)| (
            index, name, phase_cell(self.page, index, 0).load(Ordering::Acquire),
            phase_cell(self.page, index, 1).load(Ordering::Acquire),
        )).collect();
        // The UI thread can finish before the SMB watcher. Preserve stable phase
        // IDs, but calculate deltas in time order, with unreached stamps last.
        stamps.sort_by_key(|&(index, _, at, _)| (at == 0, at, index));
        let (mut previous_at, mut previous_wait) = (0, 0);
        stamps.into_iter().map(|(index, name, at, total)| {
            let (step, wait) = if at == 0 { (0, 0) } else {
                let step = if previous_at == 0 { 0 } else { at.saturating_sub(previous_at) };
                let wait = total.saturating_sub(previous_wait);
                previous_at = at;
                previous_wait = total;
                (step, wait)
            };
            format!("UI_PHASE {suite} {case} phase={} name={name} at_ms={at} step_ms={step} app_output_ms={wait} app_output_total_ms={total}", index + 1)
        }).collect()
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        PAGE.store(self.previous, Ordering::Release);
        unsafe { munmap(self.page as *mut u8, 4096); }
    }
}
