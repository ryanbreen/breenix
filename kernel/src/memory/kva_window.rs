//! The kernel VA windows (E1 plan, 6.5): their sizes and places in the
//! kernel half, the VA each hands out, and flush before free for what is
//! unmapped from them. No kernel dependencies, so `tools/kmem-host-tests`
//! compiles this file as is; `memory/kva.rs` is the kernel side.
//!
//! An unmap invalidates the leaves but keeps their addresses in the invalid
//! descriptors, and holds the extent back from the free trees. The frames
//! and the VA come back only through a `Flushed` batch, which only
//! `Batch::flush` makes, after it has handed each leaf to the TLB flush.

use super::kva_extent::{Allocation, Extent, ExtentError, Extents, Retired};
use super::kva_map::{self, Format, TableMemory, UPPER_BASE};

pub const PAGE: u64 = 4096;
pub const GIB: u64 = 1 << 30;
/// The VA one root entry maps: one x86 PML4 slot or one ARM64 L0 entry.
pub const SLOT_BYTES: u64 = 1 << 39;
/// A span of the span window (6.4).
pub const SPAN_BYTES: u64 = 2 << 20;
/// Pages a flush names one at a time; see `tlb_request::MAX_GLOBAL_PAGES`.
pub const FLUSH_LIST: usize = 32;
/// Retired bytes past which a window's pending list is flushed (6.5).
pub const PENDING_FLUSH_BYTES: u64 = 16 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WindowId {
    Span = 0,
    Vmap = 1,
    Stack = 2,
}

impl WindowId {
    pub const ALL: [WindowId; 3] = [WindowId::Span, WindowId::Vmap, WindowId::Stack];

    pub fn name(self) -> &'static str {
        match self {
            WindowId::Span => "span",
            WindowId::Vmap => "vmap",
            WindowId::Stack => "stack",
        }
    }

    /// How the window hands out VA: its granule, and the unmapped guard
    /// bytes each allocation carries before and after what it maps. Spans
    /// are 2 MiB slices indexed by `(va - base) >> 21`, so they have no
    /// guard; a vmap allocation has a guard page after it, and a kernel
    /// stack one below it (E1-10).
    pub fn shape(self) -> (u64, u64, u64) {
        match self {
            WindowId::Span => (SPAN_BYTES, 0, 0),
            WindowId::Vmap => (PAGE, 0, PAGE),
            WindowId::Stack => (PAGE, PAGE, 0),
        }
    }

    /// The window's size for `ram_bytes` of RAM, rounded up to 1 GiB: the
    /// span window RAM, the vmap window 4 × RAM, the stack window RAM.
    pub fn bytes(self, ram_bytes: u64) -> u64 {
        let factor = if self == WindowId::Vmap { 4 } else { 1 };
        ram_bytes.saturating_mul(factor).max(1).div_ceil(GIB).saturating_mul(GIB)
    }
}

/// The first VA of root entry `slot`.
pub fn slot_va(slot: usize) -> u64 {
    UPPER_BASE | (slot as u64) << 39
}

/// Where a window sits: root entries `first_slot..first_slot + slots`, from
/// `base` for `bytes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub id: WindowId,
    pub first_slot: usize,
    pub slots: usize,
    pub base: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceError {
    /// No run of `slots` free root entries is left for the window.
    NoSlots { id: WindowId, slots: usize },
}

/// Place the three windows for `ram_bytes` of RAM in root entries
/// `first..end`, in span, vmap, stack order, each in the lowest run of entries
/// above the previous window for which `free` holds. Each window takes
/// ceil(size / 512 GiB) entries of its own.
///
/// When the RAM-sized windows do not fit, each window is halved, keeping
/// their 1 : 4 : 1 proportion down to 1 GiB each, until they do: RAM larger
/// than the kernel half holds leaves smaller windows, which run out of VA
/// first, rather than none. `NoSlots` means not even 1 GiB windows fit.
pub fn place(
    ram_bytes: u64,
    first: usize,
    end: usize,
    free: impl Fn(usize) -> bool,
) -> Result<[Placement; 3], PlaceError> {
    let mut shift = 0;
    loop {
        match place_at(ram_bytes, shift, first, end, &free) {
            Ok(placed) => return Ok(placed),
            Err(error) if scaled_bytes(WindowId::Vmap, ram_bytes, shift) == GIB => return Err(error),
            Err(_) => shift += 1,
        }
    }
}

/// Window `id`'s size for `ram_bytes`, divided by 2^`shift` and rounded up
/// to 1 GiB.
fn scaled_bytes(id: WindowId, ram_bytes: u64, shift: u32) -> u64 {
    id.bytes(ram_bytes)
        .checked_shr(shift)
        .unwrap_or(0)
        .max(1)
        .div_ceil(GIB)
        .saturating_mul(GIB)
}

fn place_at(
    ram_bytes: u64,
    shift: u32,
    first: usize,
    end: usize,
    free: &impl Fn(usize) -> bool,
) -> Result<[Placement; 3], PlaceError> {
    let mut next = first;
    let mut placed = [Placement { id: WindowId::Span, first_slot: 0, slots: 0, base: 0, bytes: 0 }; 3];
    for (place, id) in placed.iter_mut().zip(WindowId::ALL) {
        let bytes = scaled_bytes(id, ram_bytes, shift);
        let slots = bytes.div_ceil(SLOT_BYTES) as usize;
        let mut start = next;
        loop {
            if start + slots > end {
                return Err(PlaceError::NoSlots { id, slots });
            }
            match (start..start + slots).find(|&slot| !free(slot)) {
                Some(taken) => start = taken + 1,
                None => break,
            }
        }
        *place = Placement { id, first_slot: start, slots, base: slot_va(start), bytes };
        next = start + slots;
    }
    Ok(placed)
}

/// One window's VA.
pub struct Window {
    pub placement: Placement,
    pub extents: Extents,
}

impl Window {
    pub const fn new(placement: Placement, granule: u64) -> Self {
        Self { placement, extents: Extents::new(granule) }
    }

    /// Make the whole window free. Needs one spare node.
    pub fn open(&mut self) -> Result<(), ExtentError> {
        self.extents.free(Extent { start: self.placement.base, len: self.placement.bytes })
    }

    /// VA for `len` bytes aligned to `align`, rounded up to the granule, with
    /// the window's guards.
    pub fn reserve(&mut self, len: u64, align: u64) -> Result<Allocation, ExtentError> {
        let (granule, before, after) = self.placement.id.shape();
        let len = len.checked_next_multiple_of(granule).ok_or(ExtentError::BadRequest)?;
        self.extents.allocate(len, align, before, after)
    }

    /// Unmap `allocation`: invalidate each leaf it mapped and hold its extent
    /// back until a flush covers it. An allocation with no leaf goes straight
    /// back to the free trees. Returns the leaves invalidated. Needs no
    /// spare node: the allocation has held one since `reserve`.
    pub fn unmap<F: Format, M: TableMemory>(
        &mut self,
        mem: &M,
        root: u64,
        allocation: &Allocation,
    ) -> Result<usize, ExtentError> {
        let leaves = kva_map::invalidate_range::<F, M>(mem, root, allocation.start, allocation.len, |_| {});
        if leaves == 0 {
            self.extents.give_back(allocation.extent)?;
        } else {
            self.extents.retire(allocation.extent)?;
        }
        Ok(leaves)
    }

    /// Take the extents awaiting a flush.
    pub fn take_pending(&mut self) -> Batch {
        Batch { retired: self.extents.take_retired() }
    }

    /// Return a flushed batch's VA to the free trees.
    pub fn release(&mut self, flushed: Flushed) -> Result<(), ExtentError> {
        self.extents.release(flushed.retired)
    }
}

/// How a CPU family's TLB flush wants a batch's pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushPolicy {
    /// One flush per batch: the pages when there are at most `FLUSH_LIST`,
    /// otherwise one full flush (x86, where each flush is a shootdown).
    Batched,
    /// Each page, `FLUSH_LIST` at a time (ARM64, where the TLBI broadcast
    /// is per page and needs no interrupt).
    Chunked,
}

/// The flush a kernel unmap needs before its frames are released: each
/// CPU, the initiator included, stops translating the pages, global
/// translations included. `tlb_request::KernelUnmap` is x86's, and
/// `tlb::Arm64KernelUnmap` ARM64's.
pub trait KernelTlb {
    /// How this flush wants a batch's pages.
    const POLICY: FlushPolicy;
    /// Flush `pages`, at most `FLUSH_LIST` of them.
    fn flush_pages(&mut self, pages: &[u64]);
    /// Flush each translation: a batch too large for one page list.
    fn flush_all(&mut self);
}

/// Extents awaiting a flush, detached from their window.
pub struct Batch {
    retired: Retired,
}

/// A batch whose leaves have been flushed: the only way to their frames.
pub struct Flushed {
    retired: Retired,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.retired.is_empty()
    }

    /// Hand each invalidated leaf of the batch to `tlb`, as its policy asks.
    pub fn flush<F: Format, M: TableMemory, T: KernelTlb>(self, mem: &M, root: u64, tlb: &mut T) -> Flushed {
        let policy = T::POLICY;
        let mut list = [0u64; FLUSH_LIST];
        let mut listed = 0;
        let mut leaves = 0;
        for extent in self.retired.extents() {
            kva_map::invalidated_leaves::<F, M>(mem, root, extent.start, extent.len, |va| {
                if listed == FLUSH_LIST && policy == FlushPolicy::Chunked {
                    tlb.flush_pages(&list);
                    listed = 0;
                }
                if listed < FLUSH_LIST {
                    list[listed] = va;
                    listed += 1;
                }
                leaves += 1;
            });
        }
        match policy {
            FlushPolicy::Batched if leaves > FLUSH_LIST => tlb.flush_all(),
            _ if listed != 0 => tlb.flush_pages(&list[..listed]),
            _ => {}
        }
        Flushed { retired: self.retired }
    }
}

impl Flushed {
    /// Clear the batch's invalidated leaves and pass each physical range
    /// they mapped to `frames` as `(pa, bytes)`. Returns the leaves.
    pub fn reclaim<F: Format, M: TableMemory>(
        &self,
        mem: &M,
        root: u64,
        mut frames: impl FnMut(u64, u64),
    ) -> usize {
        self.retired
            .extents()
            .map(|extent| kva_map::reclaim_range::<F, M>(mem, root, extent.start, extent.len, &mut frames))
            .sum()
    }
}
