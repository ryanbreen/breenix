//! UI time from the shared BGCLK001 page (BreenGL §9.1).
//!
//! A configured but invalid page is an error, never permission to switch a session
//! to real time. The mapping is retained until the configured path or backing file identity changes.
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static REAL_READS: AtomicU64 = AtomicU64::new(0);
static PAGE: Mutex<Option<(PathBuf, u64, u64, Arc<Page>)>> = Mutex::new(None);

extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    fn clock_gettime(clock: i32, result: *mut Timespec) -> i32;
}
#[repr(C)]
struct Timespec { sec: i64, nsec: i64 }
struct Page(*mut u8, usize);
// Shared mappings use atomic, naturally aligned u64 fields.
unsafe impl Send for Page {}
unsafe impl Sync for Page {}
impl Drop for Page {
    fn drop(&mut self) { unsafe { munmap(self.0, self.1); } }
}
impl Page {
    fn word(&self, offset: usize) -> &AtomicU64 {
        unsafe { &*self.0.add(offset).cast::<AtomicU64>() }
    }
    fn read(&self, offset: usize) -> u64 {
        // Header fields are naturally aligned and written atomically by the clock driver.
        unsafe { (&*self.0.add(offset).cast::<AtomicU64>()).load(Ordering::Acquire) }
    }
}
fn page() -> Option<Arc<Page>> {
    let path = std::env::var_os("BREENIX_UI_CLOCK").map(PathBuf::from)?;
    let mut cached = PAGE.lock().expect("UI clock mapping lock");
    // stat does not open a descriptor. An unlinked page remains a valid shared
    // mapping; keep it until this path names a new inode or the session changes.
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some((old, _, _, mapping)) = cached.as_ref() {
                if *old == path { return Some(mapping.clone()); }
            }
            panic!("BREENIX_UI_CLOCK cannot be opened: {e}");
        }
        Err(e) => panic!("UI clock metadata: {e}"),
    };
    assert!(meta.len() >= 64, "short UI clock page");
    if let Some((old, dev, ino, mapping)) = cached.as_ref() {
        if *old == path && *dev == meta.dev() && *ino == meta.ino() {
            return Some(mapping.clone());
        }
    }
    let file = std::fs::File::open(&path).expect("BREENIX_UI_CLOCK cannot be opened");
    let meta = file.metadata().expect("UI clock metadata");
    assert!(meta.len() >= 64, "short UI clock page");
    // PROT_READ=1, MAP_SHARED=1 on Breenix and the supported Unix hosts.
    let len = if meta.len() >= PAGE_BYTES as u64 { PAGE_BYTES } else { 64 };
    let ptr = unsafe { mmap(std::ptr::null_mut(), len, 1, 1, file.as_raw_fd(), 0) };
    assert!(ptr as isize != -1 && !ptr.is_null(), "UI clock mmap failed");
    let mapping = Arc::new(Page(ptr, len));
    let magic = unsafe { std::slice::from_raw_parts(ptr, 8) };
    assert_eq!(magic, b"BGCLK001", "invalid UI clock magic");
    *cached = Some((path, meta.dev(), meta.ino(), mapping.clone()));
    Some(mapping)
}
/// Monotonic UI nanoseconds. A stepped session performs no real-clock reads.
pub fn now_ns() -> u64 {
    page().map_or_else(real_now_ns, |p| p.stamp().now_ns)
}
/// Epoch published by the driver, or zero outside a stepped session.
pub fn epoch() -> u64 { page().map_or(0, |p| p.stamp().epoch) }
/// Wall time follows the page's fixed wall base inside a session.
pub fn wall_ns() -> u64 {
    if let Some(p) = page() { return p.read(24).saturating_add(p.stamp().now_ns); }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock before epoch").as_nanos() as u64
}
/// Explicit real-clock reads are counted while a UI page is configured.
pub fn real_now_ns() -> u64 {
    if std::env::var_os("BREENIX_UI_CLOCK").is_some() {
        REAL_READS.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(target_os = "macos")]
    const MONOTONIC: i32 = 6;
    #[cfg(not(target_os = "macos"))]
    const MONOTONIC: i32 = 1;
    let mut ts = Timespec { sec: 0, nsec: 0 };
    assert_eq!(unsafe { clock_gettime(MONOTONIC, &mut ts) }, 0, "CLOCK_MONOTONIC failed");
    (ts.sec as u64).saturating_mul(1_000_000_000).saturating_add(ts.nsec as u64)
}
/// Number of real-clock reads made by this provider while a page was set.
pub fn real_clock_reads() -> u64 { REAL_READS.load(Ordering::Relaxed) }


/// The original BGCLK001 header occupies bytes 0..64. Extension v1 starts at
/// byte 64: publication sequence, BGPT0001, then 62 64-byte participant slots
/// at byte 128. A slot holds id, next deadline, flags and accepted epoch.
pub const PAGE_BYTES: usize = 4096;
const TABLE: usize = 128;
const SLOT_BYTES: usize = 64;
const SLOTS: usize = (PAGE_BYTES - TABLE) / SLOT_BYTES;
const RESERVED: u64 = u64::MAX;
pub const ANIMATING: u64 = 1;
pub const COMPOSITOR: u64 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stamp { pub epoch: u64, pub now_ns: u64 }

impl Page {
    fn extended(&self) -> bool {
        self.1 == PAGE_BYTES && unsafe { std::slice::from_raw_parts(self.0.add(72), 8) } == b"BGPT0001"
    }
    fn stamp(&self) -> Stamp {
        loop {
            let sequence = if self.extended() { self.read(64) } else { 0 };
            if sequence & 1 != 0 { std::hint::spin_loop(); continue; }
            let epoch = self.read(8);
            let now_ns = self.read(16);
            // Match the writer's SeqCst sequence around the two atomic fields.
            std::sync::atomic::fence(Ordering::SeqCst);
            if (!self.extended() || self.read(64) == sequence) && self.read(8) == epoch {
                return Stamp { epoch, now_ns };
            }
        }
    }
    fn wake_word(&self) -> &AtomicU32 {
        unsafe { &*self.0.add(32).cast::<AtomicU32>() }
    }
}

/// A consistent render stamp. Outside a session the epoch is 0.
pub fn stamp() -> Stamp {
    page().map_or_else(|| Stamp { epoch: 0, now_ns: real_now_ns() }, |p| p.stamp())
}
pub fn configured() -> bool { std::env::var_os("BREENIX_UI_CLOCK").is_some() }

#[derive(Clone)]
pub struct ClockPage(Arc<Page>);
#[derive(Clone, Copy, Debug)]
pub struct ParticipantState {
    pub id: u64, pub next_deadline_ns: u64, pub flags: u64, pub acknowledged: u64,
}
impl ClockPage {
    pub fn create(path: &std::path::Path, wall_base_ns: u64) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let mut bytes = [0; PAGE_BYTES];
        bytes[..8].copy_from_slice(b"BGCLK001");
        bytes[24..32].copy_from_slice(&wall_base_ns.to_le_bytes());
        bytes[72..80].copy_from_slice(b"BGPT0001");
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        // Session children can run as uid 1000 and register their own windows.
        file.set_permissions(std::fs::Permissions::from_mode(0o666)).map_err(|e| e.to_string())
    }
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| e.to_string())?;
        if file.metadata().map_err(|e| e.to_string())?.len() != PAGE_BYTES as u64 {
            return Err("UI clock participant page has wrong size".into());
        }
        let ptr = unsafe { mmap(std::ptr::null_mut(), PAGE_BYTES, 3, 1, file.as_raw_fd(), 0) };
        if ptr as isize == -1 || ptr.is_null() { return Err("UI clock participant mmap failed".into()); }
        let page = Arc::new(Page(ptr, PAGE_BYTES));
        if unsafe { std::slice::from_raw_parts(ptr, 8) } != b"BGCLK001" || !page.extended() {
            return Err("invalid UI clock participant page".into());
        }
        Ok(Self(page))
    }
    pub fn stamp(&self) -> Stamp { self.0.stamp() }
    pub fn participants(&self) -> Vec<ParticipantState> {
        (0..SLOTS).filter_map(|slot| {
            let offset = TABLE + slot * SLOT_BYTES;
            let id = self.0.read(offset);
            if id == 0 || id == RESERVED { return None; }
            let state = ParticipantState { id, next_deadline_ns: self.0.read(offset + 8),
                flags: self.0.read(offset + 16), acknowledged: self.0.read(offset + 24) };
            (self.0.read(offset) == id).then_some(state)
        }).collect()
    }
    pub fn register(&self, id: u64) -> Result<Participant, String> {
        if id == 0 || id == RESERVED { return Err("invalid UI participant id".into()); }
        for slot in 0..SLOTS {
            let offset = TABLE + slot * SLOT_BYTES;
            if self.0.word(offset).compare_exchange(0, RESERVED, Ordering::AcqRel, Ordering::Acquire).is_err() { continue; }
            self.0.word(offset + 8).store(u64::MAX, Ordering::Relaxed);
            self.0.word(offset + 16).store(0, Ordering::Relaxed);
            self.0.word(offset + 24).store(0, Ordering::Relaxed);
            self.0.word(offset).store(id, Ordering::Release);
            return Ok(Participant { page: self.clone(), offset });
        }
        Err("UI clock participant table full".into())
    }
    /// One session driver publishes time, then the epoch, inside the extension's
    /// sequence. The header layout and its reserved bytes are unchanged.
    pub fn advance(&self, dt_ms: u64) -> Result<Stamp, String> {
        let sequence = self.0.read(64);
        if sequence & 1 != 0 || self.0.word(64).compare_exchange(sequence, sequence.wrapping_add(1), Ordering::SeqCst, Ordering::Acquire).is_err() {
            return Err("UI clock driver busy".into());
        }
        let result = (|| {
            let now_ns = dt_ms.checked_mul(1_000_000).and_then(|dt| self.0.read(16).checked_add(dt)).ok_or("UI clock time overflow")?;
            let epoch = self.0.read(8).checked_add(1).ok_or("UI clock epoch overflow")?;
            self.0.word(16).store(now_ns, Ordering::Release);
            self.0.word(8).store(epoch, Ordering::Release);
            Ok(Stamp { epoch, now_ns })
        })();
        self.0.word(64).store(sequence.wrapping_add(2), Ordering::SeqCst);
        self.0.wake_word().fetch_add(1, Ordering::Release);
        self.wake();
        result
    }
    pub fn wake_generation(&self) -> u32 { self.0.wake_word().load(Ordering::Acquire) }
    fn wake(&self) {
        #[cfg(target_os = "breenix")]
        unsafe {
            crate::syscall::raw::syscall6(crate::syscall::nr::FUTEX, self.0.wake_word() as *const _ as u64,
                1, i32::MAX as u64, 0, 0, 0);
        }
    }
    /// The timed wait also allows a window to drain input events: it cannot wait
    /// indefinitely on both the input queue and the clock futex at once.
    pub fn wait(&self, generation: u32, timeout_ns: u64) {
        if timeout_ns == 0 || self.wake_generation() != generation { return; }
        #[cfg(target_os = "breenix")]
        {
            let timeout = Timespec { sec: (timeout_ns / 1_000_000_000) as i64, nsec: (timeout_ns % 1_000_000_000) as i64 };
            unsafe { crate::syscall::raw::syscall6(crate::syscall::nr::FUTEX,
                self.0.wake_word() as *const _ as u64, 0, generation as u64, &timeout as *const _ as u64, 0, 0); }
        }
        #[cfg(not(target_os = "breenix"))]
        std::thread::sleep(std::time::Duration::from_nanos(timeout_ns));
    }
}

pub struct Participant { page: ClockPage, offset: usize }
impl Participant {
    pub fn configured(id: u64) -> Result<Option<Self>, String> {
        let Some(path) = std::env::var_os("BREENIX_UI_CLOCK") else { return Ok(None); };
        ClockPage::open(std::path::Path::new(&path))?.register(id).map(Some)
    }
    pub fn schedule(&self, next_deadline_ns: u64, animating: bool) {
        self.page.0.word(self.offset + 16).store(if animating { ANIMATING } else { 0 }, Ordering::Release);
        self.page.0.word(self.offset + 8).store(next_deadline_ns, Ordering::Release);
    }
    pub fn accepted(&self, stamp: Stamp) {
        self.page.0.word(self.offset + 24).store(stamp.epoch, Ordering::Release);
    }
    pub fn page(&self) -> &ClockPage { &self.page }
}
impl Drop for Participant {
    fn drop(&mut self) { self.page.0.word(self.offset).store(0, Ordering::Release); }
}
