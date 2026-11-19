//! Space-tree item operations from breenfs design sections 2.4 and 2.5.
//!
//! The caller supplies a private transaction store/root. As with `btree`, a
//! fallible store must roll back a failed edit; this layer does not publish roots
//! or perform I/O. The transaction layer owns space-tree node allocation and the
//! deferred refcount fixpoint. Ranges supplied to `add_free` must be unowned and
//! outside reserved blocks. FS child references are managed by txn/the reaper.
use super::preempt::preemption_point;
use super::{
    btree::{self, NodeStore},
    format::{self, BlockPtr, Free, InternalEntry, Key, LeafItemHeader, NodeHeader, Pending, Ref},
};
use alloc::{collections::BTreeSet, vec::Vec};

const FREE_OBJECT: u64 = 0xffff_ffff_ffff_ff20;
const PENDING_OBJECT: u64 = 0xffff_ffff_ffff_ff30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    ENOSPC,
    EMLINK,
    Invalid,
    Tree(btree::Error),
}
impl From<btree::Error> for Error {
    fn from(value: btree::Error) -> Self {
        Self::Tree(value)
    }
}
impl From<format::Error> for Error {
    fn from(_: format::Error) -> Self {
        Self::Invalid
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Kind {
    FsNode = 1,
    SpaceNode = 2,
    Data = 3,
}

fn key(object: u64, item_type: u8, offset: u64) -> Key {
    Key {
        object,
        item_type,
        offset,
    }
}
fn end(start: u64, length: u64) -> Result<u64, Error> {
    if length == 0 {
        return Err(Error::Invalid);
    }
    start.checked_add(length).ok_or(Error::Invalid)
}
fn valid_ref(length: u64, r: &Ref) -> bool {
    r.refcount != 0
        && r.alloc_generation != 0
        && match r.kind {
            1 => length == 4 && r.crcs.is_empty(),
            2 => length == 4 && r.crcs.is_empty() && r.refcount == 1,
            3 => (1..=256).contains(&length) && r.crcs.len() as u64 == length,
            _ => false,
        }
}

/// Read one exact physical allocation, including its per-block data checksums.
pub fn reference<S: NodeStore>(
    store: &S,
    root: BlockPtr,
    start: u64,
    length: u64,
) -> Result<Option<Ref>, Error> {
    end(start, length)?;
    let Some(bytes) = btree::search(store, root, key(start, format::REF, length))? else {
        return Ok(None);
    };
    let r = Ref::decode(&bytes)?;
    if !valid_ref(length, &r) {
        return Err(Error::Invalid);
    }
    Ok(Some(r))
}

// Only descend into branches intersecting this object's item-type range. The
// generic B-tree has point lookup, so use it to validate each collected payload
// and its ancestor bounds rather than maintaining a second item decoder here.
fn range<S: NodeStore>(
    store: &S,
    root: BlockPtr,
    object: u64,
    ty: u8,
) -> Result<Vec<(Key, Vec<u8>)>, Error> {
    let lower = key(object, ty, 0);
    let upper = key(object, ty, u64::MAX);
    let mut todo = alloc::vec![root];
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    let uuid = NodeHeader::decode(&store.get(root.addr)?)?.fs_uuid;
    while let Some(ptr) = todo.pop() {
        preemption_point();
        if ptr.level >= 8 || !seen.insert(ptr.addr) {
            return Err(Error::Invalid);
        }
        let bytes = store.get(ptr.addr)?;
        if bytes.len() != format::NODE_SIZE {
            return Err(Error::Invalid);
        }
        let h = NodeHeader::decode(&bytes)?;
        if h.tree != 2
            || h.fs_uuid != uuid
            || h.addr != ptr.addr
            || h.level != ptr.level
            || h.generation != ptr.generation
            || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != ptr.csum
        {
            return Err(Error::Invalid);
        }
        let count = usize::from(h.nitems);
        let width = if h.level == 0 { 21 } else { 37 };
        if 48 + count * width > bytes.len() || (h.level > 0 && count > 441) {
            return Err(Error::Invalid);
        }
        if h.level == 0 {
            for i in 0..count {
                preemption_point();
                let at = 48 + i * 21;
                let k = LeafItemHeader::decode(&bytes[at..at + 21])?.key;
                if k >= lower && k <= upper {
                    let payload = btree::search(store, root, k)?.ok_or(Error::Invalid)?;
                    out.push((k, payload));
                }
            }
        } else {
            let mut entries = Vec::new();
            for i in 0..count {
                preemption_point();
                let at = 48 + i * 37;
                entries.push(InternalEntry::decode(&bytes[at..at + 37])?);
            }
            for (i, e) in entries.iter().enumerate().rev() {
                preemption_point();
                if e.key <= upper && entries.get(i + 1).is_none_or(|next| next.key > lower) {
                    todo.push(BlockPtr {
                        addr: e.child_addr,
                        generation: e.child_generation,
                        csum: e.child_csum,
                        level: h.level - 1,
                    });
                }
            }
        }
    }
    out.sort_by_key(|item| { preemption_point(); item.0 });
    Ok(out)
}
fn free_record(key: Key, bytes: &[u8]) -> Result<(u64, u64), Error> {
    if bytes.len() != 8 {
        return Err(Error::Invalid);
    }
    let length = Free::decode(bytes)?.length;
    end(key.offset, length)?;
    Ok((key.offset, length))
}

fn free_key(start: u64) -> Key {
    key(FREE_OBJECT, format::FREE, start)
}

fn merged(mut ranges: Vec<(u64, u64)>) -> Result<Vec<(u64, u64)>, Error> {
    ranges.sort_unstable_by(|a, b| { preemption_point(); a.cmp(b) });
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (start, length) in ranges {
        preemption_point();
        end(start, length)?;
        if let Some((prev, len)) = out.last_mut() {
            let prev_end = end(*prev, *len)?;
            if prev_end > start {
                return Err(Error::Invalid);
            }
            if prev_end == start {
                *len = end(start, length)? - *prev;
                continue;
            }
        }
        out.push((start, length));
    }
    Ok(out)
}

/// Add unowned space and coalesce adjacent FREE items (design invariant A2).
pub fn add_free<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    start: u64,
    length: u64,
) -> Result<(), Error> {
    let mut start = start;
    let mut stop = end(start, length)?;
    let mut left = None;
    let mut right = None;
    // FREE is ordered by physical start and already coalesced. Only the
    // predecessor and successor can overlap or adjoin a returned range.
    btree::visit_range_reverse(store, *root, free_key(0), free_key(start), &mut |key, bytes| {
        let (addr, length) = free_record(key, bytes)?;
        let end = end(addr, length)?;
        if end > start { return Err(Error::Invalid); }
        if end == start { left = Some(addr); }
        Ok(false)
    })?;
    btree::visit_range(store, *root, free_key(start + 1), free_key(u64::MAX), &mut |key, bytes| {
        let (addr, length) = free_record(key, bytes)?;
        if addr < stop { return Err(Error::Invalid); }
        if addr == stop { right = Some((addr, end(addr, length)?)); }
        Ok(false)
    })?;
    if let Some(addr) = left {
        btree::delete(store, root, free_key(addr))?;
        start = addr;
    }
    if let Some((addr, end)) = right {
        btree::delete(store, root, free_key(addr))?;
        stop = end;
    }
    btree::insert(store, root, free_key(start), &Free { length: stop - start }.encode())?;
    Ok(())
}

/// Last publication needed to include the current PENDING ranges in FREE.
/// This observes the quarantine; promotion still goes through promote_pending.
/// Admission snapshots this bound once, rather than chasing later releases.
#[cfg(test)]
pub fn pending_promotion_generation<S: NodeStore>(
    store: &S,
    root: BlockPtr,
) -> Result<Option<u64>, Error> {
    let mut generation = None;
    for (key, bytes) in range(store, root, PENDING_OBJECT, format::PENDING)? {
        preemption_point();
        let pending = Pending::decode(&bytes)?;
        end(key.offset, pending.length)?;
        if pending.released_generation == 0 {
            return Err(Error::Invalid);
        }
        let promoted = pending
            .released_generation
            .checked_add(2)
            .ok_or(Error::Invalid)?;
        generation = Some(generation.map_or(promoted, |old: u64| old.max(promoted)));
    }
    Ok(generation)
}

/// A fixed-size account of an installed generation's quarantine.
#[derive(Clone, Copy, Default)]
pub struct PendingSummary {
    blocks: u128,
    oldest: u64,
    newest: u64,
}
impl PendingSummary {
    /// At mount, reuse the records already read by recovery.
    pub fn from_items(items: &[(Key, Vec<u8>)]) -> Result<Self, Error> {
        let mut summary = Self::default();
        for (key, bytes) in items {
            preemption_point();
            if key.object != PENDING_OBJECT || key.item_type != format::PENDING {
                continue;
            }
            let pending = Pending::decode(bytes)?;
            end(key.offset, pending.length)?;
            if pending.released_generation == 0 {
                return Err(Error::Invalid);
            }
            summary.include(pending.length, pending.released_generation);
        }
        Ok(summary)
    }
    pub(super) fn include(&mut self, length: u64, generation: u64) {
        self.blocks += u128::from(length);
        self.oldest = if self.oldest == 0 { generation } else { self.oldest.min(generation) };
        self.newest = self.newest.max(generation);
    }
    pub fn promotion_generation(self) -> Result<Option<u64>, Error> {
        if self.newest == 0 {
            return Ok(None);
        }
        self.newest.checked_add(2).map(Some).ok_or(Error::Invalid)
    }
    pub fn census(self) -> Option<(u64, u64)> {
        Some((u64::try_from(self.blocks).ok()?, self.oldest))
    }
}

/// Commit-time conversion of quarantined space, even with ample FREE space.
/// Allocation still prefers FREE within a transaction (design 2.5 reuse rule).
pub fn promote_pending<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    generation: u64,
) -> Result<PendingSummary, Error> {
    let mut pending_summary = PendingSummary::default();
    for (k, bytes) in range(store, *root, PENDING_OBJECT, format::PENDING)? {
        preemption_point();
        let pending = Pending::decode(&bytes)?;
        end(k.offset, pending.length)?;
        if pending.released_generation == 0 {
            return Err(Error::Invalid);
        }
        if generation
            .checked_sub(2)
            .is_some_and(|limit| pending.released_generation <= limit)
        {
            btree::delete(store, root, k)?;
            add_free(store, root, k.offset, pending.length)?;
        } else {
            pending_summary.include(pending.length, pending.released_generation);
        }
    }
    Ok(pending_summary)
}

/// Allocate a whole REF. Prefer existing FREE space; only if that cannot fit,
/// convert eligible PENDING items to FREE, coalescing before selecting a range.
/// CRCs are supplied from the final data-block bytes by the transaction writer.
pub fn allocate<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    length: u64,
    kind: Kind,
    crcs: &[u32],
    generation: u64,
) -> Result<u64, Error> {
    allocate_extent(store, root, length, kind, crcs, generation, false).map(|(start, _)| start)
}

/// Allocate a data prefix, falling back to the largest eligible run when the
/// requested run cannot fit. The returned length owns only that CRC prefix.
pub fn allocate_data_run<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    crcs: &[u32],
    generation: u64,
) -> Result<(u64, u64), Error> {
    allocate_extent(
        store,
        root,
        crcs.len() as u64,
        Kind::Data,
        crcs,
        generation,
        true,
    )
}

fn allocate_extent<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    length: u64,
    kind: Kind,
    crcs: &[u32],
    generation: u64,
    short_data: bool,
) -> Result<(u64, u64), Error> {
    let mut r = Ref {
        refcount: 1,
        kind: kind as u8,
        alloc_generation: generation,
        crcs: crcs.to_vec(),
    };
    if !valid_ref(length, &r) {
        return Err(Error::Invalid);
    }
    let mut free = Vec::new();
    let mut candidate = None;
    btree::visit_range(store, *root, free_key(0), free_key(u64::MAX), &mut |key, bytes| {
        let range = free_record(key, bytes)?;
        if range.1 >= length {
            candidate = Some(range);
            return Ok::<_, Error>(false);
        }
        free.push(range);
        Ok(true)
    })?;
    if candidate.is_none() {
        let mut pending = Vec::new();
        for (k, bytes) in range(store, *root, PENDING_OBJECT, format::PENDING)? {
            preemption_point();
            if bytes.len() != 16 {
                return Err(Error::Invalid);
            }
            let p = Pending::decode(&bytes)?;
            end(k.offset, p.length)?;
            if p.released_generation == 0 {
                return Err(Error::Invalid);
            }
            if generation
                .checked_sub(2)
                .is_some_and(|limit| p.released_generation <= limit)
            {
                pending.push((k.offset, p.length));
            }
        }
        free.extend(pending.iter().map(|&range| { preemption_point(); range }));
        free = merged(free)?;
        candidate = free.iter().find(|(_, len)| { preemption_point(); *len >= length }).copied();
        if candidate.is_none() && short_data {
            // Break ties by address so replaying this exact shorter allocation
            // in the real SPACE tree makes the same first-fit choice.
            candidate = free
                .iter()
                .copied()
                .max_by_key(|(start, len)| { preemption_point(); (*len, core::cmp::Reverse(*start)) });
        }
        if candidate.is_none() {
            return Err(Error::ENOSPC);
        }
        for (start, length) in pending {
            preemption_point();
            btree::delete(store, root, key(PENDING_OBJECT, format::PENDING, start))?;
            add_free(store, root, start, length)?;
        }
    }
    let (start, available) = candidate.ok_or(Error::ENOSPC)?;
    let length = length.min(available);
    r.crcs.truncate(length as usize);
    btree::delete(store, root, key(FREE_OBJECT, format::FREE, start))?;
    if available > length {
        btree::insert(
            store,
            root,
            key(FREE_OBJECT, format::FREE, start + length),
            &Free {
                length: available - length,
            }
            .encode(),
        )?;
    }
    btree::insert(store, root, key(start, format::REF, length), &r.encode())?;
    Ok((start, length))
}

/// Add an incoming reference. Keep space nodes uniquely owned (design A5).
pub fn increment<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    start: u64,
    length: u64,
) -> Result<(), Error> {
    let mut r = reference(store, *root, start, length)?.ok_or(Error::Invalid)?;
    if r.kind == Kind::SpaceNode as u8 {
        return Err(Error::Invalid);
    }
    r.refcount = r.refcount.checked_add(1).ok_or(Error::EMLINK)?;
    btree::insert(store, root, key(start, format::REF, length), &r.encode())?;
    Ok(())
}

/// Drop an incoming reference in commit G. `referenced_children` means an FS
/// node still owns internal children or data extents. DEAD retains those outgoing
/// references until the reaper processes them. Return whether this release
/// put a range in PENDING.
pub fn release<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    start: u64,
    length: u64,
    generation: u64,
    referenced_children: bool,
) -> Result<bool, Error> {
    let mut r = reference(store, *root, start, length)?.ok_or(Error::Invalid)?;
    if generation < r.alloc_generation || (referenced_children && r.kind != Kind::FsNode as u8) {
        return Err(Error::Invalid);
    }
    r.refcount -= 1;
    if r.refcount > 0 {
        btree::insert(store, root, key(start, format::REF, length), &r.encode())?;
        return Ok(false);
    }
    btree::delete(store, root, key(start, format::REF, length))?;
    if referenced_children {
        btree::insert(store, root, key(start, format::DEAD, 4), &[])?;
    } else {
        btree::insert(
            store,
            root,
            key(PENDING_OBJECT, format::PENDING, start),
            &Pending {
                length,
                released_generation: generation,
            }
            .encode(),
        )?;
    }
    Ok(!referenced_children)
}

/// Collect the DEAD address index once when opening a mount. REF and DEAD keys
/// are interleaved by address; stop before ROOT/FREE/PENDING's object ranges.
pub fn dead_nodes<S: NodeStore>(store: &S, root: BlockPtr) -> Result<BTreeSet<u64>, Error> {
    let mut dead = BTreeSet::new();
    for (k, value) in btree::range(
        store,
        root,
        key(0, 0, 0),
        key(0xffff_ffff_ffff_ff0f, u8::MAX, u64::MAX),
    )? {
        preemption_point();
        if k.item_type == format::DEAD {
            if k.offset != 4 || !value.is_empty() {
                return Err(Error::Invalid);
            }
            dead.insert(k.object);
        }
    }
    Ok(dead)
}

/// Retire a DEAD node after its outgoing references have been released. The
/// caller's transaction makes both changes atomic; reuse still waits for G+2.
pub fn reap<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    addr: u64,
    generation: u64,
) -> Result<(), Error> {
    let dead = key(addr, format::DEAD, 4);
    if btree::search(store, *root, dead)?.as_deref() != Some(&[]) || generation == 0 {
        return Err(Error::Invalid);
    }
    btree::delete(store, root, dead)?;
    btree::insert(
        store,
        root,
        key(PENDING_OBJECT, format::PENDING, addr),
        &Pending {
            length: 4,
            released_generation: generation,
        }
        .encode(),
    )?;
    Ok(())
}
