//! A PRESENT result belongs to its waiting caller, independently of queue depth.
use alloc::collections::BTreeMap;

pub struct Answers(BTreeMap<u32, Option<u32>>);
impl Answers {
    pub const fn new() -> Self { Self(BTreeMap::new()) }
    pub fn register(&mut self, fence: u32) { self.0.insert(fence, None); }
    pub fn complete(&mut self, fence: u32, answer: u32) {
        // Interrupted or faulted callers unregister, so late completion has no registered destination.
        if let Some(slot) = self.0.get_mut(&fence) { *slot = Some(answer); }
    }
    pub fn take(&mut self, fence: u32) -> Option<u32> { self.0.remove(&fence).flatten() }
}
