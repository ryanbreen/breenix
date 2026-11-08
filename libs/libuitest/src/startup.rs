//! Bounded monotonic startup stamps for failure diagnostics.
use std::{fs::OpenOptions, os::{fd::AsRawFd, unix::fs::OpenOptionsExt}, path::Path,
    sync::{OnceLock, atomic::{AtomicU64, Ordering}}};
pub const PHASES: &[&str] = &[
    "connect-begin", "case-mode-ready", "case-ready",
    "supervisor-enter", "supervisor-registered", "supervisor-channel", "supervisor-evidence",
    "supervisor-display", "supervisor-devices", "supervisor-ready",
    "configure-begin", "fixtures-ready", "pins-ready", "arena-ready", "case-connected",
    "compositor-request", "compositor-forked", "compositor-exec", "config-ready",
    "context-begin", "context-ready", "desktop-target-ready", "background-target-ready",
    "scanout-a-ready", "scanout-b-ready", "scene-target-ready", "targets-ready", "fonts-ready", "scene-ready",
    "background-ready", "chrome-blit-ready", "chrome-taskbar-ready", "chrome-appbar-ready", "chrome-ready", "fills-ready", "pointer-presented", "checks-ready",
    "font-plan-begin", "font-plan-return", "font-submit-begin", "font-submit-return", "font-wait-return", "font-draw-return",
    "frame-check-begin", "frame-buffers-ready", "frame-scanout-return", "frame-desktop-return", "frame-pixels-ready",
    "desktop-ready", "app-request", "app-forked", "app-exec", "app-commit", "first-composite",
];
extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}
pub struct Trace(*mut AtomicU64);
// Each slot is published once by atomic compare-exchange in shared mapped memory.
unsafe impl Send for Trace {}
unsafe impl Sync for Trace {}
impl Trace {
    pub fn create(root: &Path) -> Result<Self, String> {
        let file = OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)
            .open(root.join("startup.trace")).map_err(|e| e.to_string())?;
        file.set_len(4096).map_err(|e| e.to_string())?;
        Self::map(&file)
    }
    pub fn open(root: &Path) -> Result<Self, String> {
        let file = OpenOptions::new().read(true).write(true).open(root.join("startup.trace")).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() != 4096 { return Err("invalid startup trace size".into()); }
        Self::map(&file)
    }
    fn map(file: &std::fs::File) -> Result<Self, String> {
        let ptr = unsafe { mmap(core::ptr::null_mut(), 4096, 3, 1, file.as_raw_fd(), 0) };
        if ptr as isize == -1 { return Err(std::io::Error::last_os_error().to_string()); }
        Ok(Self(ptr.cast()))
    }
    pub fn at(&self, name: &str, ns: u64) {
        if let Some(i) = PHASES.iter().position(|&phase| phase == name) {
            let slot = unsafe { &*self.0.add(i) };
            let _ = slot.compare_exchange(0, ns, Ordering::Release, Ordering::Relaxed);
        }
    }
    pub fn mark(&self, name: &str) { self.at(name, now()); }
    pub fn snapshot(&self) -> Vec<(&'static str, u64)> {
        PHASES.iter().enumerate().map(|(i, &name)| (name, unsafe { &*self.0.add(i) }.load(Ordering::Acquire))).collect()
    }
}
impl Drop for Trace { fn drop(&mut self) { unsafe { munmap(self.0.cast(), 4096); } } }
pub fn now() -> u64 { libbreenix::time::now_monotonic().map_or(0, |t| t.as_nanos() as u64) }
/// Process-local handle; no path lookup or allocation at subsequent stamps.
fn configured_trace() -> Option<&'static Trace> {
    static TRACE: OnceLock<Option<Trace>> = OnceLock::new();
    TRACE.get_or_init(|| std::env::var_os("BREENIX_CONFIG_ROOT")
        .and_then(|root| Trace::open(Path::new(&root)).ok())).as_ref()
}
pub fn mark(name: &str) {
    if let Some(trace) = configured_trace() { trace.mark(name); }
}
pub fn at(name: &str, ns: u64) {
    if let Some(trace) = configured_trace() { trace.at(name, ns); }
}
