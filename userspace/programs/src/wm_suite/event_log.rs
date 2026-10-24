//! Single-producer/single-consumer shared event log for an independent WM client.
use libbreenix::{memory, window::abi::WindowEvent};
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicU64, Ordering};

pub const CAPACITY: usize = 8192;
const DATA: usize = 4096;
static NEXT: AtomicU64 = AtomicU64::new(0);

#[repr(C)]
struct Header {
    magic: [u8; 8],
    written: AtomicU64,
    read: AtomicU64,
    lost: AtomicU64,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Entry {
    pub window: u64,
    pub event: WindowEvent,
}

pub struct EventLog {
    ptr: *mut u8,
    bytes: usize,
    capacity: usize,
}
impl EventLog {
    pub fn create() -> Result<(Self, File), String> {
        Self::create_bounded(CAPACITY)
    }
    pub fn create_bounded(capacity: usize) -> Result<(Self, File), String> {
        if !(1..=CAPACITY).contains(&capacity) {
            return Err("invalid shared event log capacity".into());
        }
        let path = format!(
            "/tmp/wm-client-events-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let bytes = DATA + capacity * core::mem::size_of::<Entry>();
        file.set_len(bytes as u64).map_err(|e| e.to_string())?;
        let log = Self::map(file.as_raw_fd())?;
        // The descriptors and mappings retain the unlinked backing until both
        // processes are done; no case leaves a named shared-memory fixture.
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        unsafe {
            log.ptr.cast::<Header>().write(Header {
                magic: *b"WMEVT001",
                written: AtomicU64::new(0),
                read: AtomicU64::new(0),
                lost: AtomicU64::new(0),
            });
        }
        Ok((log, file))
    }
    pub fn map(fd: i32) -> Result<Self, String> {
        // Borrow the inherited descriptor without closing its owner's handle.
        let file = core::mem::ManuallyDrop::new(unsafe { File::from_raw_fd(fd) });
        let bytes = usize::try_from(file.metadata().map_err(|e| e.to_string())?.len())
            .map_err(|_| "shared event log length does not fit usize")?;
        let entry_bytes = core::mem::size_of::<Entry>();
        if bytes <= DATA || (bytes - DATA) % entry_bytes != 0 {
            return Err("invalid shared event log length".into());
        }
        let capacity = (bytes - DATA) / entry_bytes;
        if capacity > CAPACITY {
            return Err("shared event log exceeds maximum capacity".into());
        }
        let ptr = memory::mmap(
            core::ptr::null_mut(),
            bytes,
            memory::PROT_READ | memory::PROT_WRITE,
            memory::MAP_SHARED,
            fd,
            0,
        )
        .map_err(|e| format!("event log mapping: {e}"))?;
        Ok(Self { ptr, bytes, capacity })
    }
    fn header(&self) -> &Header {
        // The mapping starts at page alignment and outlives this borrow.
        unsafe { &*self.ptr.cast::<Header>() }
    }
    fn entry(&self, index: u64) -> *mut Entry {
        unsafe {
            self.ptr
                .add(DATA)
                .cast::<Entry>()
                .add(index as usize % self.capacity)
        }
    }
    pub fn push(&self, window: u64, event: WindowEvent) -> Result<(), String> {
        let h = self.header();
        if h.magic != *b"WMEVT001" {
            return Err("event log magic is invalid".into());
        }
        let written = h.written.load(Ordering::Relaxed);
        if written - h.read.load(Ordering::Acquire) >= self.capacity as u64 {
            h.lost.fetch_add(1, Ordering::Release);
            return Err("shared client event log overflow".into());
        }
        // The consumer has released this slot. Publish only after its contents
        // are complete, including before a deliberate crash-after-N fault.
        unsafe {
            self.entry(written).write(Entry { window, event });
        }
        h.written.store(written + 1, Ordering::Release);
        Ok(())
    }
    pub fn published(&self) -> u64 {
        self.header().written.load(Ordering::Acquire)
    }
    pub fn drain(&self) -> Result<Vec<Entry>, String> {
        self.drain_through(self.published())
    }
    pub fn drain_through(&self, written: u64) -> Result<Vec<Entry>, String> {
        let h = self.header();
        if h.magic != *b"WMEVT001" {
            return Err("event log magic is invalid".into());
        }
        if h.lost.load(Ordering::Acquire) != 0 {
            return Err("shared client event log lost events".into());
        }
        let read = h.read.load(Ordering::Relaxed);
        let available = h.written.load(Ordering::Acquire);
        if written < read || written > available || written - read > self.capacity as u64 {
            return Err("event log indices are invalid".into());
        }
        let mut entries = Vec::with_capacity((written - read) as usize);
        for i in read..written {
            // Acquire observed the producer's complete slot; it will not reuse
            // the slot until the read index below is released.
            entries.push(unsafe { self.entry(i).read() });
        }
        h.read.store(written, Ordering::Release);
        Ok(entries)
    }
}
impl Drop for EventLog {
    fn drop(&mut self) {
        let _ = memory::munmap(self.ptr, self.bytes);
    }
}
