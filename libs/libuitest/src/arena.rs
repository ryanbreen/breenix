//! One-shot evidence slots shared by the case and its sibling supervisor (D7).
use std::{fs::{File, OpenOptions}, os::{fd::AsRawFd, unix::fs::OpenOptionsExt}, path::Path,
    sync::atomic::{AtomicU32, Ordering}};
use libpixcheck::sha256;
const PAGE: usize = 4096;
const HEADER: usize = 4096;
pub const TREE_BYTES: usize = 1024 * 1024;
const FREE: u32 = 0;
const WRITING: u32 = 1;
const FINAL: u32 = 2;
#[derive(Clone, Copy, Debug)]
pub enum Slot { Screen, Frame, Tree }
impl Slot { fn index(self) -> usize { match self { Self::Screen => 0, Self::Frame => 1, Self::Tree => 2 } } }
#[derive(Clone, Copy, Debug)]
pub struct Layout { pub width: u32, pub height: u32, pixels: usize }
impl Layout {
    pub fn for_mode(width: u32, height: u32) -> Result<Self, String> {
        if width == 0 || height == 0 || width > 2560 || height > 1600 { return Err("arena mode outside SCR1 limit".into()); }
        Ok(Self { width, height, pixels: width as usize * height as usize * 4 })
    }
    pub fn capacity(self, slot: Slot) -> usize { if matches!(slot, Slot::Tree) { TREE_BYTES } else { self.pixels } }
    fn header_offset(self, slot: Slot) -> usize { PAGE + slot.index() * (HEADER + self.pixels).next_multiple_of(PAGE) }
    pub fn data_offset(self, slot: Slot) -> usize { self.header_offset(slot) + HEADER }
    pub fn len(self) -> usize { (self.data_offset(Slot::Tree) + TREE_BYTES).next_multiple_of(PAGE) }
}
/// Metadata is serialized before the state is published. Hash covers the retained bytes.
#[derive(Clone, Debug, Default)]
pub struct Record {
    pub check: String, pub window: u64, pub commit: u64, pub epoch: u64,
    pub composite: u64, pub width: u32, pub height: u32, pub oversize: bool,
    pub observation: Vec<u8>,
}
#[derive(Debug)]
pub enum SlotRead { Free, Partial(&'static str), Final { record: Record, bytes: Vec<u8> } }
extern "C" {
    fn getpagesize() -> i32;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}
// One fixed table per completed next_checkpoint, emitted before its caller can
// time out. Nanoseconds are elapsed/thread-CPU; unsupported CPU clocks are
// explicit and absent calls are '-'. No path or other unbounded text is printed.
const CALLS: [&str; 14] = [
    "open", "set_len", "header_write", "mmap_screen", "mmap_frame", "mmap_tree",
    "rename", "close", "munmap_header_screen", "munmap_header_frame", "munmap_header_tree",
    "munmap_payload_screen", "munmap_payload_frame", "munmap_payload_tree",
];
#[repr(C)]
struct CpuTimespec { sec: i64, nsec: i64 }
extern "C" {
    fn clock_gettime(clock: i32, time: *mut CpuTimespec) -> i32;
}
fn thread_cpu_ns() -> Option<u64> {
    // The Breenix ABI uses 3, as does Linux; Darwin's host-check ABI uses 16.
    let clock = if cfg!(target_os = "macos") { 16 } else { 3 };
    let mut time = CpuTimespec { sec: 0, nsec: 0 };
    if unsafe { clock_gettime(clock, &mut time) } != 0 { return None; }
    u64::try_from(time.sec).ok()?.checked_mul(1_000_000_000)?
        .checked_add(u64::try_from(time.nsec).ok()?)
}
struct CallTimes { enabled: bool, times: [Option<(u64, Option<u64>)>; 14] }
impl CallTimes {
    fn new(enabled: bool) -> Self { Self { enabled, times: [None; 14] } }
    fn call<T>(&mut self, index: usize, call: impl FnOnce() -> T) -> T {
        if !self.enabled { return call(); }
        let start = std::time::Instant::now();
        let cpu_start = thread_cpu_ns();
        let result = call();
        let cpu_end = thread_cpu_ns();
        let elapsed = start.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        let cpu = cpu_start.zip(cpu_end).and_then(|(start, end)| end.checked_sub(start));
        self.times[index] = Some((elapsed, cpu));
        result
    }
    fn report(&self, ok: bool) {
        use std::io::Write;
        use std::fmt::Write as _;
        let mut line = format!("VALUE arena-call-ns {}", if ok { "ok" } else { "error" });
        for (name, time) in CALLS.iter().zip(self.times) {
            match time {
                Some((elapsed, Some(cpu))) => { let _ = write!(line, " {name}={elapsed}/{cpu}"); }
                Some((elapsed, None)) => { let _ = write!(line, " {name}={elapsed}/unsupported"); }
                None => { let _ = write!(line, " {name}=-"); }
            }
        }
        line.push('\n');
        // Fourteen fixed names and u64 values keep the entire line below 1024
        // bytes, including the unavailable-clock spelling and final newline.
        if let Ok(mut evidence) = OpenOptions::new().write(true).open("/dev/screen-evidence") {
            let _ = evidence.write_all(line.as_bytes());
        }
    }
}
struct Mapping { base: *mut u8, ptr: *mut u8, len: usize }
impl Mapping {
    fn new(file: &File, offset: usize, len: usize) -> Result<Self, String> {
        Self::new_timed(file, offset, len, &mut CallTimes::new(false), 0)
    }
    fn new_timed(file: &File, offset: usize, len: usize, calls: &mut CallTimes, index: usize) -> Result<Self, String> {
        let page = usize::try_from(unsafe { getpagesize() }).map_err(|_| "invalid mapping page size")?;
        if !page.is_power_of_two() { return Err("invalid mapping page size".into()); }
        let skip = offset % page;
        let len = (skip + len).next_multiple_of(page);
        // File mappings populate resident pages on Breenix. Map only the range
        // needed now; defer unused screen/frame payloads until accessed.
        let ptr = calls.call(index, || {
            let ptr = unsafe { mmap(core::ptr::null_mut(), len, 3, 1, file.as_raw_fd(), (offset - skip) as i64) };
            // Capture errno before reading either clock.
            if ptr as isize == -1 { Err(std::io::Error::last_os_error().to_string()) } else { Ok(ptr) }
        })?;
        Ok(Self { base: ptr, ptr: unsafe { ptr.add(skip) }, len })
    }
}
impl Drop for Mapping { fn drop(&mut self) { unsafe { munmap(self.base, self.len); } } }
pub struct Arena {
    path: std::path::PathBuf,
    file: File,
    headers: Vec<Mapping>,
    payloads: [Option<Mapping>; 3],
    layout: Layout,
}
impl Arena {
    pub fn create(path: &Path, layout: Layout) -> Result<Self, String> {
        Self::create_timed(path, layout, &mut CallTimes::new(false))
    }
    fn create_timed(path: &Path, layout: Layout, calls: &mut CallTimes) -> Result<Self, String> {
        let mut file = calls.call(0, || OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(path)).map_err(|e| e.to_string())?;
        calls.call(1, || file.set_len(layout.len() as u64)).map_err(|e| e.to_string())?;
        use std::io::Write;
        let mut header = [0; 16]; header[..8].copy_from_slice(b"UTARE001");
        header[8..12].copy_from_slice(&layout.width.to_le_bytes()); header[12..16].copy_from_slice(&layout.height.to_le_bytes());
        calls.call(2, || file.write_all(&header)).map_err(|e| e.to_string())?;
        Self::map_timed(path, file, layout, calls)
    }
    pub fn open(path: &Path) -> Result<Self, String> {
        use std::{io::Read, os::unix::fs::MetadataExt};
        let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.mode() & 0o077 != 0 { return Err("untrusted arena file".into()); }
        let mut file = OpenOptions::new().read(true).write(true).open(path).map_err(|e| e.to_string())?;
        let mut header = [0; 16]; file.read_exact(&mut header).map_err(|e| e.to_string())?;
        if &header[..8] != b"UTARE001" { return Err("invalid arena header".into()); }
        let layout = Layout::for_mode(u32::from_le_bytes(header[8..12].try_into().unwrap()), u32::from_le_bytes(header[12..16].try_into().unwrap()))?;
        if file.metadata().map_err(|e| e.to_string())?.len() != layout.len() as u64 { return Err("arena length mismatch".into()); }
        Self::map(path, file, layout)
    }
    fn map(path: &Path, file: File, layout: Layout) -> Result<Self, String> {
        Self::map_timed(path, file, layout, &mut CallTimes::new(false))
    }
    fn map_timed(path: &Path, file: File, layout: Layout, calls: &mut CallTimes) -> Result<Self, String> {
        let headers = [Slot::Screen, Slot::Frame, Slot::Tree].into_iter()
            .map(|slot| Mapping::new_timed(&file, layout.header_offset(slot), HEADER, calls, 3 + slot.index()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { path: path.into(), file, headers, payloads: [None, None, None], layout })
    }
    pub fn layout(&self) -> Layout { self.layout }
    /// Start another immutable checkpoint; existing mappings retain the old inode.
    pub fn next_checkpoint(&mut self) -> Result<(), String> {
        let mut calls = CallTimes::new(true);
        let result = (|| {
            let next_path = self.path.with_extension("next");
            let mut next = Self::create_timed(&next_path, self.layout, &mut calls)?;
            if let Err(error) = calls.call(6, || std::fs::rename(&next_path, &self.path)) {
                let _ = std::fs::remove_file(&next_path);
                return Err(error.to_string());
            }
            next.path = self.path.clone();
            let old = std::mem::replace(self, next);
            // Preserve Arena's field-drop order: file, headers, then payloads.
            // The old mappings continue to own the unlinked inode after close.
            let Self { file, headers, payloads, .. } = old;
            calls.call(7, || drop(file));
            for (index, mapping) in headers.into_iter().enumerate() {
                calls.call(8 + index, || drop(mapping));
            }
            for (index, mapping) in payloads.into_iter().enumerate() {
                if let Some(mapping) = mapping { calls.call(11 + index, || drop(mapping)); }
            }
            Ok(())
        })();
        calls.report(result.is_ok());
        result
    }
    /// The collector opens the active checkpoint after the case has stopped.
    pub fn reopen(&self) -> Result<Self, String> { Self::open(&self.path) }
    fn state(&self, slot: Slot) -> &AtomicU32 {
        // Slot headers start on page boundaries; state is their first word.
        unsafe { &*self.headers[slot.index()].ptr.cast::<AtomicU32>() }
    }
    pub fn begin(&mut self, slot: Slot) -> Result<Writer<'_>, String> {
        self.state(slot).compare_exchange(FREE, WRITING, Ordering::AcqRel, Ordering::Acquire).map_err(|_| "arena slot already used")?;
        self.payloads[slot.index()] = Some(Mapping::new(&self.file, self.layout.data_offset(slot), self.layout.capacity(slot))?);
        Ok(Writer { arena: self, slot })
    }
    /// Fill the frame and its tree directly, keeping both WRITING on any refusal.
    pub fn write_frame_tree(&mut self, fill: impl FnOnce(&mut [u8], &mut [u8]) -> Result<(Record, usize), String>) -> Result<Record, String> {
        for slot in [Slot::Frame, Slot::Tree] {
            self.state(slot).compare_exchange(FREE, WRITING, Ordering::AcqRel, Ordering::Acquire).map_err(|_| "arena slot already used")?;
            self.payloads[slot.index()] = Some(Mapping::new(&self.file, self.layout.data_offset(slot), self.layout.capacity(slot))?);
        }
        // The layout reserves disjoint payload ranges for the two slots. Both are
        // mutably borrowed for this call and neither is published yet.
        let pixels = unsafe { std::slice::from_raw_parts_mut(self.payloads[1].as_ref().unwrap().ptr, self.layout.capacity(Slot::Frame)) };
        let tree = unsafe { std::slice::from_raw_parts_mut(self.payloads[2].as_ref().unwrap().ptr, TREE_BYTES) };
        let (record, len) = fill(pixels, tree)?;
        Writer { arena: self, slot: Slot::Tree }.publish(record.clone(), TREE_BYTES)?;
        Writer { arena: self, slot: Slot::Frame }.publish(record.clone(), len)?;
        Ok(record)
    }
    pub fn read(&self, slot: Slot) -> Result<SlotRead, String> {
        match self.state(slot).load(Ordering::Acquire) {
            FREE => return Ok(SlotRead::Free), WRITING => return Ok(SlotRead::Partial("case-died")),
            FINAL => {}, _ => return Err("invalid arena state".into()),
        }
        // The frame's release is the commit point for write_frame_tree. A death
        // after the tree release alone has not published a complete capture.
        if matches!(slot, Slot::Tree) && self.state(Slot::Frame).load(Ordering::Acquire) == WRITING {
            return Ok(SlotRead::Partial("case-died"));
        }
        // FINAL is terminal. No writer can touch either range after the release.
        let h = unsafe { std::slice::from_raw_parts(self.headers[slot.index()].ptr.add(4), HEADER - 4) };
        let q = |at| u64::from_le_bytes(h[at..at+8].try_into().unwrap());
        let n = usize::try_from(q(0)).map_err(|_| "invalid arena length")?;
        let check_len = q(56) as usize;
        if n > self.layout.capacity(slot) || check_len > 256 { return Err("invalid arena record lengths".into()); }
        let payload = if n == 0 { None } else { Some(Mapping::new(&self.file, self.layout.data_offset(slot), n)?) };
        let bytes = payload.as_ref().map_or(&[][..], |mapping| unsafe { std::slice::from_raw_parts(mapping.ptr, n) });
        if sha256(bytes).as_slice() != &h[64..96] { return Err("arena payload hash mismatch".into()); }
        let observation_len = q(360) as usize;
        if observation_len > 1024 { return Err("invalid arena observation length".into()); }
        let record = Record { observation: h[368..368+observation_len].to_vec(), check: String::from_utf8(h[96..96+check_len].to_vec()).map_err(|_| "invalid arena check")?,
            window: q(8), commit: q(16), epoch: q(24), composite: q(32), width: q(40) as u32, height: q(48) as u32, oversize: h[352] != 0 };
        Ok(SlotRead::Final { record, bytes: bytes.to_vec() })
    }
}
/// Dropping an unpublished writer leaves WRITING visible to the collector.
pub struct Writer<'a> { arena: &'a mut Arena, slot: Slot }
impl Writer<'_> {
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.arena.payloads[self.slot.index()].as_ref().expect("writer payload").ptr, self.arena.layout.capacity(self.slot)) }
    }
    pub fn publish(mut self, mut record: Record, requested: usize) -> Result<(), String> {
        if record.observation.len() > 1024 || record.check.len() > 256 || record.check.contains(['\n', '\r', '\0']) { return Err("invalid arena check name".into()); }
        let n = requested.min(self.arena.layout.capacity(self.slot));
        record.oversize |= requested > n;
        let hash = sha256(&self.bytes_mut()[..n]);
        let h = unsafe { std::slice::from_raw_parts_mut(self.arena.headers[self.slot.index()].ptr.add(4), HEADER - 4) };
        for (i, value) in [n as u64, record.window, record.commit, record.epoch, record.composite,
            u64::from(record.width), u64::from(record.height), record.check.len() as u64].into_iter().enumerate() {
            h[i*8..i*8+8].copy_from_slice(&value.to_le_bytes());
        }
        h[64..96].copy_from_slice(&hash); h[96..96+record.check.len()].copy_from_slice(record.check.as_bytes());
        h[352] = u8::from(record.oversize);
        h[360..368].copy_from_slice(&(record.observation.len() as u64).to_le_bytes());
        h[368..368+record.observation.len()].copy_from_slice(&record.observation);
        self.arena.state(self.slot).store(FINAL, Ordering::Release);
        Ok(())
    }
}
