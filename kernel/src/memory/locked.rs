//! Page-aligned memory-lock reservations, owned by an address space under PM.
use crate::syscall::errno::ENOMEM;
use alloc::vec::Vec;

#[derive(Default)]
pub struct MemoryLocks {
    // Sorted disjoint intervals; overlapping locks are counted only once.
    pub(crate) ranges: Vec<(u64, u64)>,
    pub future: bool,
    pub onfault: bool,
}

impl MemoryLocks {
    pub fn bytes(&self) -> u64 {
        self.ranges.iter().map(|(start, end)| end - start).sum()
    }

    pub fn overlaps(&self, start: u64, end: u64) -> bool {
        self.ranges.iter().any(|&(a, b)| a < end && start < b)
    }

    pub fn additional(&self, start: u64, end: u64) -> u64 {
        end - start
            - self
                .ranges
                .iter()
                .map(|&(a, b)| end.min(b).saturating_sub(start.max(a)))
                .sum::<u64>()
    }

    pub fn insert(&mut self, mut start: u64, mut end: u64) -> Result<(), u64> {
        if start == end {
            return Ok(());
        }
        self.ranges.try_reserve(1).map_err(|_| ENOMEM as u64)?;
        let mut first = 0;
        while first < self.ranges.len() && self.ranges[first].1 < start {
            first += 1;
        }
        let mut last = first;
        while last < self.ranges.len() && self.ranges[last].0 <= end {
            start = start.min(self.ranges[last].0);
            end = end.max(self.ranges[last].1);
            last += 1;
        }
        self.ranges.drain(first..last);
        self.ranges.insert(first, (start, end));
        Ok(())
    }

    // Reserve before any unmap: deleting a middle range can split one interval.
    pub fn reserve_split(&mut self) -> Result<(), u64> {
        self.ranges.try_reserve(1).map_err(|_| ENOMEM as u64)
    }

    pub fn remove(&mut self, start: u64, end: u64) {
        if start >= end {
            return;
        }
        let mut i = 0;
        while i < self.ranges.len() {
            let (a, b) = self.ranges[i];
            if b <= start || a >= end {
                i += 1;
                continue;
            }
            if a < start && end < b {
                self.ranges[i] = (a, start);
                self.ranges.insert(i + 1, (end, b));
                break;
            } else if a < start {
                self.ranges[i].1 = start;
                i += 1;
            } else if end < b {
                self.ranges[i].0 = end;
                break;
            } else {
                self.ranges.remove(i);
            }
        }
    }

    pub fn clear(&mut self) {
        self.ranges.clear();
        self.future = false;
        self.onfault = false;
    }
}
