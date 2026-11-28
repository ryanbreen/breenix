//! Out-of-memory victim selection (E3 plan, section 13).
//!
//! A candidate is an address space, so CLONE_VM siblings are scored, and
//! killed, together. Its score is what killing it frees: its private resident
//! frames (anonymous pages and private copy-on-write copies) and its
//! page-table frames. Shared file pages are excluded, because reclaim can
//! already take them. The largest score is chosen, ties going to the higher
//! pid (victim selection by size: McKusick et al. 2014, ch. 6).
//!
//! The address space holding PID 1, kernel threads, and a victim already
//! chosen are excluded. While a chosen victim is still exiting within its
//! bound, no second victim is chosen; one that overran its bound is recorded
//! and selection repeats without it.
//!
//! The module has no kernel dependency.

/// A chosen victim's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chosen {
    /// Not chosen.
    No,
    /// Chosen and still exiting within its bound.
    Exiting,
    /// Chosen, and it did not exit within its bound.
    Overran,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The highest pid among the address space's processes; it breaks ties.
    pub pid: u32,
    pub private_frames: u64,
    pub table_frames: u64,
    /// The address space holds PID 1.
    pub holds_init: bool,
    pub kernel_thread: bool,
    pub chosen: Chosen,
}

impl Candidate {
    pub const fn score(&self) -> u64 {
        self.private_frames.saturating_add(self.table_frames)
    }

    const fn eligible(&self) -> bool {
        !self.holds_init && !self.kernel_thread && matches!(self.chosen, Chosen::No)
    }
}

/// The decision, by index into the candidate slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// No eligible candidate: the allocation fails.
    None,
    /// A victim is still exiting: wait for it, and choose no other.
    Wait { victim: usize },
    /// Kill `victim`. `next` holds the two runners-up for the OOM record.
    Kill { victim: usize, next: [Option<usize>; 2] },
}

/// Whether `a` ranks above `b`: a larger score, then a higher pid.
const fn above(a: &Candidate, b: &Candidate) -> bool {
    let (sa, sb) = (a.score(), b.score());
    sa > sb || (sa == sb && a.pid > b.pid)
}

pub fn select(candidates: &[Candidate]) -> Selection {
    if let Some(victim) = candidates.iter().position(|c| matches!(c.chosen, Chosen::Exiting)) {
        return Selection::Wait { victim };
    }
    let mut ranked: [Option<usize>; 3] = [None; 3];
    for (index, candidate) in candidates.iter().enumerate() {
        if !candidate.eligible() {
            continue;
        }
        let mut slot = ranked.len();
        for (place, entry) in ranked.iter().enumerate() {
            match entry {
                Some(other) if !above(candidate, &candidates[*other]) => {}
                _ => {
                    slot = place;
                    break;
                }
            }
        }
        if slot < ranked.len() {
            let mut carry = Some(index);
            for entry in ranked.iter_mut().skip(slot) {
                core::mem::swap(entry, &mut carry);
            }
        }
    }
    match ranked[0] {
        None => Selection::None,
        Some(victim) => Selection::Kill {
            victim,
            next: [ranked[1], ranked[2]],
        },
    }
}
