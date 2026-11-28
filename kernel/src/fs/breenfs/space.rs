//! Space-tree item operations from breenfs design sections 2.4 and 2.5.
//!
//! The caller supplies a private transaction store/root. As with `btree`, a
//! fallible store must roll back a failed edit; this layer does not publish roots
//! or perform I/O. The transaction layer owns allocation policy, the delayed
//! reference heads and space-tree node allocation; this layer edits items.
//!
//! Free space is chosen from `FreeIndex`, an in-memory mirror of the FREE items
//! ordered by start and by length (the pair of by-block and by-size trees of
//! XFS's free-space design), so allocation does not walk FREE on disk. The
//! transaction later removes the chosen range from FREE with `claim`.
//!
//! Released space waits in PENDING until G+2 (design 2.5). `Quarantine` keeps
//! the runs each generation wrote, coalesced, so promotion deletes known keys
//! instead of reading PENDING. PENDING that a mount found on disk is promoted
//! by a streaming scan in batches of `PROMOTE_BATCH` records.
use super::preempt::preemption_point;
use super::{
    btree::{self, NodeStore},
    format::{self, BlockPtr, Free, Key, Pending, Ref},
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};

const FREE_OBJECT: u64 = 0xffff_ffff_ffff_ff20;
const PENDING_OBJECT: u64 = 0xffff_ffff_ffff_ff30;

/// PENDING records a streaming promotion holds at once.
pub const PROMOTE_BATCH: usize = 64;
/// Free ranges examined on each side of an allocation goal before falling
/// back to the best fit by length.
const GOAL_PROBE: usize = 16;
/// Blocks in one FS or SPACE node allocation (design 2.4).
const NODE_BLOCKS: u64 = 4;

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
    r.refcount != 0 && r.alloc_generation != 0 && valid_shape(length, r.kind, r.refcount, r.crcs.len())
}
/// The lengths, counts and checksum counts a REF of `kind` may have (2.4).
pub(super) fn valid_shape(length: u64, kind: u8, refcount: u32, crcs: usize) -> bool {
    match kind {
        1 => length == 4 && crcs == 0,
        2 => length == 4 && crcs == 0 && refcount == 1,
        3 => (1..=256).contains(&length) && crcs as u64 == length,
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

/// Store a REF for an allocation whose count changed in this transaction.
pub fn put_reference<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    start: u64,
    length: u64,
    r: &Ref,
) -> Result<(), Error> {
    if !valid_ref(length, r) {
        return Err(Error::Invalid);
    }
    btree::insert(store, root, key(start, format::REF, length), &r.encode())?;
    Ok(())
}

/// Remove a REF whose count reached 0. `dead` keeps an FS node's outgoing
/// references for the reaper (design 2.6); otherwise the caller quarantines it.
pub fn drop_reference<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    start: u64,
    length: u64,
    dead: bool,
) -> Result<(), Error> {
    if btree::delete(store, root, key(start, format::REF, length))?.is_none() {
        return Err(Error::Invalid);
    }
    if dead {
        mark_dead(store, root, start)?;
    }
    Ok(())
}

/// Mark an unreferenced FS node whose children still count as referenced.
pub fn mark_dead<S: NodeStore>(store: &mut S, root: &mut BlockPtr, addr: u64) -> Result<(), Error> {
    btree::insert(store, root, key(addr, format::DEAD, 4), &[])?;
    Ok(())
}

/// Add `[start, start + length)` to disjoint runs keyed by start, merging it
/// with the run that ends at `start` and the run that begins at its end.
pub fn insert_run(runs: &mut BTreeMap<u64, u64>, start: u64, length: u64) -> Result<(), Error> {
    let mut first = start;
    let mut stop = end(start, length)?;
    if let Some((&s, &l)) = runs.range(..=start).next_back() {
        let e = end(s, l)?;
        if e > start {
            return Err(Error::Invalid);
        }
        if e == start {
            runs.remove(&s);
            first = s;
        }
    }
    if let Some((&s, &l)) = runs.range(start + 1..).next() {
        if s < stop {
            return Err(Error::Invalid);
        }
        if s == stop {
            runs.remove(&s);
            stop = end(s, l)?;
        }
    }
    runs.insert(first, stop - first);
    Ok(())
}

fn free_record(key: Key, bytes: &[u8]) -> Result<(u64, u64), Error> {
    if key.object != FREE_OBJECT || key.item_type != format::FREE || bytes.len() != 8 {
        return Err(Error::Invalid);
    }
    let length = Free::decode(bytes)?.length;
    end(key.offset, length)?;
    Ok((key.offset, length))
}
fn pending_record(key: Key, bytes: &[u8]) -> Result<(u64, u64, u64), Error> {
    if key.object != PENDING_OBJECT || key.item_type != format::PENDING || bytes.len() != 16 {
        return Err(Error::Invalid);
    }
    let p = Pending::decode(bytes)?;
    end(key.offset, p.length)?;
    if p.released_generation == 0 {
        return Err(Error::Invalid);
    }
    Ok((key.offset, p.length, p.released_generation))
}

fn free_key(start: u64) -> Key {
    key(FREE_OBJECT, format::FREE, start)
}
fn pending_key(start: u64) -> Key {
    key(PENDING_OBJECT, format::PENDING, start)
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

/// Remove `[start, start + length)` from the one FREE item that contains it,
/// keeping the remainders on either side. The range was chosen from the
/// transaction's FreeIndex, which mirrors FREE item for item.
pub fn claim<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    start: u64,
    length: u64,
) -> Result<(), Error> {
    let stop = end(start, length)?;
    let mut found = None;
    btree::visit_range_reverse(store, *root, free_key(0), free_key(start), &mut |key, bytes| {
        found = Some(free_record(key, bytes)?);
        Ok::<_, Error>(false)
    })?;
    let (addr, available) = found.ok_or(Error::Invalid)?;
    let item_end = end(addr, available)?;
    if item_end < stop {
        return Err(Error::Invalid);
    }
    if addr < start {
        btree::insert(store, root, free_key(addr), &Free { length: start - addr }.encode())?;
    } else {
        btree::delete(store, root, free_key(addr))?;
    }
    if stop < item_end {
        btree::insert(store, root, free_key(stop), &Free { length: item_end - stop }.encode())?;
    }
    Ok(())
}

/// The FREE items of a space tree, by start and by length. Insertion coalesces
/// with the immediate neighbors only, exactly as `add_free` does on disk, and
/// removal splits one range as `claim` does, so the index and the FREE items
/// stay equal item for item.
#[derive(Clone, Default)]
pub struct FreeIndex {
    by_start: BTreeMap<u64, u64>,
    by_length: BTreeSet<(u64, u64)>,
}
impl FreeIndex {
    fn put(&mut self, start: u64, length: u64) {
        self.by_start.insert(start, length);
        self.by_length.insert((length, start));
    }
    fn take(&mut self, start: u64) -> Option<u64> {
        let length = self.by_start.remove(&start)?;
        self.by_length.remove(&(length, start));
        Some(length)
    }
    /// Append a FREE item read in key order at mount. Items must not overlap.
    fn push(&mut self, start: u64, length: u64) -> Result<(), Error> {
        end(start, length)?;
        if let Some((&s, &l)) = self.by_start.last_key_value() {
            if end(s, l)? > start {
                return Err(Error::Invalid);
            }
        }
        self.put(start, length);
        Ok(())
    }
    /// Return unowned space (promotion), coalescing as `add_free` does.
    pub fn insert(&mut self, start: u64, length: u64) -> Result<(), Error> {
        let mut first = start;
        let mut stop = end(start, length)?;
        if let Some((&s, &l)) = self.by_start.range(..=start).next_back() {
            let e = end(s, l)?;
            if e > start {
                return Err(Error::Invalid);
            }
            if e == start {
                self.take(s);
                first = s;
            }
        }
        if let Some((&s, &l)) = self.by_start.range(start + 1..).next() {
            if s < stop {
                return Err(Error::Invalid);
            }
            if s == stop {
                self.take(s);
                stop = end(s, l)?;
            }
        }
        self.put(first, stop - first);
        Ok(())
    }
    /// Remove an allocation from the one range that contains it.
    pub fn remove(&mut self, start: u64, length: u64) -> Result<(), Error> {
        let stop = end(start, length)?;
        let (&s, &l) = self.by_start.range(..=start).next_back().ok_or(Error::Invalid)?;
        let range_end = end(s, l)?;
        if range_end < stop {
            return Err(Error::Invalid);
        }
        self.take(s);
        if s < start {
            self.put(s, start - s);
        }
        if stop < range_end {
            self.put(stop, range_end - stop);
        }
        Ok(())
    }
    /// Choose `length` blocks. With a goal, the run starting at the goal, else
    /// the nearest range after it that fits, else the nearest before it, each
    /// within GOAL_PROBE ranges. Otherwise the smallest range that fits, lowest
    /// address first. `short` falls back to the largest range.
    ///
    /// A goal inside a range would split it in two. It is taken only when
    /// both pieces left free can still hold a node: otherwise sparse writes
    /// could turn every node-sized range into smaller holes while most of the
    /// space is free, and settlement could not copy a node.
    pub fn choose(&self, length: u64, goal: Option<u64>, short: bool) -> Option<(u64, u64)> {
        if length == 0 {
            return None;
        }
        if let Some(goal) = goal {
            if let Some((&s, &l)) = self.by_start.range(..=goal).next_back() {
                let room = s.checked_add(l).and_then(|e| e.checked_sub(goal));
                let node_sized = |piece: u64| piece == 0 || piece >= NODE_BLOCKS;
                if room.is_some_and(|room| room >= length
                    && (goal == s || (node_sized(goal - s) && node_sized(room - length))))
                {
                    return Some((goal, length));
                }
            }
            let after = self.by_start.range(goal.saturating_add(1)..).take(GOAL_PROBE)
                .find(|&(_, &l)| { preemption_point(); l >= length });
            if let Some((&s, _)) = after {
                return Some((s, length));
            }
            let before = self.by_start.range(..goal).rev().take(GOAL_PROBE)
                .find(|&(_, &l)| { preemption_point(); l >= length });
            if let Some((&s, &l)) = before {
                return Some((s + l - length, length));
            }
        }
        if let Some(&(_, s)) = self.by_length.range((length, 0)..).next() {
            return Some((s, length));
        }
        if short {
            let &(largest, _) = self.by_length.last()?;
            let &(l, s) = self.by_length.range((largest, 0)..).next()?;
            return Some((s, l));
        }
        None
    }
    /// The ranges in address order (host oracles).
    pub fn ranges(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.by_start.iter().map(|(&s, &l)| (s, l))
    }
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
            let (_, length, generation) = pending_record(*key, bytes)?;
            summary.include(length, generation);
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

/// Last publication needed to include the current PENDING ranges in FREE.
/// This observes the quarantine; promotion still goes through the transaction.
/// Admission snapshots this bound once, rather than chasing later releases.
#[cfg(test)]
pub fn pending_promotion_generation<S: NodeStore>(
    store: &S,
    root: BlockPtr,
) -> Result<Option<u64>, Error> {
    let mut summary = PendingSummary::default();
    btree::visit_range(store, root, pending_key(0), pending_key(u64::MAX), &mut |key, bytes| {
        preemption_point();
        let (_, length, generation) = pending_record(key, bytes)?;
        summary.include(length, generation);
        Ok::<_, Error>(true)
    })?;
    summary.promotion_generation()
}

/// Read FREE into an index and summarize PENDING, streaming, when a mount opens
/// its first transaction, retaining a fixed-size summary of PENDING.
pub fn open<S: NodeStore>(store: &S, root: BlockPtr) -> Result<(FreeIndex, Quarantine), Error> {
    let mut free = FreeIndex::default();
    let mut found = PendingSummary::default();
    btree::visit_range(store, root, free_key(0), pending_key(u64::MAX), &mut |key, bytes| {
        preemption_point();
        if key.object == FREE_OBJECT {
            let (start, length) = free_record(key, bytes)?;
            free.push(start, length)?;
        } else {
            let (_, length, generation) = pending_record(key, bytes)?;
            found.include(length, generation);
        }
        Ok::<_, Error>(true)
    })?;
    let quarantine = Quarantine {
        released: BTreeMap::new(),
        found: (found.newest != 0).then_some(found),
    };
    Ok((free, quarantine))
}

/// One generation's stored PENDING runs and their total length.
#[derive(Clone, Default)]
pub struct Released {
    blocks: u64,
    runs: BTreeMap<u64, u64>,
}
impl Released {
    /// Write staged releases of `generation` as PENDING runs, each merged
    /// with the stored run it adjoins: at most two item edits per staged run.
    pub fn write<S: NodeStore>(
        &mut self,
        store: &mut S,
        root: &mut BlockPtr,
        generation: u64,
        staged: &mut BTreeMap<u64, u64>,
    ) -> Result<(), Error> {
        while let Some((start, length)) = staged.pop_first() {
            preemption_point();
            let stop = end(start, length)?;
            let left = self.runs.range(..start).next_back().map(|(&s, &l)| (s, l));
            if let Some((s, l)) = left {
                if end(s, l)? > start {
                    return Err(Error::Invalid);
                }
            }
            if let Some((&s, _)) = self.runs.range(start..).next() {
                if s < stop {
                    return Err(Error::Invalid);
                }
            }
            let mut first = start;
            let mut last = stop;
            if let Some(right) = self.runs.remove(&stop) {
                if btree::delete(store, root, pending_key(stop))?.is_none() {
                    return Err(Error::Invalid);
                }
                last = end(stop, right)?;
            }
            if let Some((s, _)) = left.filter(|&(s, l)| s + l == start) {
                self.runs.remove(&s);
                first = s;
            }
            let run = Pending { length: last - first, released_generation: generation };
            btree::insert(store, root, pending_key(first), &run.encode())?;
            self.runs.insert(first, last - first);
            self.blocks = self.blocks.checked_add(length).ok_or(Error::Invalid)?;
        }
        Ok(())
    }
}

/// PENDING as this mount wrote it, by released generation: each generation's
/// runs exactly as stored, coalesced. `found` summarizes PENDING that was on
/// disk when the mount opened; those records are not held in memory.
#[derive(Clone, Default)]
pub struct Quarantine {
    released: BTreeMap<u64, Released>,
    found: Option<PendingSummary>,
}
impl Quarantine {
    pub fn summary(&self) -> PendingSummary {
        let mut summary = self.found.unwrap_or_default();
        for (&generation, released) in &self.released {
            summary.include(released.blocks, generation);
        }
        summary
    }
    /// Stored PENDING runs written by this mount, as (start, length, released).
    pub fn runs(&self) -> impl Iterator<Item = (u64, u64, u64)> + '_ {
        self.released.iter().flat_map(|(&g, r)| r.runs.iter().map(move |(&s, &l)| (s, l, g)))
    }
    /// Make the PENDING released at or before `generation - 2` allocatable:
    /// add it to `free` now and return the SPACE edits for settlement.
    /// `base` is the transaction's starting space tree; found records are
    /// streamed from it, and the ones not yet eligible are summarized.
    pub fn promote<S: NodeStore>(
        &mut self,
        base: &S,
        root: BlockPtr,
        generation: u64,
        free: &mut FreeIndex,
    ) -> Result<Promotion, Error> {
        let mut promotion = Promotion::default();
        let Some(limit) = generation.checked_sub(2) else {
            return Ok(promotion);
        };
        // The eligible generations' runs move to the promotion whole: a
        // copy would be one buffer in proportion to the runs.
        let later = self.released.split_off(&limit.saturating_add(1));
        promotion.released = core::mem::replace(&mut self.released, later);
        for released in promotion.released.values() {
            for (&start, &length) in &released.runs {
                preemption_point();
                free.insert(start, length)?;
            }
        }
        if let Some(found) = self.found {
            if found.oldest <= limit {
                // Records this mount wrote are newer than the found ones (mount generation).
                let found_limit = limit.min(found.newest);
                let mut waiting = PendingSummary::default();
                btree::visit_range(base, root, pending_key(0), pending_key(u64::MAX), &mut |key, bytes| {
                    preemption_point();
                    let (start, length, released) = pending_record(key, bytes)?;
                    if released <= found_limit {
                        free.insert(start, length)?;
                    } else if released <= found.newest {
                        waiting.include(length, released);
                    }
                    Ok::<_, Error>(true)
                })?;
                promotion.found_limit = Some(found_limit);
                self.found = (waiting.newest != 0).then_some(waiting);
            }
        }
        Ok(promotion)
    }
    /// Detach one generation's runs while the transaction writes to them.
    pub fn take_generation(&mut self, generation: u64) -> Released {
        self.released.remove(&generation).unwrap_or_default()
    }
    pub fn put_generation(&mut self, generation: u64, released: Released) {
        if !released.runs.is_empty() {
            self.released.insert(generation, released);
        }
    }
}

/// SPACE edits of one transaction's promotion: known runs to move from
/// PENDING to FREE, and the bound up to which found records are promoted.
#[derive(Default)]
pub struct Promotion {
    released: BTreeMap<u64, Released>,
    found_limit: Option<u64>,
}
impl Promotion {
    pub fn is_empty(&self) -> bool {
        self.released.is_empty() && self.found_limit.is_none()
    }
    /// Apply to the transaction's space tree. Found records are visited in
    /// batches of PROMOTE_BATCH, so the heap holds a bounded number of them.
    pub fn apply<S: NodeStore>(self, store: &mut S, root: &mut BlockPtr) -> Result<(), Error> {
        for (start, length) in self.released.into_values().flat_map(|released| released.runs) {
            preemption_point();
            let old = btree::delete(store, root, pending_key(start))?.ok_or(Error::Invalid)?;
            if pending_record(pending_key(start), &old)?.1 != length {
                return Err(Error::Invalid);
            }
            add_free(store, root, start, length)?;
        }
        let Some(limit) = self.found_limit else {
            return Ok(());
        };
        let mut cursor = 0;
        loop {
            let mut batch = Vec::new();
            btree::visit_range(store, *root, pending_key(cursor), pending_key(u64::MAX), &mut |key, bytes| {
                preemption_point();
                let (start, length, released) = pending_record(key, bytes)?;
                if released <= limit {
                    batch.push((start, length));
                }
                Ok::<_, Error>(batch.len() < PROMOTE_BATCH)
            })?;
            let Some(&(last, _)) = batch.last() else {
                return Ok(());
            };
            for (start, length) in batch {
                preemption_point();
                btree::delete(store, root, pending_key(start))?;
                add_free(store, root, start, length)?;
            }
            cursor = last.checked_add(1).ok_or(Error::Invalid)?;
        }
    }
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
/// caller's transaction makes both changes atomic and quarantines the node's
/// four blocks; reuse still waits for G+2.
pub fn reap<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    addr: u64,
) -> Result<(), Error> {
    let dead = key(addr, format::DEAD, 4);
    if btree::search(store, *root, dead)?.as_deref() != Some(&[]) {
        return Err(Error::Invalid);
    }
    btree::delete(store, root, dead)?;
    Ok(())
}
