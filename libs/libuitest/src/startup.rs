//! Bounded monotonic startup stamps for session diagnostics.
use std::{fs::OpenOptions, os::{fd::AsRawFd, unix::fs::OpenOptionsExt}, path::Path,
    sync::{OnceLock, atomic::{AtomicU64, Ordering}}};
pub fn verdict(elapsed_ns: u64, composited: bool) -> bool {
    composited && elapsed_ns <= 2_000_000_000
}

pub const PHASES: &[&str] = &[
    "connect-begin", "case-mode-ready", "case-ready",
    "supervisor-enter", "supervisor-registered", "supervisor-channel", "supervisor-evidence",
    "supervisor-display", "supervisor-devices", "supervisor-ready",
    "configure-begin", "fixtures-ready", "pins-ready", "arena-ready", "case-connected",
    "compositor-request", "compositor-forked", "compositor-exec", "config-ready",
    "context-begin", "context-ready", "desktop-target-ready", "background-target-ready",
    "scanout-a-ready", "scanout-b-ready", "scene-target-ready", "targets-ready", "startup-font-begin", "fonts-ready", "scene-ready",
    "background-ready", "chrome-blit-ready", "chrome-taskbar-ready", "chrome-appbar-ready", "chrome-ready", "fills-ready", "pointer-presented", "checks-ready",
    "font-plan-begin", "font-plan-return", "font-submit-begin", "font-submit-return", "font-wait-return", "font-draw-return",
    "frame-check-begin", "frame-buffers-ready", "frame-scanout-return", "frame-desktop-return", "frame-pixels-ready",
    "initial-windows-ready", "title-plan-begin", "title-plan-return", "client-chrome-ready", "client-compose-begin",
    "font-config-ready", "font-file-ready", "font-parse-ready", "startup-workers-return",
    "desktop-ready", "app-request", "app-forked", "app-exec", "app-commit", "first-composite",
];
extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}
pub struct Trace(*mut AtomicU64);
const TEARDOWN_SLOT: usize = 128;
const CPU_SLOT: usize = 256;
pub const CPU_COSTS: &[&str] = &["turn", "clock", "compose", "check", "turns"];
const TEARDOWN_PHASES: &[&str] = &["unreached", "begin", "children-reaped", "devices-released",
    "display-returned", "root-delete-begin", "root-delete-done", "stopped-sent"];
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
    pub fn cpu_snapshot(&self) -> [u64; 5] {
        core::array::from_fn(|i| unsafe { &*self.0.add(CPU_SLOT + i) }.load(Ordering::Acquire))
    }
    fn cpu_add(&self, slot: usize, value: u64) {
        if slot < CPU_COSTS.len() {
            unsafe { &*self.0.add(CPU_SLOT + slot) }.fetch_add(value, Ordering::Relaxed);
        }
    }
    pub fn teardown_mark(&self, stage: usize) {
        if stage == 0 || stage >= TEARDOWN_PHASES.len() { return; }
        unsafe {
            (&*self.0.add(TEARDOWN_SLOT + stage)).store(now(), Ordering::Relaxed);
            (&*self.0.add(TEARDOWN_SLOT)).store(stage as u64, Ordering::Release);
        }
    }
    pub fn teardown_note(&self) -> String {
        let stage = unsafe { (&*self.0.add(TEARDOWN_SLOT)).load(Ordering::Acquire) } as usize;
        let name = TEARDOWN_PHASES.get(stage).copied().unwrap_or("invalid");
        let at = if stage < TEARDOWN_PHASES.len() {
            unsafe { (&*self.0.add(TEARDOWN_SLOT + stage)).load(Ordering::Relaxed) }
        } else { 0 };
        format!("teardown-stage={name} at={at}")
    }
}
impl Drop for Trace { fn drop(&mut self) { unsafe { munmap(self.0.cast(), 4096); } } }
pub fn now() -> u64 { libbreenix::time::now_monotonic().map_or(0, |t| t.as_nanos() as u64) }
/// Process-local handle; no path lookup or allocation at subsequent stamps.
fn configured_trace() -> Option<&'static Trace> {
    static TRACE: OnceLock<Option<Trace>> = OnceLock::new();
    TRACE.get_or_init(|| {
        if let Some(root) = std::env::var_os("BREENIX_CONFIG_ROOT") {
            return Trace::open(Path::new(&root)).ok();
        }
        let root = std::env::var_os("BREENIX_STARTUP_TRACE_DIR")?;
        let pid = libbreenix::process::getpid().ok()?.raw();
        let path = Path::new(&root).join(pid.to_string());
        std::fs::create_dir_all(&path).ok()?;
        Trace::create(&path).ok()
    }).as_ref()
}
pub fn mark(name: &str) {
    if let Some(trace) = configured_trace() { trace.mark(name); }
}
pub fn at(name: &str, ns: u64) {
    if let Some(trace) = configured_trace() { trace.at(name, ns); }
}
pub fn teardown(stage: usize) {
    if let Some(trace) = configured_trace() { trace.teardown_mark(stage); }
}

/// CPU time, including synchronous kernel work, retained without printing.
pub struct CpuCost { slot: usize, begin: u64 }
fn cpu_ns() -> u64 {
    let mut time = libbreenix::Timespec::new();
    libbreenix::time::clock_gettime(2, &mut time).map_or(0, |()| time.as_nanos() as u64)
}
impl CpuCost {
    pub fn begin(slot: usize) -> Self { Self { slot, begin: cpu_ns() } }
}
impl Drop for CpuCost {
    fn drop(&mut self) {
        if let Some(trace) = configured_trace() {
            trace.cpu_add(self.slot, cpu_ns().saturating_sub(self.begin));
            if self.slot == 0 { trace.cpu_add(4, 1); }
        }
    }
}

/// One bounded line in catalog order, including 0 for an unreached phase.
/// Names come from the fixed phase catalog.
pub fn phase_line(snapshot: &[(&str, u64)]) -> String {
    use std::fmt::Write;
    let mut line = String::from("UITEST_STARTUP v=1");
    for &name in PHASES {
        let ns = snapshot.iter().find(|&&(phase, _)| phase == name).map_or(0, |&(_, ns)| ns);
        write!(&mut line, " {name}={ns}").expect("format startup phase");
    }
    line
}
