//! Space-tree item operations from cowfs design sections 2.4 and 2.5.
//!
//! The caller supplies a private transaction store/root. As with `btree`, a
//! fallible store must roll back a failed edit; this layer does not publish roots
//! or perform I/O. The transaction layer owns space-tree node allocation and the
//! deferred refcount fixpoint. Ranges supplied to `add_free` must be unowned and
//! outside reserved blocks. FS child references are managed by txn/the reaper.
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
                let at = 48 + i * 37;
                entries.push(InternalEntry::decode(&bytes[at..at + 37])?);
            }
            for (i, e) in entries.iter().enumerate().rev() {
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
    out.sort_by_key(|item| item.0);
    Ok(out)
}
fn free_ranges<S: NodeStore>(store: &S, root: BlockPtr) -> Result<Vec<(u64, u64)>, Error> {
    range(store, root, FREE_OBJECT, format::FREE)?
        .into_iter()
        .map(|(k, bytes)| {
            if bytes.len() != 8 {
                return Err(Error::Invalid);
            }
            let length = Free::decode(&bytes)?.length;
            end(k.offset, length)?;
            Ok((k.offset, length))
        })
        .collect()
}
fn merged(mut ranges: Vec<(u64, u64)>) -> Result<Vec<(u64, u64)>, Error> {
    ranges.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (start, length) in ranges {
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
    end(start, length)?;
    let old = free_ranges(store, *root)?;
    let mut ranges = old.clone();
    ranges.push((start, length));
    let ranges = merged(ranges)?;
    for &(start, length) in &old {
        if !ranges.contains(&(start, length)) {
            btree::delete(store, root, key(FREE_OBJECT, format::FREE, start))?;
        }
    }
    for (start, length) in ranges {
        if old.contains(&(start, length)) {
            continue;
        }
        btree::insert(
            store,
            root,
            key(FREE_OBJECT, format::FREE, start),
            &Free { length }.encode(),
        )?;
    }
    Ok(())
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
    let r = Ref {
        refcount: 1,
        kind: kind as u8,
        alloc_generation: generation,
        crcs: crcs.to_vec(),
    };
    if !valid_ref(length, &r) {
        return Err(Error::Invalid);
    }
    let mut free = free_ranges(store, *root)?;
    let mut candidate = free.iter().find(|(_, len)| *len >= length).copied();
    if candidate.is_none() {
        let mut pending = Vec::new();
        for (k, bytes) in range(store, *root, PENDING_OBJECT, format::PENDING)? {
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
        free.extend_from_slice(&pending);
        free = merged(free)?;
        candidate = free.iter().find(|(_, len)| *len >= length).copied();
        if candidate.is_none() {
            return Err(Error::ENOSPC);
        }
        for (start, length) in pending {
            btree::delete(store, root, key(PENDING_OBJECT, format::PENDING, start))?;
            add_free(store, root, start, length)?;
        }
    }
    let (start, available) = candidate.ok_or(Error::ENOSPC)?;
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
    Ok(start)
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
/// references until the reaper processes them; an empty node/data becomes PENDING.
pub fn release<S: NodeStore>(
    store: &mut S,
    root: &mut BlockPtr,
    start: u64,
    length: u64,
    generation: u64,
    referenced_children: bool,
) -> Result<(), Error> {
    let mut r = reference(store, *root, start, length)?.ok_or(Error::Invalid)?;
    if generation < r.alloc_generation || (referenced_children && r.kind != Kind::FsNode as u8) {
        return Err(Error::Invalid);
    }
    r.refcount -= 1;
    if r.refcount > 0 {
        btree::insert(store, root, key(start, format::REF, length), &r.encode())?;
    } else {
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
    }
    Ok(())
}
