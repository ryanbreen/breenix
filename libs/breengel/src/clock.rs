//! UI time from the shared BGCLK001 page (BreenGL §9.1).
//!
//! A configured but invalid page is an error, never permission to switch a session
//! to real time. The mapping is retained until the configured path or backing file identity changes.
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
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
struct Page(*mut u8);
// The read-only shared mapping has atomic, naturally aligned u64 fields.
unsafe impl Send for Page {}
unsafe impl Sync for Page {}
impl Drop for Page {
    fn drop(&mut self) { unsafe { munmap(self.0, 64); } }
}
impl Page {
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
    if let Some((old, dev, ino, mapping)) = cached.as_ref() {
        if *old == path && *dev == meta.dev() && *ino == meta.ino() {
            return Some(mapping.clone());
        }
    }
    let file = std::fs::File::open(&path).expect("BREENIX_UI_CLOCK cannot be opened");
    let meta = file.metadata().expect("UI clock metadata");
    assert!(meta.len() >= 64, "short UI clock page");
    // PROT_READ=1, MAP_SHARED=1 on Breenix and the supported Unix hosts.
    let ptr = unsafe { mmap(std::ptr::null_mut(), 64, 1, 1, file.as_raw_fd(), 0) };
    assert!(ptr as isize != -1 && !ptr.is_null(), "UI clock mmap failed");
    let mapping = Arc::new(Page(ptr));
    let magic = unsafe { std::slice::from_raw_parts(ptr, 8) };
    assert_eq!(magic, b"BGCLK001", "invalid UI clock magic");
    *cached = Some((path, meta.dev(), meta.ino(), mapping.clone()));
    Some(mapping)
}
/// Monotonic UI nanoseconds. A stepped session performs no real-clock reads.
pub fn now_ns() -> u64 {
    page().map_or_else(real_now_ns, |p| p.read(16))
}
/// Epoch published by the driver, or zero outside a stepped session.
pub fn epoch() -> u64 { page().map_or(0, |p| p.read(8)) }
/// Wall time follows the page's fixed wall base inside a session.
pub fn wall_ns() -> u64 {
    if let Some(p) = page() { return p.read(24).saturating_add(p.read(16)); }
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
