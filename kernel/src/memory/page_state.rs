//! Page-state transitions of the page cache (E3 plan, sections 5.1 and 5.2).
//!
//! Callers change a cache page's state through [`transition`]. A refused
//! transition is a kernel bug, reported with the state and the event.
//!
//! Absent does not mean zero-filled: only a `Hole` entry, which a fill or the pre-E2.9
//! EOF extension creates, and the part of the last page past EOF read as zeros.
//!
//! Generation number 0 is not a BreenFS generation: `Dirty { gen: 0 }` is a
//! page stored through a shared mapping and not yet admitted to a generation
//! (`MAPPED_DIRTY`, section 11.2a).
//!
//! The module has no kernel dependency.

/// A BreenFS generation. 0 means "not admitted".
pub type Gen = u64;

/// The generation number of a page that a mapping dirtied before admission.
pub const NOT_ADMITTED: Gen = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageState {
    /// No entry: the bytes are unknown and must be read.
    Absent,
    /// A frame whose fill is in flight; its bytes are not file bytes yet.
    Filling,
    /// The frame holds the file's bytes.
    Uptodate,
    /// Newer than the disk. `gen` names the generation that owns the bytes, or
    /// is `NOT_ADMITTED` for a mapped store not yet admitted.
    Dirty { gen: Gen },
    /// Generation `gen`'s stable copy, being written.
    Writeback { gen: Gen },
    /// No extent covers the page and it is below EOF.
    Hole,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A reader, fault or readahead claims an absent page for a fill.
    ClaimFill,
    /// The fill completed and its CRCs verified.
    FillData,
    /// The fill reported that no extent covers the page.
    FillHole,
    /// The fill failed; the errno lives on the frame its waiters hold.
    FillFailed,
    /// The claim was retired: a dropped readahead window or a stale claim.
    Cancel,
    /// A `write()` part, or a shrink's tail zeroing, in the open generation.
    Write { open: Gen },
    /// A store fault through a shared writable mapping.
    SharedStore,
    /// Mapped admission into the open generation.
    MappedAdmission { open: Gen },
    /// The freeze of generation `gen`.
    Freeze { gen: Gen },
    /// Generation `gen` is durable and the open view has adopted it.
    Adopted { gen: Gen },
    /// Reclaim of a clean, unmapped page or a hole.
    Reclaim,
    /// Truncation past the page.
    Truncate,
    /// EOF grew over a bound absent page (before E2.9).
    EofGrow,
}

/// What a transition does to the index's tree entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryChange {
    /// The entry keeps its frame (or stays absent).
    Keep,
    /// An entry is inserted: a claimed frame, a prepared frame or a hole.
    Insert,
    /// A new frame or value replaces the entry. When the old entry was a
    /// frame, or a hole mapped through the shared zeroed frame, the caller
    /// revokes its PTEs before returning (section 5.4).
    Replace,
    /// The entry leaves the tree.
    Remove,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub to: PageState,
    pub entry: EntryChange,
}

/// A transition the table does not allow, with both values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused {
    pub from: PageState,
    pub event: Event,
}

const fn to(to: PageState, entry: EntryChange) -> Result<Transition, Refused> {
    Ok(Transition { to, entry })
}

/// The transition `event` makes from `from`.
pub const fn transition(from: PageState, event: Event) -> Result<Transition, Refused> {
    use EntryChange::{Insert, Keep, Remove, Replace};
    use PageState::{Absent, Dirty, Filling, Hole, Uptodate, Writeback};
    let refused = Err(Refused { from, event });
    match (from, event) {
        (Absent, Event::ClaimFill) => to(Filling, Insert),
        (Filling, Event::FillData) => to(Uptodate, Keep),
        (Filling, Event::FillHole) => to(Hole, Replace),
        (Filling, Event::FillFailed) | (Filling, Event::Cancel) => to(Absent, Remove),

        (_, Event::Write { open: NOT_ADMITTED }) => refused,
        // A partial write of an absent page fills a prepared frame first; a
        // write that reaches a fill in flight waits for it (sections 6.1a, 7.2).
        (Absent, Event::Write { open }) => to(Dirty { gen: open }, Insert),
        (Filling, Event::Write { .. }) => refused,
        (Uptodate, Event::Write { open }) => to(Dirty { gen: open }, Keep),
        // A page dirty in an older generation than the open one was missed by
        // that generation's freeze.
        (Dirty { gen }, Event::Write { open }) if gen == open || gen == NOT_ADMITTED => {
            to(Dirty { gen: open }, Keep)
        }
        (Dirty { .. }, Event::Write { .. }) => refused,
        // The old frame stays the frozen generation's stable copy (11.2); the
        // write belongs to a later, open generation.
        (Writeback { gen }, Event::Write { open }) if open > gen => to(Dirty { gen: open }, Replace),
        (Writeback { .. }, Event::Write { .. }) => refused,
        (Hole, Event::Write { open }) => to(Dirty { gen: open }, Replace),

        // A store needs a resident page: before E2.9 a fault cannot fill, and
        // from E3-11 it fills first, then stores.
        (Absent, Event::SharedStore) | (Filling, Event::SharedStore) => refused,
        (Uptodate, Event::SharedStore) => to(Dirty { gen: NOT_ADMITTED }, Keep),
        (Dirty { gen }, Event::SharedStore) => to(Dirty { gen }, Keep),
        (Writeback { .. }, Event::SharedStore) | (Hole, Event::SharedStore) => {
            to(Dirty { gen: NOT_ADMITTED }, Replace)
        }

        (_, Event::MappedAdmission { open: NOT_ADMITTED }) => refused,
        (Dirty { gen: NOT_ADMITTED }, Event::MappedAdmission { open }) => to(Dirty { gen: open }, Keep),
        (_, Event::MappedAdmission { .. }) => refused,

        // A page that is still MAPPED_DIRTY waits for the next admission, so
        // no page is dirty in a generation whose batch is chosen (11.2a).
        (_, Event::Freeze { gen: NOT_ADMITTED }) => refused,
        (Dirty { gen }, Event::Freeze { gen: frozen }) if gen == frozen => to(Writeback { gen }, Keep),
        (_, Event::Freeze { .. }) => refused,

        (_, Event::Adopted { gen: NOT_ADMITTED }) => refused,
        (Writeback { gen }, Event::Adopted { gen: adopted }) if gen == adopted => to(Uptodate, Keep),
        (_, Event::Adopted { .. }) => refused,

        (Uptodate, Event::Reclaim) | (Hole, Event::Reclaim) => to(Absent, Remove),
        (_, Event::Reclaim) => refused,

        // A Writeback frame stays referenced by its generation's batch, and a
        // filler's completion finds its claim gone and cancels (6.1a).
        (Absent, Event::Truncate) => to(Absent, Keep),
        (_, Event::Truncate) => to(Absent, Remove),

        (Absent, Event::EofGrow) => to(Hole, Insert),
        (_, Event::EofGrow) => refused,

        (_, Event::ClaimFill)
        | (_, Event::FillData)
        | (_, Event::FillHole)
        | (_, Event::FillFailed)
        | (_, Event::Cancel) => refused,
    }
}
