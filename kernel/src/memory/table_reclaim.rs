//! Shared selection of empty user page-table frames after an unmap.
//!
//! With 4 KiB pages and 512 entries per table, the three non-root tables
//! cover 2 MiB, 1 GiB and 512 GiB windows. A window can be reclaimed when it
//! is empty and fits between the previous VMA's end and the next VMA's start.
//! An empty descriptor alone is insufficient: a neighbouring VMA may still
//! need the table for pages it has not faulted in yet.

use core::ops::Range;

const ENTRIES: usize = 512;
const LEAF_TABLE_SPAN: u64 = 1 << 21;
const ROOT_ENTRY_SPAN: u64 = 1 << 39;
const ROOT_SPAN: u64 = 1 << 48;

/// Descriptor operations for a stable, four-level page-table tree.
///
/// The adapter owns table-frame custody and excludes concurrent software
/// walks and mutations while reclaiming. Inherited/shared tables must not be
/// exposed as reclaimable children. Hardware translation invalidation and
/// deferred frame return remain the caller's responsibility.
pub trait TableOps {
    /// A handle used to access a table's descriptors.
    type Table: Copy;
    /// Custody of an unlinked table frame, suitable for deferred return.
    type Frame;

    /// Decode an owned child table. Report absent entries, block mappings
    /// and tables outside this adapter's reclaimable custody without a child.
    /// Called only above the final page-descriptor level.
    fn child_table(&self, table: Self::Table, index: usize) -> Option<Self::Table>;

    /// Whether the entry has neither a mapping nor software state to retain.
    /// Indices are in 0..512, including entries outside the unmapped range.
    fn entry_empty(&self, table: Self::Table, index: usize) -> bool;

    /// Clear the parent descriptor and transfer custody of its child frame.
    /// The child has been checked for emptiness. Clear the link before
    /// returning custody; do not release the physical frame here.
    fn unlink(&mut self, parent: Self::Table, index: usize) -> Self::Frame;
}

/// Reclaim empty table windows touched by `unmapped`, in bottom-up order.
///
/// `bounds` is `previous_vma_end..next_vma_start`, after updating the VMA set
/// for this unmap. Use 0 for an absent previous VMA and `user_end` for an
/// absent next VMA. `user_end` is the exclusive user-address limit (which may
/// differ between address spaces); the walk and bounds are clipped to it
/// and to the four-level tree. A table's complete span must fit in those
/// bounds, even when only part of that span was unmapped. The root stays.
///
/// `unlinked` receives frame custody after the descriptor is cleared. It must
/// retain that custody until the required TLB/walk-cache invalidations and
/// translation quiescence have completed, before returning the frame to the
/// allocator. This function only selects and unlinks tables; it does not
/// invalidate translations. Returns the number of transferred frames.
pub fn reclaim_range<O: TableOps>(
    ops: &mut O,
    root: O::Table,
    unmapped: Range<u64>,
    bounds: Range<u64>,
    user_end: u64,
    unlinked: impl FnMut(O::Frame),
) -> usize {
    let limit = user_end.min(ROOT_SPAN);
    let floor = bounds.start.min(limit);
    let ceiling = bounds.end.min(limit);
    let start = unmapped.start.max(floor).min(limit);
    let end = unmapped.end.min(ceiling);
    if start >= end {
        return 0;
    }

    Walker {
        ops,
        floor,
        ceiling,
        unlinked,
    }
    .walk(root, 0, ROOT_ENTRY_SPAN, start..end)
}

struct Walker<'a, O: TableOps, F> {
    ops: &'a mut O,
    floor: u64,
    ceiling: u64,
    unlinked: F,
}

impl<O: TableOps, F: FnMut(O::Frame)> Walker<'_, O, F> {
    fn walk(
        &mut self,
        table: O::Table,
        table_base: u64,
        child_span: u64,
        range: Range<u64>,
    ) -> usize {
        let first = ((range.start - table_base) / child_span) as usize;
        let last = ((range.end - 1 - table_base) / child_span) as usize;
        let mut reclaimed = 0;
        for index in first..=last {
            let Some(child) = self.ops.child_table(table, index) else {
                continue;
            };
            let base = table_base + index as u64 * child_span;
            let end = base + child_span;
            if child_span > LEAF_TABLE_SPAN {
                reclaimed += self.walk(
                    child,
                    base,
                    child_span / ENTRIES as u64,
                    range.start.max(base)..range.end.min(end),
                );
            }
            // Scan after the children have been unlinked, including entries
            // outside the unmap that could keep this parent table live.
            if base >= self.floor
                && end <= self.ceiling
                && (0..ENTRIES).all(|entry| self.ops.entry_empty(child, entry))
            {
                let frame = self.ops.unlink(table, index);
                (self.unlinked)(frame);
                reclaimed += 1;
            }
        }
        reclaimed
    }
}
