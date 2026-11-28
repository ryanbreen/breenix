//! vmap allocations (E1 plan, 6.5): requests above 512 KiB, and requests
//! aligned above a page at any size, as pages of order-0 frames mapped into
//! the vmap window. The frames need not be contiguous; each allocation has
//! an unmapped guard page after it. No kernel dependencies, so
//! `tools/kmem-host-tests` compiles this file as is; `memory/large.rs`
//! supplies the window, the tables and the frames through `Space`.
//!
//! A resize stays in place where it can. A block shrinks by recording its new
//! size, keeping its pages for a later growth or its free. It grows into the
//! VA after it when that is free, mapping new pages there. Otherwise its
//! pages move to a new range without their bytes being copied: the new
//! range maps the same frames, and the old one is forwarded, so its leaves
//! wait for the flush like an unmap's and give no frame back.

use super::kva_extent::Allocation;

pub const PAGE: u64 = 4096;

/// One vmap allocation: its VA, the bytes mapped from its start, and the
/// size and alignment it was allocated or last resized for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub allocation: Allocation,
    pub mapped: u64,
    pub size: usize,
    pub align: u64,
}

/// The vmap window as the layer reaches it.
pub trait Space {
    type Error: Copy;
    /// VA for `len` bytes aligned to `align`, with a guard page after it.
    fn reserve(&mut self, len: u64, align: u64) -> Result<Allocation, Self::Error>;
    /// Grow `allocation` by `extra` bytes into the free VA after it, if
    /// there is that much.
    fn extend(&mut self, allocation: &Allocation, extra: u64) -> Option<Allocation>;
    /// Map `[allocation.start + offset, + len)` to new frames whole, or leave
    /// it unmapped.
    fn map_fresh(&mut self, allocation: &Allocation, offset: u64, len: u64) -> Result<(), Self::Error>;
    /// Map `to` whole, or leave it unmapped: its first `moved` bytes to the
    /// frames `from` maps at the same offsets, and the rest to new frames.
    fn map_moved(&mut self, from: &Allocation, to: &Allocation, moved: u64) -> Result<(), Self::Error>;
    /// Unmap `allocation`, whose frames `map_moved` gave to another range.
    fn forward(&mut self, allocation: Allocation);
    /// Unmap `allocation`; its frames and VA come back after the flush that
    /// covers them.
    fn release(&mut self, allocation: Allocation);
}

/// The VA alignment for a request aligned to `align`: at least a page.
fn page_align(align: usize) -> u64 {
    (align as u64).max(PAGE)
}

/// Allocate `size` bytes aligned to `align`.
pub fn allocate<S: Space>(space: &mut S, size: usize, align: usize) -> Result<Block, S::Error> {
    let len = (size as u64).max(1).next_multiple_of(PAGE);
    let align = page_align(align);
    let allocation = space.reserve(len, align)?;
    if let Err(error) = space.map_fresh(&allocation, 0, len) {
        space.release(allocation);
        return Err(error);
    }
    Ok(Block { allocation, mapped: len, size, align })
}

/// How a resize was met.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resized {
    /// The block kept its address and pages.
    InPlace,
    /// The block's pages moved to a new address with their bytes.
    Moved,
}

/// Resize `block` to `new_size` bytes, keeping its bytes up to the smaller
/// size. An error leaves the block at its address with its bytes, mapped as
/// it was.
pub fn resize<S: Space>(space: &mut S, block: &mut Block, new_size: usize) -> Result<Resized, S::Error> {
    let len = (new_size as u64).max(1).next_multiple_of(PAGE);
    if len <= block.mapped {
        block.size = new_size;
        return Ok(Resized::InPlace);
    }
    if len > block.allocation.len {
        if let Some(grown) = space.extend(&block.allocation, len - block.allocation.len) {
            block.allocation = grown;
        }
    }
    if len <= block.allocation.len {
        space.map_fresh(&block.allocation, block.mapped, len - block.mapped)?;
        block.mapped = len;
        block.size = new_size;
        return Ok(Resized::InPlace);
    }
    let to = space.reserve(len, block.align)?;
    if let Err(error) = space.map_moved(&block.allocation, &to, block.mapped) {
        space.release(to);
        return Err(error);
    }
    space.forward(block.allocation);
    *block = Block { allocation: to, mapped: len, size: new_size, align: block.align };
    Ok(Resized::Moved)
}

/// Free `block`.
pub fn free<S: Space>(space: &mut S, block: Block) {
    space.release(block.allocation);
}
