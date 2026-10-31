//! CPU staging bytes in fallible, owned system frames rather than the kernel heap.
use core::ops::{Deref, DerefMut};
use super::dma::CpuBox;

pub struct Bytes {
    storage: CpuBox,
    start: usize,
    len: usize,
}
impl Bytes {
    pub(crate) fn new(len: usize) -> Result<Self, ()> {
        Ok(Self { storage: CpuBox::try_new(len).ok_or(())?, start: 0, len })
    }

    /// A failed growth keeps the old bytes and ownership intact.
    pub(crate) fn resize(&mut self, len: usize) -> Result<(), ()> {
        if len > self.storage.len() - self.start {
            let capacity = len.max(self.len.checked_mul(2).ok_or(())?);
            let mut replacement = CpuBox::try_new(capacity).ok_or(())?;
            replacement.as_mut()[..self.len].copy_from_slice(self);
            self.storage = replacement;
            self.start = 0;
        }
        if len > self.len {
            self.storage.as_mut()[self.start + self.len..self.start + len].fill(0);
        }
        self.len = len;
        Ok(())
    }

    /// Omit a prefix without allocating another staging buffer.
    pub(crate) fn remove_prefix(&mut self, count: usize) {
        assert!(count <= self.len);
        self.start += count;
        self.len -= count;
    }
}
impl Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] { &self.storage.as_ref()[self.start..self.start + self.len] }
}
impl DerefMut for Bytes {
    fn deref_mut(&mut self) -> &mut [u8] { &mut self.storage.as_mut()[self.start..self.start + self.len] }
}
