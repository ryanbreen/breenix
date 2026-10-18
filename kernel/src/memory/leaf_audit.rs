//! A depth-first cursor over a dead address space. Tables stay allocated until
//! the audit finishes, so saved physical addresses remain valid between passes.

pub(super) struct LeafAudit {
    frames: [u64; 4],
    next: [usize; 4],
    prefix: [u64; 4],
    depth: usize,
    root_entries: usize,
    done: bool,
}

impl LeafAudit {
    pub(super) const fn new(root_entries: usize) -> Self {
        Self {
            frames: [0; 4],
            next: [0; 4],
            prefix: [0; 4],
            depth: 0,
            root_entries,
            done: false,
        }
    }

    /// Advance before returning an entry: even a yield at a table boundary
    /// cannot inspect that entry twice. Ascending reads no descriptors.
    pub(super) fn next_entry(&mut self, root: u64) -> Option<(u64, usize, u64, u32)> {
        self.frames[0] = root;
        while !self.done {
            let limit = if self.depth == 0 {
                self.root_entries
            } else {
                512
            };
            if self.next[self.depth] == limit {
                if self.depth == 0 {
                    self.done = true;
                } else {
                    self.depth -= 1;
                }
                continue;
            }
            let index = self.next[self.depth];
            self.next[self.depth] += 1;
            let shift = 39 - self.depth as u32 * 9;
            let address = self.prefix[self.depth] | ((index as u64) << shift);
            return Some((self.frames[self.depth], index, address, shift));
        }
        None
    }

    pub(super) fn finished(&mut self) -> bool {
        while !self.done {
            let limit = if self.depth == 0 {
                self.root_entries
            } else {
                512
            };
            if self.next[self.depth] != limit {
                break;
            }
            if self.depth == 0 {
                self.done = true;
            } else {
                self.depth -= 1;
            }
        }
        self.done
    }

    /// Called only for an intermediate table, immediately after next_entry.
    pub(super) fn descend(&mut self, frame: u64, address: u64) {
        self.depth += 1;
        self.frames[self.depth] = frame;
        self.next[self.depth] = 0;
        self.prefix[self.depth] = address;
    }
}

#[cfg(test)]
mod tests {
    use super::LeafAudit;

    #[test]
    fn resumes_at_every_entry_without_revisiting_ancestors() {
        for root_entries in [256, 512] {
            let mut cursor = LeafAudit::new(root_entries);
            let mut seen = std::collections::BTreeSet::new();
            let mut leaves = Vec::new();
            // One call is one unit of a one-entry pass. Descend at the last
            // root slot and both ends of each intermediate table.
            while !cursor.finished() {
                let (frame, index, address, shift) = cursor.next_entry(1).expect("unfinished audit");
                assert!(seen.insert((frame, index)), "entry audited twice");
                if (shift == 39 && index == root_entries - 1)
                    || (shift > 12 && shift < 39 && (index == 0 || index == 511))
                {
                    cursor.descend(address + shift as u64 + 2, address);
                } else if shift == 12 {
                    leaves.push(address);
                }
            }
            assert_eq!(seen.len(), root_entries + 7 * 512);
            assert_eq!(leaves.len(), 4 * 512);
            assert!(leaves.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(cursor.finished());
            assert!(cursor.next_entry(1).is_none());
        }
    }
}
