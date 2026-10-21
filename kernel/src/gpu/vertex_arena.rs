//! Nonoverlapping vertex ranges retained until actual completion and fence publication.
//! Four admissions are possible per context; no range is reused while one still owns it.

pub(super) const MAX_BYTES: u32 = 128 << 20;
const MIN_BYTES: u32 = 64 << 10;

#[derive(Clone, Copy)]
struct Range { serial: u64, start: u32, end: u32 }

pub(super) struct Ranges {
    pub(super) capacity: u32,
    held: [Option<Range>; gpu_abi::limits::IN_FLIGHT],
}

impl Ranges {
    pub(super) fn new(bytes: u64) -> Option<Self> {
        let bytes = u32::try_from(bytes).ok()?;
        if bytes == 0 || bytes > MAX_BYTES { return None; }
        // Reserve the requested storage, rather than speculatively multiplying it by
        // the admission count: staging and window backing share the same quota.
        let capacity = bytes.max(MIN_BYTES).next_multiple_of(4096);
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
        let mut a = Ranges::new(requested).unwrap();
        assert!(u64::from(a.capacity) >= requested);
        assert!(u64::from(a.capacity) < requested + 4096);
        assert_eq!(a.reserve(1, requested as u32), Some(0));
        // Arena, vertex staging and a 32 MiB window leave room for commands.
        assert!(u64::from(a.capacity) + requested + (32 << 20) < (192 << 20));
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
    }
}
