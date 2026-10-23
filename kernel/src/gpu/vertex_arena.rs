//! Nonoverlapping vertex ranges retained until actual completion and fence publication.
//! Four admissions are possible per context; no range is reused while one still owns it.
//! An arena holds [`limits::IN_FLIGHT`] times the context's largest recent submission,
//! page-rounded, so the four frames libgfx keeps in flight fit (ruling B32), and the arena
//! shrinks after a run of small frames.

use gpu_abi::limits;

pub(super) const MAX_BYTES: u32 = 128 << 20;
const MIN_BYTES: u32 = 64 << 10;
/// Consecutive submissions using under a quarter of an arena after which it shrinks.
const SHRINK_AFTER: u32 = 16;

#[derive(Clone, Copy)]
struct Range { serial: u64, start: u32, end: u32 }

pub(super) struct Ranges {
    pub(super) capacity: u32,
    held: [Option<Range>; limits::IN_FLIGHT],
    /// The largest submission since this arena was sized.
    largest: u32,
    /// Consecutive submissions under a quarter of `capacity`, and the largest of them.
    small_run: u32,
    small_largest: u32,
}

/// The capacity of an arena for `largest`: [`limits::IN_FLIGHT`] times it, page-rounded,
/// within 64 KiB..=[`MAX_BYTES`].
pub(super) fn capacity_for(largest: u32) -> u32 {
    largest.saturating_mul(limits::IN_FLIGHT as u32).min(MAX_BYTES).max(MIN_BYTES).next_multiple_of(4096)
}

/// Page-rounded storage for one submission of `bytes`, held by that submission alone.
pub(super) fn exact(bytes: u64) -> Option<u32> {
    let bytes = u32::try_from(bytes).ok()?;
    if bytes == 0 || bytes > MAX_BYTES { return None; }
    Some(bytes.next_multiple_of(4096))
}

impl Ranges {
    /// An arena sized for `largest`, the largest recent submission.
    pub(super) fn new(largest: u64) -> Option<Self> {
        let largest = u32::try_from(largest).ok()?;
        if largest == 0 || largest > MAX_BYTES { return None; }
        Some(Self { capacity: capacity_for(largest), held: [None; limits::IN_FLIGHT], largest, small_run: 0, small_largest: 0 })
    }

    /// Record a submission of `bytes`; return the largest recent submission the arena
    /// should now be sized for: that of the run of small submissions once the run is long
    /// enough to shrink it, else the largest since it was sized.
    pub(super) fn note(&mut self, bytes: u32) -> u32 {
        self.largest = self.largest.max(bytes);
        if bytes < self.capacity / 4 {
            self.small_run += 1;
            self.small_largest = self.small_largest.max(bytes);
        } else {
            self.small_run = 0;
            self.small_largest = 0;
        }
        if self.small_run >= SHRINK_AFTER { self.small_largest } else { self.largest }
    }

    pub(super) fn reserve(&mut self, serial: u64, bytes: u32) -> Option<u32> {
        let slot = self.held.iter().position(Option::is_none)?;
        let mut start = 0u32;
        loop {
            let end = start.checked_add(bytes)?;
            if end > self.capacity { return None; }
            if let Some(overlap) = self.held.iter().flatten().find(|r| start < r.end && r.start < end) {
                start = overlap.end;
            } else {
                self.held[slot] = Some(Range { serial, start, end });
                return Some(start);
            }
        }
    }

    pub(super) fn release(&mut self, serial: u64) {
        for slot in &mut self.held {
            if slot.is_some_and(|r| r.serial == serial) { *slot = None; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn four_pending_ranges_stay_distinct_and_only_completed_space_is_reused() {
        let mut a = Ranges::new(48).unwrap();
        for serial in 1..=4 { assert_eq!(a.reserve(serial, 48), Some((serial as u32 - 1) * 48)); }
        assert_eq!(a.reserve(5, 48), None);
        a.release(2);
        assert_eq!(a.reserve(5, 48), Some(48));
        a.release(1);
        assert_eq!(a.reserve(6, 96), Some(192));
    }
    #[test]
    fn capacity_and_offsets_never_wrap_or_exceed_the_buffer_limit() {
        assert!(Ranges::new(0).is_none());
        assert!(Ranges::new(u64::MAX).is_none());
        assert!(Ranges::new(MAX_BYTES as u64 + 1).is_none());
        let mut a = Ranges::new(MAX_BYTES as u64).unwrap();
        assert_eq!(a.capacity, MAX_BYTES);
        assert_eq!(a.reserve(1, MAX_BYTES), Some(0));
        assert_eq!(a.reserve(2, 48), None);
        a.release(1);
        assert_eq!(a.reserve(3, 48), Some(0));
        assert!(exact(0).is_none());
        assert!(exact(MAX_BYTES as u64 + 1).is_none());
        assert_eq!(exact(MAX_BYTES as u64), Some(MAX_BYTES));
        assert_eq!(exact(1), Some(4096));
    }
    #[test]
    fn a_large_frame_grows_the_arena_to_four_frames_page_rounded() {
        let requested = 17_616_077; // 16.8 MiB, formerly a 128 MiB arena.
        let mut a = Ranges::new(requested).unwrap();
        assert_eq!(a.capacity, (4 * requested as u32).next_multiple_of(4096));
        for serial in 0..4 { assert!(a.reserve(serial, requested as u32).is_some(), "frame {serial} of four in flight"); }
        // The arena, the frame's staging and a 32 MiB window leave room for commands.
        assert!(u64::from(a.capacity) + requested + (32 << 20) < (192 << 20));
        assert_eq!(Ranges::new(1).unwrap().capacity, MIN_BYTES);
    }
    #[test]
    fn a_run_of_small_submissions_shrinks_the_arena() {
        let mut a = Ranges::new(1 << 20).unwrap();
        for _ in 1..SHRINK_AFTER { assert_eq!(a.note(1000), 1 << 20); }
        assert_eq!(a.note(1000), 1000);
        assert_eq!(capacity_for(1000), MIN_BYTES);
        let mut b = Ranges::new(1 << 20).unwrap();
        for _ in 1..SHRINK_AFTER { b.note(1000); }
        b.note(b.capacity / 4); // a quarter or more ends the run
        assert_eq!(b.note(1000), 1 << 20);
        let mut floor = Ranges::new(1).unwrap();
        for _ in 0..SHRINK_AFTER { floor.note(1); }
        assert_eq!(capacity_for(floor.note(1)), floor.capacity, "a minimum arena never shrinks");
    }
}
