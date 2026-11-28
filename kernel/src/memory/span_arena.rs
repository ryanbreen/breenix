//! Span arenas (E1 plan, 6.4): requests above a page and up to 512 KiB, each
//! served by `heap_index` inside a 2 MiB span of the span window. No kernel
//! dependencies, so `tools/kmem-host-tests` compiles this file as is;
//! `memory/large.rs` is the kernel side, with the locks, the window and the
//! frames.
//!
//! A span's first bytes are its record: the `heap_index` heap over the rest
//! of the span, the arena that owns it, and the token its mapping returns.
//! Spans are 2 MiB aligned, so a block's span is its address rounded down
//! to 2 MiB, and a free finds the owning arena in O(1) from the address.
//!
//! An arena keeps its spans in bins by the size of each span's largest free
//! extent: bin `b` holds spans whose largest extent is in `[2^b, 2^(b+1))`.
//! An allocation takes the first span of the lowest bin each of whose spans
//! fits it, else tries each span of the bins below that whose largest extent
//! can hold the block unpadded, else asks for a new span.
//! A span whose last block is freed is given back, except one empty span
//! the arena keeps.

use core::alloc::Layout;
use core::ptr::{null_mut, NonNull};

use super::heap_index::{Heap, RESIZE_SLACK};

/// The bytes of one span: the span window's granule.
pub const SPAN_BYTES: usize = 2 << 20;
/// The largest request a span arena serves (6.1 row 2).
pub const MAX_SPAN_REQUEST: usize = 512 << 10;
/// The largest alignment a span arena serves (6.1 row 2).
pub const MAX_SPAN_ALIGN: usize = 4096;
/// The record's bytes at the start of each span, before its heap.
const RECORD_BYTES: usize = 1024;
/// One bin per power of two up to the span's size.
const BINS: usize = 22;
const NO_BIN: usize = BINS;

/// A span's record, at its first byte.
#[repr(C)]
struct Span<T> {
    heap: Heap,
    token: T,
    arena: usize,
    bin: usize,
    prev: *mut Span<T>,
    next: *mut Span<T>,
}

/// Why an arena could not serve a request from its spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    /// No span has room: map a new one and pass it to `adopt`.
    NeedSpan,
    /// The layout is outside 6.1 row 2 and no span can hold it.
    TooLarge,
}

/// A span to give back to the window: its base and its mapping's token.
pub struct Unmapped<T> {
    pub base: NonNull<u8>,
    pub token: T,
}

/// One CPU's spans (or the shared arena's), serialized by the caller's lock.
pub struct Arena<T: Copy> {
    id: usize,
    bins: [*mut Span<T>; BINS],
    nonempty: u32,
    spans: usize,
    /// The one empty span the arena keeps, if any.
    retained: *mut Span<T>,
}

// SAFETY: the spans are reached only through the arena that owns them, which
// the kernel serializes under its lock; a cross-CPU free takes that lock.
unsafe impl<T: Copy + Send> Send for Arena<T> {}

/// The span holding `ptr`.
fn span_of<T>(ptr: NonNull<u8>) -> *mut Span<T> {
    (ptr.as_ptr() as usize & !(SPAN_BYTES - 1)) as *mut Span<T>
}

/// The bin of a span whose largest free extent is `largest` bytes.
fn bin_of(largest: u32) -> usize {
    if largest == 0 {
        NO_BIN
    } else {
        (31 - largest.leading_zeros()) as usize
    }
}

/// The block bytes, header and worst-case alignment padding included: a free
/// extent of at least that size fits `layout` at any address.
fn worst_case(layout: Layout) -> usize {
    unpadded(layout) + layout.align().saturating_sub(8)
}

/// The block bytes with its header and no padding, at least `heap_index`'s
/// 16-byte block: no free extent smaller than that holds `layout`.
fn unpadded(layout: Layout) -> usize {
    (layout.size().next_multiple_of(8) + 8).max(16)
}

/// Whether a span arena serves `layout` (6.1 row 2, after row 1).
pub fn serves(layout: Layout) -> bool {
    layout.size() <= MAX_SPAN_REQUEST && layout.align() <= MAX_SPAN_ALIGN
}

/// The arena that owns the block at `ptr`.
///
/// # Safety
///
/// `ptr` is a live block from a span arena.
pub unsafe fn owner<T>(ptr: NonNull<u8>) -> usize {
    unsafe { (*span_of::<T>(ptr)).arena }
}

impl<T: Copy> Arena<T> {
    pub const fn new(id: usize) -> Self {
        Self { id, bins: [null_mut(); BINS], nonempty: 0, spans: 0, retained: null_mut() }
    }

    /// The spans this arena holds, the kept empty one included.
    pub fn spans(&self) -> usize {
        self.spans
    }

    /// Whether the arena keeps an empty span.
    pub fn has_retained(&self) -> bool {
        !self.retained.is_null()
    }

    unsafe fn link(&mut self, span: *mut Span<T>) {
        unsafe {
            let bin = bin_of((*span).heap.free_shape().1);
            (*span).bin = bin;
            (*span).prev = null_mut();
            if bin == NO_BIN {
                (*span).next = null_mut();
                return;
            }
            (*span).next = self.bins[bin];
            if let Some(head) = self.bins[bin].as_mut() {
                head.prev = span;
            }
            self.bins[bin] = span;
            self.nonempty |= 1 << bin;
        }
    }

    unsafe fn unlink(&mut self, span: *mut Span<T>) {
        unsafe {
            let bin = (*span).bin;
            if bin == NO_BIN {
                return;
            }
            match (*span).prev.as_mut() {
                Some(prev) => prev.next = (*span).next,
                None => self.bins[bin] = (*span).next,
            }
            if let Some(next) = (*span).next.as_mut() {
                next.prev = (*span).prev;
            }
            if self.bins[bin].is_null() {
                self.nonempty &= !(1 << bin);
            }
            (*span).bin = NO_BIN;
        }
    }

    /// Move `span` to the bin its largest free extent now names.
    unsafe fn rebin(&mut self, span: *mut Span<T>) {
        unsafe {
            if (*span).bin != bin_of((*span).heap.free_shape().1) {
                self.unlink(span);
                self.link(span);
            }
        }
    }

    unsafe fn take_from(&mut self, span: *mut Span<T>, layout: Layout) -> Option<NonNull<u8>> {
        unsafe {
            let ptr = (*span).heap.allocate(layout).0?;
            if span == self.retained {
                self.retained = null_mut();
            }
            self.rebin(span);
            Some(ptr)
        }
    }

    /// A block for `layout` from the arena's spans. A span in a bin at or
    /// above the request's worst case fits it at any address, so the head of
    /// the lowest such bin is taken. Below that, a span whose largest extent
    /// holds the unpadded block may still fit it at its alignment, so each
    /// span of those bins is tried before a new span is asked for. The worst
    /// case exceeds the unpadded block by under 4 KiB, the largest padding.
    pub fn allocate(&mut self, layout: Layout) -> Result<NonNull<u8>, Miss> {
        if !serves(layout) {
            return Err(Miss::TooLarge);
        }
        let worst = worst_case(layout);
        let fits = worst.next_power_of_two().trailing_zeros() as usize;
        let guaranteed = self.nonempty & (u32::MAX << fits.min(31));
        if guaranteed != 0 {
            let span = self.bins[guaranteed.trailing_zeros() as usize];
            // SAFETY: a binned span is this arena's and mapped.
            if let Some(ptr) = unsafe { self.take_from(span, layout) } {
                return Ok(ptr);
            }
        }
        let least = bin_of(unpadded(layout) as u32);
        for bin in least..fits.min(BINS) {
            let mut span = self.bins[bin];
            while !span.is_null() {
                // SAFETY: as above; `take_from` may rebin `span`, so its
                // successor is read first.
                let next = unsafe { (*span).next };
                if let Some(ptr) = unsafe { self.take_from(span, layout) } {
                    return Ok(ptr);
                }
                span = next;
            }
        }
        Err(Miss::NeedSpan)
    }

    /// Take a newly mapped span and serve `layout` from it. A layout no span
    /// can hold gives the span back unused.
    ///
    /// # Safety
    ///
    /// `base` is `SPAN_BYTES` aligned, and its `SPAN_BYTES` are mapped
    /// writable, owned by this arena until `free`, `take_retained` or this
    /// call hands them back with `token`.
    pub unsafe fn adopt(&mut self, base: NonNull<u8>, token: T, layout: Layout) -> Result<NonNull<u8>, Unmapped<T>> {
        if !serves(layout) {
            return Err(Unmapped { base, token });
        }
        const { assert!(core::mem::size_of::<Span<T>>() <= RECORD_BYTES) };
        let span = base.as_ptr() as *mut Span<T>;
        unsafe {
            span.write(Span { heap: Heap::empty(), token, arena: self.id, bin: NO_BIN, prev: null_mut(), next: null_mut() });
            (*span).heap.init(base.as_ptr().add(RECORD_BYTES), SPAN_BYTES - RECORD_BYTES);
            self.spans += 1;
            let ptr = (*span).heap.allocate(layout).0.expect("a new span holds each request a span serves");
            self.link(span);
            Ok(ptr)
        }
    }

    /// Free the block at `ptr`, allocated or last resized for `size` bytes.
    /// Returns the span to unmap when the block was its last and the arena
    /// already keeps an empty span. A `size` the block does not hold (6.1's
    /// check) leaves the arena unchanged and is returned as `Mismatch`, for
    /// the caller to report once it holds no lock.
    ///
    /// # Safety
    ///
    /// `ptr` is a live block of this arena, not freed twice.
    pub unsafe fn free(&mut self, ptr: NonNull<u8>, size: usize) -> Result<Option<Unmapped<T>>, Mismatch> {
        let span = span_of::<T>(ptr);
        unsafe {
            check_size(&(*span).heap, ptr, size)?;
            (*span).heap.deallocate(ptr);
            self.rebin(span);
            if (*span).heap.used() != 0 {
                return Ok(None);
            }
            if self.retained.is_null() || self.retained == span {
                self.retained = span;
                return Ok(None);
            }
            self.unlink(span);
            self.spans -= 1;
            Ok(Some(Unmapped { base: NonNull::new_unchecked(span.cast()), token: (*span).token }))
        }
    }

    /// Resize the block at `ptr`, allocated or last resized for `old_size`
    /// bytes, to `new_size` without moving it (6.4). Returns false, with the
    /// arena unchanged, when the free block after it cannot hold the growth;
    /// a `Mismatch` as `free` does.
    ///
    /// # Safety
    ///
    /// `ptr` is a live block of this arena.
    pub unsafe fn resize(&mut self, ptr: NonNull<u8>, old_size: usize, new_size: usize) -> Result<bool, Mismatch> {
        let span = span_of::<T>(ptr);
        unsafe {
            check_size(&(*span).heap, ptr, old_size)?;
            if !(*span).heap.resize_in_place(ptr, new_size) {
                return Ok(false);
            }
            self.rebin(span);
        }
        Ok(true)
    }

    /// Take back an empty span `take_retained` gave up whose unmap could not
    /// run (its window's lock was busy): kept again if the arena keeps no other
    /// empty span, otherwise binned as an empty span the next request can use.
    ///
    /// # Safety
    ///
    /// `span` came from this arena's `take_retained` and is still mapped.
    pub unsafe fn restore(&mut self, span: Unmapped<T>) {
        let span = span.base.as_ptr() as *mut Span<T>;
        unsafe { self.link(span) };
        self.spans += 1;
        if self.retained.is_null() {
            self.retained = span;
        }
    }

    /// Give up the kept empty span (7.2 step 2), to unmap.
    pub fn take_retained(&mut self) -> Option<Unmapped<T>> {
        let span = core::mem::replace(&mut self.retained, null_mut());
        if span.is_null() {
            return None;
        }
        // SAFETY: the kept span is this arena's, mapped and empty.
        unsafe {
            self.unlink(span);
            self.spans -= 1;
            Some(Unmapped { base: NonNull::new_unchecked(span.cast()), token: (*span).token })
        }
    }
}

/// A free or resize whose size is not the block's: the block holds `usable`
/// bytes. That is memory corruption, and the caller panics with both values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub usable: usize,
}

/// The free and resize check of 6.1: `size` is the size the block was
/// allocated or last resized for, so its usable size is at least that and
/// less than that plus `RESIZE_SLACK`.
unsafe fn check_size(heap: &Heap, ptr: NonNull<u8>, size: usize) -> Result<(), Mismatch> {
    let usable = unsafe { heap.usable_size(ptr) };
    if usable < size || usable - size >= RESIZE_SLACK {
        return Err(Mismatch { usable });
    }
    Ok(())
}
