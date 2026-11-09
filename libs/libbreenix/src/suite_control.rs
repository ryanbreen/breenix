//! Real-time hang watchdogs and fork-shared case diagnostics.
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
pub const HANG_MS: u64 = 10_000;
#[derive(Clone, Copy)]
pub enum Wait { CompositorRegistration, SupervisorReady, Configure, HoldWindow, InputAck, Stderr, Launch, AppOutput, AppExit, Stopped, ChannelSend, SupervisorTeardown, InputGrab, CollectorReply, DiscardReply, ControllerReply, DiagnosticUpload }
const WAITS: [Wait; 17] = [Wait::CompositorRegistration, Wait::SupervisorReady, Wait::Configure, Wait::HoldWindow, Wait::InputAck, Wait::Stderr, Wait::Launch, Wait::AppOutput, Wait::AppExit, Wait::Stopped, Wait::ChannelSend, Wait::SupervisorTeardown, Wait::InputGrab, Wait::CollectorReply, Wait::DiscardReply, Wait::ControllerReply, Wait::DiagnosticUpload];
const NAMES: [&str; 17] = ["CompositorRegistration", "SupervisorReady", "Configure", "HoldWindow", "InputAck", "Stderr", "Launch", "AppOutput", "AppExit", "Stopped", "ChannelSend", "SupervisorTeardown", "InputGrab", "CollectorReply", "DiscardReply", "ControllerReply", "DiagnosticUpload"];
const CELLS: usize = 6;
static PAGE: AtomicUsize = AtomicUsize::new(0);
fn now_ms() -> Result<u64, String> {
    libbreenix::time::now_monotonic().map(|t| (t.as_nanos()/1_000_000) as u64).map_err(|e| e.to_string())
}
fn cell(page: usize, wait: Wait, field: usize) -> &'static AtomicU64 {
    // The case scope outlives its children and their watchdogs.
    unsafe { &*((page as *const AtomicU64).add(wait as usize * CELLS + field)) }
}
pub struct Watchdog { wait: Wait, start: u64, page: usize }
impl Watchdog {
    pub fn new(wait: Wait) -> Result<Self, String> {
        let start = now_ms()?;
        let page = PAGE.load(Ordering::Acquire);
        if page != 0 {
            cell(page, wait, 0).fetch_add(1, Ordering::Relaxed);
            cell(page, wait, 4).store(start, Ordering::Relaxed);
        }
        Ok(Self { wait, start, page })
    }
    pub fn deadline_ms(&self) -> i128 { i128::from(self.start.saturating_add(HANG_MS)) }
    pub fn deadline_ns(&self) -> u64 { self.start.saturating_add(HANG_MS).saturating_mul(1_000_000) }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        if self.page == 0 { return; }
        match now_ms() {
            Ok(end) if end >= self.start => {
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
pub(super) struct Scope { page: usize, previous: usize }
impl Scope {
    pub(super) fn new() -> Result<Self, String> {
        #[cfg(target_os="macos")] const ANON: i32 = 0x1000;
        #[cfg(not(target_os="macos"))] const ANON: i32 = 0x20;
        assert!(WAITS.len() * CELLS * core::mem::size_of::<AtomicU64>() <= 4096);
        let ptr = unsafe { mmap(core::ptr::null_mut(), 4096, 3, 1 | ANON, -1, 0) };
        if ptr as isize == -1 { return Err("control diagnostics mmap failed".into()); }
        for i in 0..WAITS.len()*CELLS {
            unsafe { ptr.cast::<AtomicU64>().add(i).write(AtomicU64::new(0)); }
        }
        let page = ptr as usize;
        let previous = PAGE.swap(page, Ordering::AcqRel);
        Ok(Self { page, previous })
    }
    pub(super) fn report(&self, suite: &str, case: &str) -> Vec<String> {
        WAITS.iter().enumerate().filter_map(|(i, &wait)| {
            let values: Vec<_> = (0..CELLS).map(|f| cell(self.page, wait, f).load(Ordering::Acquire)).collect();
            (values[0] != 0).then(|| format!("UI_CONTROL {suite} {case} wait={} started={} completed={} total_ms={} max_ms={} last_start_ms={} clock_errors={}", NAMES[i], values[0], values[1], values[2], values[3], values[4], values[5]))
        }).collect()
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        PAGE.store(self.previous, Ordering::Release);
        unsafe { munmap(self.page as *mut u8, 4096); }
    }
}
