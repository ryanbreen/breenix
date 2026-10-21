//! Nonoverlapping vertex ranges retained until actual completion and fence publication.
//! Four admissions are possible per context; no range is reused while one still owns it.
//! The reusable arena is small; a larger submission gets storage of its own (see [`exact`]).

/// The largest vertex storage one submission may name.
pub(super) const MAX_BYTES: u32 = 128 << 20;
/// The reusable arena's largest capacity, charged to the owner while the context keeps it.
pub(super) const ARENA_MAX: u32 = 4 << 20;
const MIN_BYTES: u32 = 64 << 10;

/// Page-rounded storage for one submission of `bytes` that the arena does not serve,
/// released when that submission retires, so a large frame strands no quota.
pub(super) fn exact(bytes: u64) -> Option<u32> {
    let bytes = u32::try_from(bytes).ok()?;
    if bytes == 0 || bytes > MAX_BYTES { return None; }
    Some(bytes.next_multiple_of(4096))
}

#[derive(Clone, Copy)]
struct Range { serial: u64, start: u32, end: u32 }

pub(super) struct Ranges {
    pub(super) capacity: u32,
    held: [Option<Range>; gpu_abi::limits::IN_FLIGHT],
}

impl Ranges {
    /// An arena for submissions of `bytes`: room for four in flight, a power of two
    /// between 64 KiB and [`ARENA_MAX`], or no arena when `bytes` exceeds that capacity.
    pub(super) fn new(bytes: u64) -> Option<Self> {
        let bytes = u32::try_from(bytes).ok()?;
        if bytes == 0 || bytes > ARENA_MAX { return None; }
        let capacity = bytes.saturating_mul(gpu_abi::limits::IN_FLIGHT as u32)
            .max(MIN_BYTES).next_power_of_two().min(ARENA_MAX);
        Some(Self { capacity, held: [None; gpu_abi::limits::IN_FLIGHT] })
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
    fn large_submission_keeps_room_for_staging_and_window_backing() {
        let requested = 17_616_077; // 16.8 MiB, formerly rounded to a 128 MiB arena.
        assert!(Ranges::new(requested).is_none(), "the reusable arena never takes a large frame");
        let own = exact(requested).unwrap();
        assert!(u64::from(own) >= requested);
        assert!(u64::from(own) < requested + 4096);
        // The largest arena, the frame's own storage, its staging and a 32 MiB window leave room for commands.
        assert!(u64::from(ARENA_MAX) + u64::from(own) + requested + (32 << 20) < (192 << 20));
    }
    #[test]
    fn small_frames_share_an_arena_of_at_most_four_mib() {
        assert_eq!(Ranges::new(1).unwrap().capacity, MIN_BYTES);
        assert_eq!(Ranges::new(300_000).unwrap().capacity, 2 << 20);
        let mut a = Ranges::new(1 << 20).unwrap();
        assert_eq!(a.capacity, ARENA_MAX);
        for serial in 1..=4 { assert_eq!(a.reserve(serial, 1 << 20), Some((serial as u32 - 1) << 20)); }
        let mut b = Ranges::new(ARENA_MAX as u64).unwrap();
        assert_eq!(b.capacity, ARENA_MAX);
        assert_eq!(b.reserve(1, ARENA_MAX), Some(0));
        assert_eq!(b.reserve(2, 48), None);
        b.release(1);
        assert_eq!(b.reserve(3, 48), Some(0));
    }
    #[test]
    fn capacity_and_offsets_never_wrap_or_exceed_the_buffer_limit() {
        assert!(Ranges::new(0).is_none());
        assert!(Ranges::new(u64::MAX).is_none());
        assert!(Ranges::new(ARENA_MAX as u64 + 1).is_none());
        assert!(exact(0).is_none());
        assert!(exact(u64::MAX).is_none());
        assert!(exact(MAX_BYTES as u64 + 1).is_none());
        assert_eq!(exact(MAX_BYTES as u64), Some(MAX_BYTES));
        assert_eq!(exact(1), Some(4096));
    }
}
