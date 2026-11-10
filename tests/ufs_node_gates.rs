extern crate alloc;
#[path = "../kernel/src/fs/ufs/node_gates.rs"]
mod node_gates;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};

#[test]
fn independent_nodes_progress_and_same_node_serializes() {
    let mut gates = node_gates::NodeGates::new();
    let make = || Ok::<_, ()>(Arc::new(AtomicBool::new(false)));
    let writer = gates.get_or_insert(2, make, ()).unwrap();
    writer.store(true, Ordering::Release);
    let other = gates.get_or_insert(3, make, ()).unwrap();
    assert!(!other.swap(true, Ordering::AcqRel));
    let append = gates.get_or_insert(2, make, ()).unwrap();
    assert!(Arc::ptr_eq(&writer, &append));
    assert!(append.load(Ordering::Acquire));
    drop(writer);
    drop(append);
    assert!(!gates.get_or_insert(2, make, ()).unwrap().load(Ordering::Acquire));
}
