//! One reader per MAP_TREE, shared by its VMA pieces until the last disappears.
use alloc::sync::Arc;
use core::sync::atomic::Ordering;
use super::mem::Mem;

#[derive(Debug)]
pub struct Reader { status: Arc<Mem> }
impl Reader {
    pub fn attach(status: Arc<Mem>) -> Self {
        status.reader_state().readers.fetch_add(1, Ordering::AcqRel);
        Self { status }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.status.reader_state().readers.fetch_sub(1, Ordering::AcqRel);
    }
}
