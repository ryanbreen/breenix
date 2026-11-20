//! Fixed single-writer atomic CPU samples.

use core::sync::atomic::{fence, AtomicU64, Ordering};

/// Writers are serialized by the record's owner. A capture makes one read
/// attempt and omits an in-flight row, including if it interrupted the writer.
pub(super) struct Sample<const N: usize> {
    version: AtomicU64,
    pub(super) words: [AtomicU64; N],
}

impl<const N: usize> Sample<N> {
    pub(super) const fn new() -> Self {
        Self {
            version: AtomicU64::new(0),
            words: [const { AtomicU64::new(0) }; N],
        }
    }

    pub(super) fn store(&self, words: [u64; N]) {
        let version = self.version.load(Ordering::Relaxed);
        self.version
            .store(version.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (word, value) in self.words.iter().zip(words) {
            word.store(value, Ordering::Relaxed);
        }
        self.version
            .store(version.wrapping_add(2), Ordering::Release);
    }

    pub(super) fn load(&self) -> Option<[u64; N]> {
        let before = self.version.load(Ordering::Acquire);
        if before & 1 != 0 {
            return None;
        }
        let words = self
            .words
            .each_ref()
            .map(|word| word.load(Ordering::Relaxed));
        // Seeing a word written after the writer's release fence also orders
        // the odd version before this final read, so a mixed row is rejected.
        fence(Ordering::Acquire);
        (before == self.version.load(Ordering::Relaxed)).then_some(words)
    }
}
