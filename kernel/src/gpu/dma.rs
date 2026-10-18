//! Guest memory the GPU paths hand to the device.
//!
//! Resource backing and large command payloads are whole frames from the frame allocator
//! (`docs/gui/gpu.md` section 1.3), described to the device as page lists; small command
//! headers and responses live in heap boxes, whose direct-mapped addresses are physically
//! contiguous on ARM64. Every buffer is owned by the kernel object that holds it, and a
//! request that hands a buffer to the device keeps it until the device returns the
//! request or the device is reset (VirtIO 1.3, section 2.7.13 and 2.4).

use alloc::alloc::{alloc_zeroed, dealloc, handle_alloc_error, Layout};
use alloc::sync::Arc;
use core::ptr::NonNull;
use alloc::vec::Vec;

use crate::memory::arch_stub::PhysFrame;
use crate::memory::frame_allocator;

pub const PAGE: usize = 4096;

/// The device address of a kernel virtual address in the direct map or the heap.
#[inline]
pub fn device_addr(virt: u64) -> u64 {
    crate::platform_config::kernel_va_to_ipa(virt)
}

/// The kernel virtual address of a frame.
#[inline]
fn frame_va(frame: &PhysFrame) -> u64 {
    crate::memory::physical_memory_offset().as_u64() + frame.start_address().as_u64()
}

/// The data cache line size from CTR_EL0 (Arm Architecture Reference Manual, D19.2.34).
fn dcache_line() -> u64 {
    let ctr: u64;
    // SAFETY: CTR_EL0 is readable at EL1.
    unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    4 << ((ctr >> 16) & 0xF)
}

/// Clean `[va, va + len)` to the point of coherency, so the device reads what the CPU wrote.
pub fn clean(va: u64, len: usize) {
    maintain(va, len, false);
}

/// Clean and invalidate `[va, va + len)`, so the CPU reads what the device wrote.
pub fn invalidate(va: u64, len: usize) {
    maintain(va, len, true);
}

fn maintain(va: u64, len: usize, invalidate: bool) {
    if len == 0 {
        return;
    }
    let line = dcache_line();
    let mut at = va & !(line - 1);
    let end = va + len as u64;
    while at < end {
        // SAFETY: the range is mapped kernel memory owned by the caller.
        unsafe {
            if invalidate {
                core::arch::asm!("dc civac, {}", in(reg) at, options(nostack));
            } else {
                core::arch::asm!("dc cvac, {}", in(reg) at, options(nostack));
            }
        }
        at += line;
    }
    // SAFETY: barrier only.
    unsafe { core::arch::asm!("dsb sy", options(nostack)) };
}

/// A small, physically contiguous, zeroed buffer for command headers and responses.
pub struct DmaBox {
    bytes: NonNull<u8>,
    layout: Layout,
    len: usize,
}

// SAFETY: uniquely owned allocation, with access serialized by the GPU core.
unsafe impl Send for DmaBox {}

impl Drop for DmaBox {
    fn drop(&mut self) {
        // SAFETY: the original allocator and layout; DMA ownership ended before Drop.
        unsafe { dealloc(self.bytes.as_ptr(), self.layout) };
    }
}

impl DmaBox {
    /// A zeroed buffer of `len` bytes. The ARM64 kernel heap is one physically contiguous
    /// range of the direct map, so the device sees the buffer as one contiguous range.
    pub fn new(len: usize) -> DmaBox {
        // The entire first and last cache lines belong to this allocation, so
        // invalidating a device response cannot discard a neighbour's CPU writes.
        // SVGA command headers reserve the low six address bits for the queue.
        let line = (dcache_line() as usize).max(64);
        let layout = Layout::from_size_align(len.max(1).next_multiple_of(line), line)
            .expect("DMA buffer layout");
        // SAFETY: nonzero layout; the allocation is owned until Drop.
        let raw = unsafe { alloc_zeroed(layout) };
        let bytes = NonNull::new(raw).unwrap_or_else(|| handle_alloc_error(layout));
        DmaBox { bytes, layout, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn as_mut(&mut self) -> &mut [u8] {
        // SAFETY: the u64 slice holds at least `len` bytes.
        unsafe { core::slice::from_raw_parts_mut(self.bytes.as_ptr(), self.len) }
    }

    pub fn as_ref(&self) -> &[u8] {
        // SAFETY: as above.
        unsafe { core::slice::from_raw_parts(self.bytes.as_ptr(), self.len) }
    }

    pub fn va(&self) -> u64 {
        self.bytes.as_ptr() as u64
    }

    pub fn addr(&self) -> u64 {
        device_addr(self.va())
    }

    pub fn put_u32(&mut self, at: usize, v: u32) {
        self.as_mut()[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn put_u64(&mut self, at: usize, v: u64) {
        self.as_mut()[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    pub fn u32_at(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.as_ref()[at..at + 4].try_into().unwrap_or([0; 4]))
    }

    pub fn u64_at(&self, at: usize) -> u64 {
        u64::from_le_bytes(self.as_ref()[at..at + 8].try_into().unwrap_or([0; 8]))
    }

    /// Make the CPU's writes visible to the device.
    pub fn publish(&self) {
        clean(self.va(), self.len);
    }

    /// Make the device's writes visible to the CPU.
    pub fn observe(&self) {
        invalidate(self.va(), self.len);
    }
}

/// Whole frames, in order, owned until dropped.
pub struct Pages {
    frames: Vec<PhysFrame>,
    /// RLIMIT_AS charge follows the backing through requests and confirmed reset.
    charge: Option<Arc<crate::process::limits::Limits>>,
    // Backing and its charge guards share a lifetime, through unref or reset.
    _quota_charge: Option<super::quota::QuotaCharge>,
    _copy_charge: Option<super::quota::Charge>,
}

impl Pages {
    /// `count` zeroed frames, or `None` (with every frame taken returned) when the
    /// allocator runs out.
    pub fn new(count: usize) -> Option<Pages> {
        let mut pages = Pages { frames: Vec::new(), charge: None, _quota_charge: None, _copy_charge: None };
        if pages.frames.try_reserve_exact(count).is_err() {
            return None;
        }
        for _ in 0..count {
            let frame = frame_allocator::allocate_frame()?;
            // SAFETY: a fresh frame in the direct map, owned here.
            unsafe { core::ptr::write_bytes(frame_va(&frame) as *mut u8, 0, PAGE) };
            pages.frames.push(frame);
        }
        Some(pages)
    }

    /// No frames: for a resource the device keeps without guest backing.
    pub fn none() -> Pages {
        Pages { frames: Vec::new(), charge: None, _quota_charge: None, _copy_charge: None }
    }

    /// Adopt a reservation already made under PROCESS_MANAGER.
    pub fn charge_to(&mut self, limits: Arc<crate::process::limits::Limits>) {
        self.charge = Some(limits);
    }

    /// Adopt a resource's quota reservation before publishing its backing.
    pub(super) fn charge_quota(&mut self, charge: super::quota::QuotaCharge) {
        self._quota_charge = Some(charge);
    }

    /// Adopt a submission's device-storage reservation (including backing-free storage).
    pub(super) fn charge_copy(&mut self, charge: super::quota::Charge) {
        self._copy_charge = Some(charge);
    }

    pub fn count(&self) -> usize {
        self.frames.len()
    }

    pub fn bytes(&self) -> usize {
        self.frames.len() * PAGE
    }

    /// The device address of each run of physically adjacent frames, with its length.
    pub fn runs(&self) -> Vec<(u64, u32)> {
        let mut runs: Vec<(u64, u32)> = Vec::new();
        for f in &self.frames {
            let addr = device_addr(frame_va(f));
            match runs.last_mut() {
                Some((start, len)) if *start + *len as u64 == addr && (*len as usize) < (1 << 30) => {
                    *len += PAGE as u32;
                }
                _ => runs.push((addr, PAGE as u32)),
            }
        }
        runs
    }

    /// Frame `i`'s kernel virtual address.
    pub fn page_va(&self, i: usize) -> u64 {
        frame_va(&self.frames[i])
    }

    /// The device address of frame `i`.
    pub fn page_addr(&self, i: usize) -> u64 {
        device_addr(self.page_va(i))
    }

    /// Copy `src` in at byte `offset`.
    pub fn write(&self, offset: usize, src: &[u8]) {
        let mut done = 0;
        while done < src.len() {
            let at = offset + done;
            let page = at / PAGE;
            let within = at % PAGE;
            let n = (PAGE - within).min(src.len() - done);
            // SAFETY: the page is owned and mapped; n stays inside it.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src[done..].as_ptr(),
                    (self.page_va(page) + within as u64) as *mut u8,
                    n,
                )
            };
            done += n;
        }
    }

    /// Copy out `dst.len()` bytes from byte `offset`.
    pub fn read(&self, offset: usize, dst: &mut [u8]) {
        let mut done = 0;
        while done < dst.len() {
            let at = offset + done;
            let page = at / PAGE;
            let within = at % PAGE;
            let n = (PAGE - within).min(dst.len() - done);
            // SAFETY: as in `write`.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (self.page_va(page) + within as u64) as *const u8,
                    dst[done..].as_mut_ptr(),
                    n,
                )
            };
            done += n;
        }
    }

    /// Fill `[offset, offset + len)` with the repeating 32-bit `pattern`, aligned so byte
    /// `i` of the buffer is byte `i % 4` of the pattern.
    pub fn fill(&self, offset: usize, len: usize, pattern: u32) {
        let _ = self.try_spans(offset, len, |done, ptr, n| {
            // SAFETY: the span lies inside one of these frames.
            fill_pattern(unsafe { core::slice::from_raw_parts_mut(ptr, n) }, offset + done, pattern);
            Ok::<(), ()>(())
        });
    }

    /// Call `f` with each part of `[offset, offset + len)` that lies inside one frame: the
    /// part's position from `offset`, its kernel address and its length, in order. An error
    /// from `f` ends the walk.
    pub fn try_spans<E>(&self, offset: usize, len: usize, mut f: impl FnMut(usize, *mut u8, usize) -> Result<(), E>) -> Result<(), E> {
        let mut done = 0;
        while done < len {
            let at = offset + done;
            let within = at % PAGE;
            let n = (PAGE - within).min(len - done);
            f(done, (self.page_va(at / PAGE) + within as u64) as *mut u8, n)?;
            done += n;
        }
        Ok(())
    }

    pub fn u32_at(&self, offset: usize) -> u32 {
        let mut b = [0u8; 4];
        self.read(offset, &mut b);
        u32::from_le_bytes(b)
    }

    /// Clean or invalidate `[offset, offset + len)` page by page.
    pub fn maintain(&self, offset: usize, len: usize, for_cpu: bool) {
        let mut done = 0;
        while done < len {
            let at = offset + done;
            let page = at / PAGE;
            let within = at % PAGE;
            let n = (PAGE - within).min(len - done);
            let va = self.page_va(page) + within as u64;
            if for_cpu {
                invalidate(va, n);
            } else {
                clean(va, n);
            }
            done += n;
        }
    }
}

/// Write `pattern`'s bytes cyclically into `span`, which begins at byte `at` of a run filled
/// with the pattern from its byte 0: eight bytes per store once the run is at a pattern
/// boundary.
fn fill_pattern(span: &mut [u8], at: usize, pattern: u32) {
    let p = pattern.to_le_bytes();
    let (head, rest) = span.split_at_mut(((4 - at % 4) % 4).min(span.len()));
    for (k, b) in head.iter_mut().enumerate() {
        *b = p[(at + k) % 4];
    }
    let wide = [p[0], p[1], p[2], p[3], p[0], p[1], p[2], p[3]];
    let mut words = rest.chunks_exact_mut(8);
    for w in &mut words {
        w.copy_from_slice(&wide);
    }
    for (k, b) in words.into_remainder().iter_mut().enumerate() {
        *b = p[k % 4];
    }
}

impl Drop for Pages {
    fn drop(&mut self) {
        if let Some(limits) = self.charge.take() {
            limits.release_gpu_bytes(self.bytes() as u64);
        }
        for frame in self.frames.drain(..) {
            frame_allocator::deallocate_frame(frame);
        }
    }
}

/// A growing sequence of 32-bit words in frames: a VirGL command buffer or a copy of a
/// BGL stream.
pub struct WordBuf {
    pages: Vec<Pages>,
    words: usize,
}

const WORDS_PER_PAGE: usize = PAGE / 4;

impl WordBuf {
    pub fn new() -> WordBuf {
        WordBuf { pages: Vec::new(), words: 0 }
    }

    pub fn len(&self) -> usize {
        self.words
    }

    /// Append one word; false when the allocator has no frame for it.
    pub fn push(&mut self, w: u32) -> bool {
        if self.words % WORDS_PER_PAGE == 0 && self.words / WORDS_PER_PAGE == self.pages.len() {
            match Pages::new(1) {
                Some(p) => self.pages.push(p),
                None => return false,
            }
        }
        let page = self.words / WORDS_PER_PAGE;
        let within = self.words % WORDS_PER_PAGE;
        // SAFETY: inside an owned page.
        unsafe { *((self.pages[page].page_va(0) + 4 * within as u64) as *mut u32) = w };
        self.words += 1;
        true
    }

    pub fn word(&self, i: usize) -> u32 {
        // SAFETY: i < words, inside an owned page.
        unsafe {
            *((self.pages[i / WORDS_PER_PAGE].page_va(0) + 4 * (i % WORDS_PER_PAGE) as u64) as *const u32)
        }
    }

    /// Frame `i`'s pages object (each holds one frame).
    pub fn page(&self, i: usize) -> &Pages {
        &self.pages[i]
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Copy `len` bytes from user address `src` into a new buffer, page by page.
    pub fn from_user(src: u64, len: usize) -> Result<WordBuf, i64> {
        let mut buf = WordBuf::new();
        let pages = len.div_ceil(PAGE);
        if buf.pages.try_reserve_exact(pages).is_err() {
            return Err(gpu_abi::errno::ENOMEM);
        }
        for i in 0..pages {
            let p = Pages::new(1).ok_or(gpu_abi::errno::ENOMEM)?;
            let n = PAGE.min(len - i * PAGE);
            crate::syscall::userptr::read_user_bytes(p.page_va(0) as *mut u8, src + (i * PAGE) as u64, n)
                .map_err(|_| gpu_abi::errno::EFAULT)?;
            buf.pages.push(p);
        }
        buf.words = len / 4;
        Ok(buf)
    }
}

impl gpu_abi::stream::Words for WordBuf {
    fn len(&self) -> usize {
        self.words
    }
    fn word(&self, index: usize) -> u32 {
        WordBuf::word(self, index)
    }
}

impl super::observation::Memory for Pages {
    fn invalidate(&self, offset: usize, length: usize) {
        self.maintain(offset, length, true);
    }
    fn copy(&self, offset: usize, out: &mut [u8]) {
        self.read(offset, out);
    }
}
