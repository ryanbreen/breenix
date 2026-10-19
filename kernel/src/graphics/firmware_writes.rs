//! Kernel writes to the firmware framebuffer after the GPU console's handoff.
//!
//! `docs/gui/gpu-abi.md`, G7 display interface, `firmware_writes_after_handoff` (ruling 50):
//! a device-wide, monotonic, saturating count of kernel store, fill and copy operations
//! that write the firmware framebuffer once `handoff` has become 1. It is reset only at
//! boot. Lock-free, so any writer, including a panic path, may count.

use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

static HANDED_OFF: AtomicBool = AtomicBool::new(false);
static WRITES: AtomicU64 = AtomicU64::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static LENGTH: AtomicUsize = AtomicUsize::new(0);

/// Publish the mapped firmware framebuffer when its canvas is initialized.
pub fn register(base: usize, length: usize) {
    BASE.store(base, Ordering::Relaxed);
    LENGTH.store(length, Ordering::Release);
}

/// Account a raw destination only when it overlaps the firmware framebuffer.
pub fn note_range(destination: *mut u8, length: usize) {
    let firmware_length = LENGTH.load(Ordering::Acquire);
    let base = BASE.load(Ordering::Relaxed);
    let start = destination as usize;
    if length != 0 && firmware_length != 0
        && start < base.saturating_add(firmware_length)
        && base < start.saturating_add(length)
    {
        note();
    }
}

/// The GPU console's first present completed: count kernel writes from now on.
pub fn handoff() {
    HANDED_OFF.store(true, Ordering::Release);
}

/// One kernel operation stored to the firmware framebuffer.
#[inline]
pub fn note() {
    if HANDED_OFF.load(Ordering::Acquire) {
        let _ = WRITES.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1));
    }
}

/// The operations counted since handoff.
pub fn count() -> u64 {
    WRITES.load(Ordering::Acquire)
}

/// Mutable canvas access without lending an unaccounted mutable slice or pointer.
/// The handoff latch is checked at each mutation, including for access obtained before it.
pub struct BufferMut<'a> {
    bytes: &'a mut [u8],
    firmware: bool,
}

impl<'a> BufferMut<'a> {
    pub fn new(bytes: &'a mut [u8], firmware: bool) -> Self {
        Self { bytes, firmware }
    }

    pub fn copy_from_slice(&mut self, offset: usize, source: &[u8]) {
        let destination = &mut self.bytes[offset..offset + source.len()];
        if !destination.is_empty() {
            destination.copy_from_slice(source);
            if self.firmware {
                note();
            }
        }
    }

    /// Fill complete pixels as one mutation, checking the handoff latch once.
    pub fn fill_pixels(&mut self, range: core::ops::Range<usize>, pixel: &[u8]) {
        let destination = &mut self.bytes[range];
        if !destination.is_empty() && !pixel.is_empty() {
            for chunk in destination.chunks_exact_mut(pixel.len()) {
                chunk.copy_from_slice(pixel);
            }
            if self.firmware && destination.len() >= pixel.len() {
                note();
            }
        }
    }

    pub fn copy_within(&mut self, source: core::ops::Range<usize>, destination: usize) {
        // Validate even an empty range before deciding whether it writes anything.
        let length = self.bytes[source.clone()].len();
        let _ = &self.bytes[destination..destination + length];
        if length != 0 {
            self.bytes.copy_within(source, destination);
            if self.firmware {
                note();
            }
        }
    }

    pub fn fill(&mut self, range: core::ops::Range<usize>, value: u8) {
        let destination = &mut self.bytes[range];
        if !destination.is_empty() {
            destination.fill(value);
            if self.firmware {
                note();
            }
        }
    }
}

impl core::ops::Deref for BufferMut<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.bytes
    }
}
