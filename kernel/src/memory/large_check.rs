//! The span and vmap init self-check (E1-7), over the layers as `Layers`
//! reaches them. No kernel dependencies, so `tools/kmem-host-tests` runs it
//! over the real span arena and vmap code; `memory/large.rs` runs it at boot
//! over the kernel's.

use core::alloc::Layout;
use core::ptr::NonNull;

/// The window a block lies in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Span,
    Vmap,
    Elsewhere,
}

/// The larger-request layers as the check reaches them.
pub trait Layers {
    /// A block for `layout` from the layer 6.1 routes it to.
    fn allocate(&mut self, layout: Layout) -> Option<NonNull<u8>>;
    /// `large::reallocate`: `None` leaves the block as it was.
    ///
    /// # Safety
    ///
    /// `ptr` is a live block allocated or last resized for `layout`.
    unsafe fn reallocate(&mut self, ptr: NonNull<u8>, layout: Layout, new_size: usize) -> Option<NonNull<u8>>;
    /// # Safety
    ///
    /// As for `reallocate`; the block is not used again.
    unsafe fn deallocate(&mut self, ptr: NonNull<u8>, layout: Layout);
    /// The window `ptr` lies in.
    fn place(&self, ptr: NonNull<u8>) -> Place;
    /// Whether `dma::bus_address` refuses `[ptr, ptr + len)`.
    fn dma_refused(&self, ptr: NonNull<u8>, len: usize) -> bool;
    /// Write block `tag`'s pattern over `len` bytes at `ptr`.
    ///
    /// # Safety
    ///
    /// `[ptr, ptr + len)` is a live block's.
    unsafe fn fill(&mut self, ptr: NonNull<u8>, len: usize, tag: u8);
    /// Whether `len` bytes at `ptr` hold block `tag`'s pattern.
    ///
    /// # Safety
    ///
    /// As for `fill`.
    unsafe fn holds(&self, ptr: NonNull<u8>, len: usize, tag: u8) -> bool;
}

/// The byte at offset `i` of check block `tag`.
pub fn pattern(tag: u8, i: usize) -> u8 {
    (i as u8).wrapping_mul(31).wrapping_add(tag)
}

/// Span blocks allocated, written, grown in place and freed; a vmap block
/// of discontiguous frames grown in place and then moved without a copy;
/// and `dma::bus_address` refusing both windows' addresses. Each step is
/// checked through the bytes the CPU reads.
pub fn run(layers: &mut impl Layers) -> Result<(), &'static str> {
    let small = Layout::from_size_align(16 << 10, 4096).map_err(|_| "layout")?;
    let a = layers.allocate(small).ok_or("no span block")?;
    let b = layers.allocate(small).ok_or("no second span block")?;
    if layers.place(a) != Place::Span || a.as_ptr() as usize % 4096 != 0 {
        return Err("a span block is outside the span window or misaligned");
    }
    // SAFETY: each block is live for the lengths named.
    unsafe {
        layers.fill(a, small.size(), 1);
        layers.fill(b, small.size(), 2);
        // `b` follows `a`, so `a` has no room after it; free `b` and grow `a`.
        layers.deallocate(b, small);
        let grown = layers.reallocate(a, small, 64 << 10).ok_or("a span block did not grow")?;
        if grown != a || !layers.holds(a, small.size(), 1) {
            return Err("a span block with free space after it moved or lost bytes when it grew");
        }
        layers.deallocate(a, Layout::from_size_align_unchecked(64 << 10, 4096));
    }
    if !layers.dma_refused(a, 4096) {
        return Err("dma::bus_address accepted a span address");
    }

    let big = Layout::from_size_align(4 << 20, 8).map_err(|_| "layout")?;
    let v = layers.allocate(big).ok_or("no vmap block")?;
    if layers.place(v) != Place::Vmap {
        return Err("a vmap block is outside the vmap window");
    }
    // SAFETY: `v` is live for each length named, and each resize keeps its bytes.
    unsafe {
        layers.fill(v, big.size(), 3);
        let grown = layers.reallocate(v, big, 6 << 20).ok_or("a vmap block did not grow")?;
        // A second block right after it blocks the next growth, which moves.
        let blocker = layers.allocate(big).ok_or("no second vmap block")?;
        let six = Layout::from_size_align_unchecked(6 << 20, 8);
        let moved = layers.reallocate(grown, six, 10 << 20).ok_or("a vmap block did not move")?;
        let ok = grown == v && moved != grown && layers.holds(moved, big.size(), 3);
        let refused = layers.dma_refused(moved, 8192);
        layers.deallocate(blocker, big);
        layers.deallocate(moved, Layout::from_size_align_unchecked(10 << 20, 8));
        if !ok {
            Err("a vmap block lost bytes or did not stay in place when it grew")
        } else if !refused {
            Err("dma::bus_address accepted a vmap address")
        } else {
            Ok(())
        }
    }
}
