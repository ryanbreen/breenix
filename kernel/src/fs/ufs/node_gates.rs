//! Share mutation serialization by daemon node, without retaining closed nodes.
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;

pub struct NodeGates<T>(Vec<(u64, Weak<T>)>);
impl<T> NodeGates<T> {
    pub const fn new() -> Self { Self(Vec::new()) }

    pub fn get_or_insert<E>(&mut self, node: u64, create: impl FnOnce() -> Result<Arc<T>, E>, no_memory: E) -> Result<Arc<T>, E> {
        self.0.retain(|(_, gate)| gate.strong_count() != 0);
        if let Some(gate) = self.0.iter().find_map(|(id, gate)| (*id == node).then(|| gate.upgrade()).flatten()) {
            return Ok(gate);
        }
        self.0.try_reserve(1).map_err(|_| no_memory)?;
        let gate = create()?;
        self.0.push((node, Arc::downgrade(&gate)));
        Ok(gate)
    }
}
