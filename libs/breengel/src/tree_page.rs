//! Live compositor page: an atomic sequence word followed by the tree codec's JSON.
//! This is separate from immutable, commit-coupled client tree pages.
use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use crate::tree::Tree;

pub struct Page<'a> {
    sequence: &'a AtomicU64,
    bytes: &'a [AtomicU8],
}
impl<'a> Page<'a> {
    /// Borrow a live compositor tree mapping.
    ///
    /// # Safety
    /// The mapping must remain readable for `'a`, be shared coherent memory, and
    /// use this protocol for concurrent access. The sequence occupies the first
    /// eight bytes, aligned to eight; its initial value is 0.
    pub unsafe fn from_mapping(addr: *mut u8, len: usize) -> Result<Self, String> {
        if addr.is_null() || addr as usize % 8 != 0 || len <= 8 || len > isize::MAX as usize {
            return Err("invalid compositor tree mapping".into());
        }
        Ok(Self { sequence: &*addr.cast::<AtomicU64>(),
            bytes: core::slice::from_raw_parts(addr.add(8).cast::<AtomicU8>(), len-8) })
    }

    /// Publish one document; errors leave the prior publication readable.
    ///
    /// # Safety
    /// The mapping must be writable and the caller must be its sole writer.
    pub unsafe fn publish(&self, tree: &Tree) -> Result<(), String> {
        let sequence = self.sequence.load(Ordering::SeqCst);
        if sequence & 1 != 0 { return Err("compositor tree writer already active".into()); }
        let next = sequence.checked_add(2).ok_or("compositor tree sequence exhausted")?;
        let mut bytes = vec![0; self.bytes.len()];
        let used = tree.write(&mut bytes)?;
        // SC byte accesses also make the copied payload race-free on ARM.
        self.sequence.store(next-1, Ordering::SeqCst);
        for (dst, src) in self.bytes.iter().zip(&bytes[..used]) { dst.store(*src, Ordering::SeqCst); }
        self.sequence.store(next, Ordering::SeqCst);
        Ok(())
    }

    /// Attempt a bounded copy. An absent result means unpublished data or a concurrent writer;
    /// the caller supplies its own retry deadline.
    pub fn snapshot(&self) -> Result<Option<Tree>, String> {
        let before = self.sequence.load(Ordering::SeqCst);
        if before == 0 || before & 1 != 0 { return Ok(None); }
        let mut bytes = Vec::new();
        for src in self.bytes {
            let value = src.load(Ordering::SeqCst);
            bytes.push(value);
            if value == 0 { break; }
        }
        if self.sequence.load(Ordering::SeqCst) != before { return Ok(None); }
        Tree::parse(&bytes).map(Some)
    }
}
