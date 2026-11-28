//! Fallibly grown hash buckets with dense storage for TCP's table sweeps.

use alloc::vec::Vec;

use crate::random::siphash;
use crate::syscall::errno::ENOMEM;

const NONE: usize = usize::MAX;
const INITIAL_BUCKETS: usize = 8;

struct Entry<T> {
    hash: u64,
    prev: usize,
    next: usize,
    value: T,
}

/// Values live in a dense vector. Chains use indices so growth can rebuild the
/// heads without moving values, and removal can repair the slot moved into the
/// hole without scanning either chain. Callers compare the full key and hold
/// the TCP lock throughout reservation, insertion, lookup and removal.
pub(super) struct Buckets<T> {
    key: [u8; 16],
    heads: Vec<usize>,
    entries: Vec<Entry<T>>,
}

impl<T> Buckets<T> {
    pub const fn new() -> Self {
        Self {
            key: [0; 16],
            heads: Vec::new(),
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn get(&self, index: usize) -> &T {
        &self.entries[index].value
    }

    pub fn get_mut(&mut self, index: usize) -> &mut T {
        &mut self.entries[index].value
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.entries.iter().map(|entry| &entry.value)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.entries.iter_mut().map(|entry| &mut entry.value)
    }

    fn bucket(&self, hash: u64) -> usize {
        hash as usize & (self.heads.len() - 1)
    }

    pub fn candidates(&self, key: &[u8]) -> impl Iterator<Item = (usize, &T)> {
        let mut next = if self.entries.is_empty() {
            NONE
        } else {
            self.heads[self.bucket(siphash::hash(&self.key, key))]
        };
        core::iter::from_fn(move || {
            let index = next;
            let entry = self.entries.get(index)?;
            next = entry.next;
            Some((index, &entry.value))
        })
    }

    pub fn position(&self, key: &[u8], mut matches: impl FnMut(&T) -> bool) -> Option<usize> {
        self.candidates(key)
            .find_map(|(index, value)| matches(value).then_some(index))
    }

    /// Secure room without changing any membership. A resize just relinks
    /// the entries already published; after this succeeds insertion cannot allocate.
    pub fn try_reserve_one(&mut self) -> Result<(), i32> {
        self.entries.try_reserve(1).map_err(|_| ENOMEM)?;
        let old = self.heads.len();
        if self.entries.len() < old {
            return Ok(());
        }
        let new = old.checked_mul(2).ok_or(ENOMEM)?.max(INITIAL_BUCKETS);
        self.heads
            .try_reserve_exact(new - old)
            .map_err(|_| ENOMEM)?;
        // Production seeds each index after the boot pool is initialized. The
        // host fixture uses the initial zero key to choose exact collisions.
        #[cfg(not(test))]
        if old == 0 {
            crate::random::fill(&mut self.key);
        }
        self.heads.resize(new, NONE);
        self.heads.fill(NONE);
        for index in 0..self.entries.len() {
            self.link(index);
        }
        Ok(())
    }

    pub fn push_reserved(&mut self, key: &[u8], value: T) {
        let index = self.entries.len();
        debug_assert!(index < self.entries.capacity() && index < self.heads.len());
        self.entries.push(Entry {
            hash: siphash::hash(&self.key, key),
            prev: NONE,
            next: NONE,
            value,
        });
        self.link(index);
    }

    fn link(&mut self, index: usize) {
        let bucket = self.bucket(self.entries[index].hash);
        let next = core::mem::replace(&mut self.heads[bucket], index);
        self.entries[index].prev = NONE;
        self.entries[index].next = next;
        if next != NONE {
            self.entries[next].prev = index;
        }
    }

    fn unlink(&mut self, index: usize) {
        let entry = &self.entries[index];
        let (prev, next) = (entry.prev, entry.next);
        if prev == NONE {
            let bucket = self.bucket(entry.hash);
            self.heads[bucket] = next;
        } else {
            self.entries[prev].next = next;
        }
        if next != NONE {
            self.entries[next].prev = prev;
        }
    }

    pub fn remove(&mut self, index: usize) -> T {
        self.unlink(index);
        let last = self.entries.len() - 1;
        if index == last {
            return self.entries.pop().unwrap().value;
        }
        self.unlink(last);
        let removed = self.entries.swap_remove(index);
        self.link(index);
        removed.value
    }
}
