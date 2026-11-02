//! Whole-scanout checkpoints: observation, canonical hashing and digest transport.
use crate::gpu::{self, abi::{record as r, Mode}};
use std::{string::String, vec::Vec};
use std::fmt::Write;
use std::sync::{Mutex, atomic::{AtomicBool, AtomicU64, AtomicPtr, Ordering}};

fn word(rec: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(rec[at..at + 4].try_into().unwrap())
}
fn quad(rec: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(rec[at..at + 8].try_into().unwrap())
}

pub(super) fn validate(rec: &[u8; r::SIZE], mode: Mode, backend: u32, runner: u64) -> Result<(), &'static str> {
    if word(rec, r::VERSION_OFF) != r::VERSION || word(rec, r::SIZE_OFF) as usize != r::SIZE {
        return Err("record-version");
    }
    if mode.width == 0 || mode.height == 0
        || (word(rec, r::WIDTH), word(rec, r::HEIGHT)) != (mode.width, mode.height)
        || word(rec, r::TARGET_WIDTH) < mode.width || word(rec, r::TARGET_HEIGHT) < mode.height
        || word(rec, r::X) != 0 || word(rec, r::Y) != 0 || quad(rec, r::TARGET) != 0 {
        return Err("not-whole-mode");
    }
    if word(rec, r::FLAGS) & 1 == 0 || quad(rec, r::PRESENT_FENCE) == 0
        || word(rec, r::UNWRITTEN) != 0 || word(rec, r::BACKEND) != backend {
        return Err("invalid-scanout");
    }
    // Native records name one shared hardware channel. Its resource identifies
    // the runner surface; snapshot and reuse exclude that resource separately.
    if quad(rec, r::CONTEXT) == 0 || (backend != gpu::abi::backend::GK208 && quad(rec, r::CONTEXT) == runner) {
        return Err("runner-context");
    }
    Ok(())
}

pub(super) fn canonical_hash(pixels: &mut [u8], mode: Mode) -> Result<[u8; 32], &'static str> {
    let len = (mode.width as usize).checked_mul(mode.height as usize).and_then(|n| n.checked_mul(4));
    if len != Some(pixels.len()) || pixels.is_empty() { return Err("pixel-length"); }
    for pixel in pixels.chunks_exact_mut(4) { pixel[3] = 255; }
    Ok(libpixcheck::sha256(pixels))
}

pub(super) fn named(name: &str) -> bool {
    !matches!(name, "end" | "intent") && name.split('-').all(|word|
        !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase()))
}

pub(super) fn format_record(suite: &str, case: &str, name: &str, result: Result<([u8; 32], Mode, Mode), &str>) -> String {
    let mut line = std::format!("SUITE {suite} SCREEN {case} {name}");
    match result {
        Ok((hash, mode, binding)) => {
            line.push_str(" sha256=");
            for byte in hash { let _ = write!(line, "{byte:02x}"); }
            let _ = write!(line, " mode={}x{} binding={}x{}", mode.width, mode.height, binding.width, binding.height);
        }
        Err(reason) => {
            if let Some(detail) = reason.strip_prefix("unobservable reason=") {
                if name == "end" { let _ = write!(line, " {reason}"); }
                else {
                    let fields = detail.split_once(' ').map_or("", |(_, fields)| fields);
                    let _ = write!(line, " error=readback-failed {fields}");
                }
            } else { let _ = write!(line, " error={reason}"); }
        }
    }
    line.push('\n');
    line
}

pub(super) fn pack(body: &str) -> Vec<u8> {
    let mut bytes = libpixcheck::sha256(body.as_bytes()).to_vec();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

fn detail_fields(errno: &str, reason: &str, failed: &str) -> bool {
    errno.strip_prefix("errno=").is_some_and(|n| n.parse::<i32>().is_ok_and(|n| n > 0))
        && reason.strip_prefix("eio_reason=").is_some_and(|n| n.parse::<u32>().is_ok())
        && failed.strip_prefix("failed_submission=").is_some_and(|n| n.parse::<u64>().is_ok())
}

pub(super) fn unpack<'a>(bytes: &'a [u8], suite: &str, case: &str) -> Result<&'a str, &'static str> {
    if bytes.len() < 32 || libpixcheck::sha256(&bytes[32..]) != bytes[..32] { return Err("side-file-hash"); }
    let body = std::str::from_utf8(&bytes[32..]).map_err(|_| "side-file-text")?;
    let mut names = Vec::new();
    for line in body.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        let ["SUITE", id, "SCREEN", owner, name, fields @ ..] = words.as_slice() else { return Err("side-file-record"); };
        if *id != suite || *owner != case || (!named(name) && *name != "end") || names.contains(name) {
            return Err("side-file-case");
        }
        match fields {
            [hash, mode, binding] => {
                let hash = hash.strip_prefix("sha256=").ok_or("side-file-digest")?;
                if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
                    return Err("side-file-digest");
                }
                let dimensions = |field: &str, prefix: &str| -> Result<(u32, u32), &'static str> {
                    let (w, h) = field.strip_prefix(prefix).and_then(|v| v.split_once('x')).ok_or("side-file-mode")?;
                    let w = w.parse::<u32>().ok().filter(|&v| v != 0).ok_or("side-file-mode")?;
                    let h = h.parse::<u32>().ok().filter(|&v| v != 0).ok_or("side-file-mode")?;
                    Ok((w, h))
                };
                let (w, h) = dimensions(mode, "mode=")?;
                let (bw, bh) = dimensions(binding, "binding=")?;
                if bw < w || bh < h { return Err("side-file-mode"); }
            }
            [error] if error.strip_prefix("error=").is_some_and(named) => {}
            [error, errno, reason, failed] if error.strip_prefix("error=").is_some_and(named)
                && detail_fields(errno, reason, failed) => {}
            ["unobservable", reason, errno, eio, failed] if *name == "end"
                && matches!(*reason, "reason=device-reset" | "reason=target-invalidated")
                && detail_fields(errno, eio, failed) => {}
            _ => return Err("side-file-fields"),
        }
        names.push(*name);
    }
    Ok(body)
}

/// A shared clock lets the parent timeout and child report exclude the same interval.
#[repr(C)]
pub(super) struct CheckpointClock {
    sequence: AtomicU64,
    excluded: AtomicU64,
    active: AtomicU64,
    window_excluded: AtomicU64,
}
impl CheckpointClock {
    pub(super) const fn new() -> Self {
        Self { sequence: AtomicU64::new(0), excluded: AtomicU64::new(0), active: AtomicU64::new(0), window_excluded: AtomicU64::new(0) }
    }
    pub(super) fn start(&self, now: u64) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.active.store(now, Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }
    pub(super) fn stop(&self, now: u64) {
        self.sequence.fetch_add(1, Ordering::SeqCst);
        let start = self.active.swap(0, Ordering::SeqCst);
        self.excluded.fetch_add(now.saturating_sub(start), Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }
    fn excluded_at(&self, now: u64) -> u64 {
        for _ in 0..8 {
            let sequence = self.sequence.load(Ordering::SeqCst);
            if sequence & 1 != 0 { continue; }
            let excluded = self.excluded.load(Ordering::SeqCst);
            let active = self.active.load(Ordering::SeqCst);
            if self.sequence.load(Ordering::SeqCst) == sequence {
                return excluded.saturating_add(if active == 0 { 0 } else { now.saturating_sub(active) });
            }
        }
        // The writer may have died between sequence updates. Read the completed exclusion conservatively.
        self.excluded.load(Ordering::SeqCst)
    }
    pub(super) fn elapsed(&self, start: u64, now: u64) -> u64 {
        self.elapsed_from(start, now, 0)
    }
    pub(super) fn elapsed_from(&self, start: u64, now: u64, excluded_before: u64) -> u64 {
        now.saturating_sub(start).saturating_sub(self.excluded_at(now).saturating_sub(excluded_before))
    }
    pub(super) fn overdue(&self, now: u64) -> bool {
        let active = self.active.load(Ordering::SeqCst);
        active != 0 && now.saturating_sub(active) >= 120_000_000_000
    }
}

pub(super) struct ClockMap(*mut CheckpointClock);
impl Drop for ClockMap {
    fn drop(&mut self) {
        CLOCK.store(core::ptr::null_mut(), Ordering::Relaxed);
        if !self.0.is_null() { let _ = libbreenix::memory::munmap(self.0.cast(), 4096); }
    }
}

type Snapshot = Result<([u8; 32], Mode, Mode), String>;
#[derive(Default)]
pub(super) struct EndCheckpoint { pub(super) result: Option<Snapshot> }
impl EndCheckpoint {
    pub(super) fn before_drop(&mut self, observe: impl FnOnce() -> Snapshot) {
        let result = observe();
        // No binding owned by the case is not an observation failure.
        if !matches!(&result, Err(e) if e == "no-case-scanout") { self.result = Some(result); }
    }
}
static END: Mutex<EndCheckpoint> = Mutex::new(EndCheckpoint { result: None });
static CLOCK: AtomicPtr<CheckpointClock> = AtomicPtr::new(core::ptr::null_mut());
static CASE_PID: AtomicU64 = AtomicU64::new(0);
static ENABLED: AtomicBool = AtomicBool::new(false);
static RUNNER: AtomicU64 = AtomicU64::new(0);
static RUNNER_RESOURCE: AtomicU64 = AtomicU64::new(0);
static LINES: Mutex<String> = Mutex::new(String::new());

pub(super) fn configured(suite: &str, option: &str) -> bool {
    matches!(suite, "gpu" | "breengel") && option.trim() != "off"
}

pub(super) fn begin(suite: &str) -> ClockMap {
    static OPTION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let option = OPTION.get_or_init(|| std::fs::read_to_string("/proc/breenix/screen-checkpoints").unwrap_or_default());
    ENABLED.store(configured(suite, option), Ordering::Relaxed);
    if !ENABLED.load(Ordering::Relaxed) { return ClockMap(core::ptr::null_mut()); }
    let ptr = libbreenix::memory::mmap(core::ptr::null_mut(), 4096,
        libbreenix::memory::PROT_READ | libbreenix::memory::PROT_WRITE,
        libbreenix::memory::MAP_SHARED | libbreenix::memory::MAP_ANONYMOUS, -1, 0)
        .map_or(core::ptr::null_mut(), |p| p.cast::<CheckpointClock>());
    if !ptr.is_null() {
        // A private mapping owned until the parent finishes this case; atomics are shared by fork.
        unsafe { ptr.write(CheckpointClock::new()); }
    }
    CLOCK.store(ptr, Ordering::Relaxed);
    let mut pixel = [0; 4];
    let mut rec = [0; r::SIZE];
    let status = gpu::readback(0, [0, 0, 1, 1], &mut pixel, 4, &mut rec);
    RUNNER.store(if status == 0 { quad(&rec, r::CONTEXT) } else { 0 }, Ordering::Relaxed);
    RUNNER_RESOURCE.store(if status == 0 { quad(&rec, r::RESOURCE) } else { 0 }, Ordering::Relaxed);
    LINES.lock().unwrap().clear();
    END.lock().unwrap().result = None;
    *REUSE.lock().unwrap() = None;
    *CAPTURE_PIXELS.lock().unwrap() = None;
    let _ = std::fs::remove_file(std::format!("{}.screen", super::judged_path()));
    ClockMap(ptr)
}

pub(super) fn enter_case() {
    CASE_PID.store(crate::process::getpid().map_or(0, |p| p.raw()) as u64, Ordering::Relaxed);
}
fn in_case() -> bool {
    ENABLED.load(Ordering::Relaxed) && CASE_PID.load(Ordering::Relaxed) != 0
        && crate::process::getpid().map_or(0, |p| p.raw()) as u64 == CASE_PID.load(Ordering::Relaxed)
}
fn now_ns() -> u64 { super::monotonic_ns().unwrap_or(0).max(0) as u64 }
fn timed<R>(f: impl FnOnce() -> R) -> R {
    let clock = CLOCK.load(Ordering::Relaxed);
    // A missing clock must not charge an observation to the case.
    assert!(!clock.is_null());
    // The mapping stays live throughout run_case, including its child and observations.
    let clock = unsafe { &*clock };
    clock.start(now_ns());
    let result = f();
    clock.stop(now_ns());
    result
}
pub(super) fn elapsed_ns(start: u64, now: u64) -> u64 {
    let clock = CLOCK.load(Ordering::Relaxed);
    if clock.is_null() { now.saturating_sub(start) } else { unsafe { &*clock }.elapsed(start, now) }
}

pub(super) fn excluded_ns(now: u64) -> u64 {
    let clock = CLOCK.load(Ordering::Relaxed);
    if clock.is_null() { 0 } else { unsafe { &*clock }.excluded_at(now) }
}
pub(super) fn start_window(now: u64) {
    let clock = CLOCK.load(Ordering::Relaxed);
    if !clock.is_null() {
        let clock = unsafe { &*clock };
        clock.window_excluded.store(clock.excluded_at(now), Ordering::SeqCst);
    }
}
pub(super) fn window_excluded() -> u64 {
    let clock = CLOCK.load(Ordering::Relaxed);
    if clock.is_null() { 0 } else { unsafe { &*clock }.window_excluded.load(Ordering::SeqCst) }
}
pub(super) fn overdue(now: u64) -> bool {
    let clock = CLOCK.load(Ordering::Relaxed);
    !clock.is_null() && unsafe { &*clock }.overdue(now)
}

/// Called by a helper's destructor before killing its display owner. Its return type carries no verdict.
pub(super) fn before_helper_drop() {
    if !in_case() { return; }
    if CLOCK.load(Ordering::Relaxed).is_null() {
        END.lock().unwrap().result = Some(Err("clock-allocation".into()));
        return;
    }
    timed(|| END.lock().unwrap().before_drop(snapshot));
}

/// An exact whole-mode judged readback; only binding metadata may be refreshed.
pub(super) struct ReadbackReuse {
    pub(super) hash: [u8; 32],
    mode: Mode,
    record: [u8; r::SIZE],
}
impl ReadbackReuse {
    pub(super) fn new(pixels: &mut [u8], rec: &[u8; r::SIZE], mode: Mode, backend: u32, runner: u64) -> Result<Self, &'static str> {
        validate(rec, mode, backend, runner)?;
        Ok(Self { hash: canonical_hash(pixels, mode)?, mode, record: *rec })
    }
    pub(super) fn matches(&self, rec: &[u8; r::SIZE]) -> bool {
        [r::BACKEND, r::X, r::Y, r::WIDTH, r::HEIGHT, r::TARGET_WIDTH, r::TARGET_HEIGHT].iter()
            .all(|&at| word(&self.record, at) == word(rec, at))
            && [r::CONTEXT, r::RESOURCE, r::PRESENT_FENCE, r::GENERATION, r::RENDER_SERIAL, r::RENDER_FENCE].iter()
                .all(|&at| quad(&self.record, at) == quad(rec, at))
    }
}
struct PendingReadback { pixels: Vec<u8>, record: [u8; r::SIZE], mode: Mode, backend: u32 }
static CAPTURE_PIXELS: Mutex<Option<Vec<u8>>> = Mutex::new(None);
static REUSE: Mutex<Option<PendingReadback>> = Mutex::new(None);

/// Called only after the existing comparator has judged this exact readback.
pub(super) fn judged_readback(pixels: &[u8], rec: &[u8; r::SIZE]) {
    if !in_case() || CLOCK.load(Ordering::Relaxed).is_null() { return; }
    // Keep reuse preparation outside performance cases.
    if super::running_case().is_some_and(|n| n.starts_with("performance/")) { return; }
    let mode = Mode { width: word(rec, r::TARGET_WIDTH), height: word(rec, r::TARGET_HEIGHT), scale: 1, reserved: 0 };
    let backend = word(rec, r::BACKEND);
    if validate(rec, mode, backend, RUNNER.load(Ordering::Relaxed)).is_err()
        || non_case_binding(true, quad(rec, r::RESOURCE), 0, RUNNER_RESOURCE.load(Ordering::Relaxed))
        || pixels.len() != mode.width as usize * mode.height as usize * 4 { return; }
    timed(|| {
        *REUSE.lock().unwrap() = Some(PendingReadback { pixels: pixels.to_vec(), record: *rec, mode, backend });
    });
}

fn reuse_snapshot() -> Option<Snapshot> {
    let pending = REUSE.lock().unwrap().take()?;
    let mode = gpu::current_mode().ok()?;
    if (mode.width, mode.height) != (pending.mode.width, pending.mode.height) { return None; }
    let mut probe = [0; 4];
    let mut record = [0; r::SIZE];
    if gpu::readback(0, [0, 0, 1, 1], &mut probe, 4, &mut record) != 0 { return None; }
    // The small read supplies binding identities, not the picture or its geometry.
    record[r::WIDTH..r::WIDTH+4].copy_from_slice(&pending.mode.width.to_le_bytes());
    record[r::HEIGHT..r::HEIGHT+4].copy_from_slice(&pending.mode.height.to_le_bytes());
    let mut pixels = pending.pixels;
    let saved = ReadbackReuse::new(&mut pixels, &pending.record, pending.mode, pending.backend, RUNNER.load(Ordering::Relaxed)).ok()?;
    let binding = Mode { width: word(&record, r::TARGET_WIDTH), height: word(&record, r::TARGET_HEIGHT), scale: 1, reserved: 0 };
    let hash = saved.hash;
    if !saved.matches(&record) { return None; }
    if capture_enabled() { *CAPTURE_PIXELS.lock().unwrap() = Some(pixels); }
    Some(Ok((hash, saved.mode, binding)))
}

pub(super) fn non_case_binding(known: bool, resource: u64, console: u64, runner: u64) -> bool {
    known && (resource == 0 || resource == console || resource == runner)
}

/// Classify only facts established by the endpoint; an error during a transfer
/// (including DEVICE_RESET) remains an observation defect, not a declaration.
pub(super) fn endpoint_error(errno: i32, record: &[u8]) -> String {
    if i64::from(errno) == gpu::abi::screen::NO_CASE { return "no-case-scanout".into(); }
    let failure = gpu::abi::screen::Failure::decode(record).filter(|f| f.errno == i64::from(errno));
    let (reason, failed) = failure.map_or((0, 0), |f| (f.reason, f.failed_submission));
    let declared = failure.is_some_and(|f| f.declared == 1);
    let state = match (errno, reason, failed, declared) {
        (5, gpu::abi::readback_reason::TARGET_INVALID, 1.., true) => "unobservable reason=target-invalidated",
        (5, gpu::abi::readback_reason::TARGET_LOST, _, true) => "unobservable reason=device-reset",
        _ => "readback-failed",
    };
    std::format!("{state} errno={errno} eio_reason={reason} failed_submission={failed}")
}

fn snapshot() -> Snapshot {
    let (status, modes) = gpu::modes();
    if status != 0 { return Err("mode-failed".into()); }
    // An in-flight PRESENT may make MODES transiently unknown. Only the pinned
    // observer settles that transition; unknown does not identify a non-case frame.
    if non_case_binding(modes.binding_known != 0, modes.scanout_resource, modes.console_resource,
        RUNNER_RESOURCE.load(Ordering::Relaxed)) {
        return Err("no-case-scanout".into());
    }
    if let Some(saved) = reuse_snapshot() { return saved; }
    if RUNNER.load(Ordering::Relaxed) == 0 { return Err("runner-unknown".into()); }
    let (status, caps) = gpu::caps();
    if status != 0 { return Err("caps-failed".into()); }
    let mode = gpu::current_mode().map_err(|_| "mode-failed")?;
    let len = (mode.width as usize).checked_mul(mode.height as usize).and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= i32::MAX as usize).ok_or("mode-size")?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(len).map_err(|_| "allocation-failed")?;
    pixels.resize(len, 0);
    let mut rec = [0u8; r::SIZE];
    use std::io::Write;
    let request = gpu::abi::screen::Request { pixels: pixels.as_mut_ptr() as u64, len: pixels.len() as u64,
        record: rec.as_mut_ptr() as u64 }.encode();
    let mut device = std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence").map_err(|_| "readback-open")?;
    device.write_all(&request).map_err(|error| endpoint_error(error.raw_os_error().unwrap_or(0), &rec))?;
    if (caps.backend != gpu::abi::backend::GK208 && quad(&rec, r::CONTEXT) == RUNNER.load(Ordering::Relaxed))
        || non_case_binding(true, quad(&rec, r::RESOURCE), modes.console_resource,
            RUNNER_RESOURCE.load(Ordering::Relaxed)) { return Err("no-case-scanout".into()); }
    validate(&rec, mode, caps.backend, RUNNER.load(Ordering::Relaxed))?;
    let binding = Mode { width: word(&rec, r::TARGET_WIDTH), height: word(&rec, r::TARGET_HEIGHT), scale: 1, reserved: 0 };
    let hash = canonical_hash(&mut pixels, mode)?;
    if capture_enabled() { *CAPTURE_PIXELS.lock().unwrap() = Some(pixels); }
    Ok((hash, mode, binding))
}

fn append(name: &str, end: bool) -> Result<(), &'static str> {
    let case = super::running_case().ok_or("outside-case")?;
    let (prefix, _) = super::RECORD_NAMES.get().ok_or("outside-case")?;
    let suite = prefix.strip_prefix("SUITE ").and_then(|s| s.strip_suffix(' ')).ok_or("outside-case")?;
    let mut result = if CLOCK.load(Ordering::Relaxed).is_null() {
        Err("clock-allocation".into())
    } else if end {
        match snapshot() {
            Err(e) if e == "no-case-scanout" => END.lock().unwrap().result.take().unwrap_or(Err(e)),
            result => result,
        }
    } else { timed(snapshot) };
    if end && matches!(&result, Err(e) if e == "no-case-scanout") { return Ok(()); }
    if let Ok((hash, mode, _)) = &result {
        if capture_enabled() {
            let pixels = CAPTURE_PIXELS.lock().unwrap().take();
            if let Err(e) = pixels.ok_or("capture-pixels").and_then(|pixels|
                capture(suite, case, name, *mode, *hash, &pixels)) { result = Err(e.into()); }
        }
    }
    let line = format_record(suite, case, name, result.as_ref().map(|v| *v).map_err(String::as_str));
    let mut lines = LINES.lock().unwrap();
    if lines.lines().any(|line| line.split_whitespace().nth(4) == Some(name)) { return Err("duplicate-checkpoint"); }
    lines.push_str(&line);
    Ok(())
}

pub(super) fn checkpoint(name: &str) -> super::CaseResult {
    if !ENABLED.load(Ordering::Relaxed) { return Ok(()); }
    super::check(named(name), "invalid or reserved screen checkpoint name")?;
    super::check(!super::running_case().is_some_and(|case| case.starts_with("performance/")),
        "performance cases cannot declare screen checkpoints")?;
    append(name, false).map_err(|e| super::CaseError::Fail(e.into()))
}

pub(super) fn finish() {
    if !ENABLED.load(Ordering::Relaxed) { return; }
    if !in_case() { return; }
    let publish = || {
        let _ = append("end", true);
        let lines = LINES.lock().unwrap();
        // A missing or corrupt handoff is an additive SCREEN error, without changing CASE.
        let path = std::format!("{}.screen", super::judged_path());
        let _ = std::fs::write(path, pack(&lines));
    };
    if CLOCK.load(Ordering::Relaxed).is_null() { publish(); } else { timed(publish); }
}

pub(super) fn collect(suite: &str, case: &str) -> Vec<String> {
    if !ENABLED.load(Ordering::Relaxed) { return Vec::new(); }
    let path = std::format!("{}.screen", super::judged_path());
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return vec![format_record(suite, case, "end", Err("side-file-read"))],
    };
    let _ = std::fs::remove_file(path);
    match unpack(&bytes, suite, case) {
        Ok(body) => body.lines().map(String::from).collect(),
        Err(e) => vec![format_record(suite, case, "end", Err(e))],
    }
}

/// Send records on the evidence path, independently of stdout and the controlling VT.
pub(super) fn emit(line: &str) {
    use std::io::Write;
    if let Ok(mut evidence) = std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence") {
        let _ = evidence.write_all(std::format!("\n{line}\n").as_bytes());
    }
}


fn capture_enabled() -> bool {
    static SETTING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SETTING.get_or_init(|| std::fs::read_to_string("/etc/breenix/screen-capture")
        .is_ok_and(|s| s.trim() == "on"))
}

pub(super) fn capture(suite: &str, case: &str, checkpoint: &str, mode: Mode,
    digest: [u8;32], pixels: &[u8]) -> Result<(), &'static str> {
    if !capture_enabled() { return Ok(()); }
    use std::io::Write;
    let bytes = screen_capture::encode(&screen_capture::Record {
        suite,case,checkpoint,width: mode.width,height: mode.height,digest,pixels })?;
    let mut request = [0;24];
    request[..8].copy_from_slice(screen_capture::REQUEST_MAGIC);
    request[8..16].copy_from_slice(&(bytes.as_ptr() as u64).to_le_bytes());
    request[16..24].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
    std::fs::OpenOptions::new().write(true).open("/dev/screen-evidence")
        .map_err(|_| "capture-open")?.write_all(&request).map_err(|_| "capture-write")
}
