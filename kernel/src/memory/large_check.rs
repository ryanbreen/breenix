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

/// The check's live blocks, each with the layout it was allocated or last
/// resized for, so each exit frees them.
struct Held {
    blocks: [Option<(NonNull<u8>, Layout)>; 2],
}

impl Held {
    fn add(&mut self, ptr: NonNull<u8>, layout: Layout) -> usize {
        let slot = self.blocks.iter().position(Option::is_none).expect("the check holds at most two blocks");
        self.blocks[slot] = Some((ptr, layout));
        slot
    }

    fn get(&self, slot: usize) -> NonNull<u8> {
        self.blocks[slot].expect("a held block").0
    }

    fn set(&mut self, slot: usize, ptr: NonNull<u8>, layout: Layout) {
        self.blocks[slot] = Some((ptr, layout));
    }

    /// # Safety
    ///
    /// Each held block is live with its recorded layout.
    unsafe fn free(&mut self, slot: usize, layers: &mut impl Layers) {
        if let Some((ptr, layout)) = self.blocks[slot].take() {
            unsafe { layers.deallocate(ptr, layout) };
        }
    }
}

/// Span blocks allocated, written, grown in place and freed; a vmap block
/// of discontiguous frames grown in place and then moved without a copy;
/// and `dma::bus_address` refusing both windows' addresses. Each step is
/// checked through the bytes the CPU reads, and each exit frees the
/// check's live blocks.
///
/// The check holds one span and seven vmap pages of data frames at most. At
/// boot those frames come from the frame allocator's sequential frontier,
/// and freeing them puts them on its free list, so the frontier that
/// contiguous runs for 32-bit DMA drivers come from moves past them. The
/// span is the layer's own 2 MiB unit, which the arena then keeps as its
/// empty span; a vmap block aligned above a page routes to vmap at any
/// size (6.1 row 3), so 8 KiB blocks prove the growth and the move.
pub fn run(layers: &mut impl Layers) -> Result<(), &'static str> {
    let mut held = Held { blocks: [None; 2] };
    let result = steps(layers, &mut held);
    for slot in 0..held.blocks.len() {
        // SAFETY: `steps` records each live block with its current layout.
        unsafe { held.free(slot, layers) };
    }
    result
}

fn steps(layers: &mut impl Layers, held: &mut Held) -> Result<(), &'static str> {
    let small = Layout::from_size_align(16 << 10, 4096).map_err(|_| "layout")?;
    let a = held.add(layers.allocate(small).ok_or("no span block")?, small);
    let b = held.add(layers.allocate(small).ok_or("no second span block")?, small);
    let (pa, pb) = (held.get(a), held.get(b));
    if layers.place(pa) != Place::Span || pa.as_ptr() as usize % 4096 != 0 {
        return Err("a span block is outside the span window or misaligned");
    }
    if !layers.dma_refused(pa, 4096) {
        return Err("dma::bus_address accepted a span address");
    }
    let grown_span = Layout::from_size_align(64 << 10, 4096).map_err(|_| "layout")?;
    // SAFETY: each block is live for the lengths named.
    unsafe {
        layers.fill(pa, small.size(), 1);
        layers.fill(pb, small.size(), 2);
        // `b` follows `a`, so `a` has no room after it; free `b` and grow `a`.
        held.free(b, layers);
        let grown = layers.reallocate(pa, small, grown_span.size()).ok_or("a span block did not grow")?;
        held.set(a, grown, grown_span);
        if grown != pa || !layers.holds(grown, small.size(), 1) {
            return Err("a span block with free space after it moved or lost bytes when it grew");
        }
        held.free(a, layers);
    }

    let block = Layout::from_size_align(8 << 10, 8 << 10).map_err(|_| "layout")?;
    let grown_vmap = Layout::from_size_align(12 << 10, 8 << 10).map_err(|_| "layout")?;
    let moved_vmap = Layout::from_size_align(20 << 10, 8 << 10).map_err(|_| "layout")?;
    let v = held.add(layers.allocate(block).ok_or("no vmap block")?, block);
    let pv = held.get(v);
    if layers.place(pv) != Place::Vmap {
        return Err("a vmap block is outside the vmap window");
    }
    // SAFETY: `v` is live for each length named, and each resize keeps its bytes.
    unsafe {
        layers.fill(pv, block.size(), 3);
        let grown = layers.reallocate(pv, block, grown_vmap.size()).ok_or("a vmap block did not grow")?;
        held.set(v, grown, grown_vmap);
        if grown != pv {
            return Err("a vmap block with free VA after it moved when it grew");
        }
        // A second block right after it blocks the next growth, which moves.
        held.add(layers.allocate(block).ok_or("no second vmap block")?, block);
        let moved = layers.reallocate(grown, grown_vmap, moved_vmap.size()).ok_or("a vmap block did not move")?;
        held.set(v, moved, moved_vmap);
        if moved == grown || !layers.holds(moved, block.size(), 3) {
            return Err("a blocked vmap block did not move or lost bytes when it moved");
        }
        if !layers.dma_refused(moved, 8192) {
            return Err("dma::bus_address accepted a vmap address");
        }
    }
    Ok(())
}
