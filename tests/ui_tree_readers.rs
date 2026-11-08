//! Execute the production VMA lease and BreenGL commit gate off hardware.
extern crate alloc;
use std::sync::{Arc, atomic::Ordering};
#[path = "../libs/breengel/src/tree.rs"]
mod tree;
mod mem {
    #[derive(Debug, Default)]
    pub struct Mem(pub breengel_abi::TreeReaderState);
    impl Mem {
        pub fn reader_state(&self) -> &breengel_abi::TreeReaderState { &self.0 }
    }
}
#[path = "../kernel/src/window/readers.rs"]
mod readers;

#[test]
fn gate_counts_only_attached_commits_and_mapping_lifetimes() {
    let status = Arc::new(mem::Mem::default());
    let mut serializations = 0;
    let mut writes = 0;
    let mut commit = || {
        tree::serialize_for_commit(status.reader_state().readers.load(Ordering::Acquire),
            &mut serializations, || { writes += 1; Ok(()) }).unwrap();
        (serializations, writes)
    };
    const N: u64 = 19;
    for _ in 0..N { assert_eq!(commit(), (0, 0)); }
    let reader_a = Arc::new(readers::Reader::attach(status.clone()));
    let reader_b = Arc::new(readers::Reader::attach(status.clone()));
    assert_eq!(status.reader_state().readers.load(Ordering::Acquire), 2);
    for i in 1..=N { assert_eq!(commit(), (i, i)); }
    // A partial unmap leaves a piece of the same MAP_TREE mapping alive.
    let split = reader_a.clone();
    drop(reader_a);
    assert_eq!(status.reader_state().readers.load(Ordering::Acquire), 2);
    drop(split); // final piece unmapped
    assert_eq!(status.reader_state().readers.load(Ordering::Acquire), 1);
    // Process exit destroys its remaining VMA tokens, without a special syscall.
    let process_vmas = vec![reader_b];
    drop(process_vmas);
    assert_eq!(status.reader_state().readers.load(Ordering::Acquire), 0);
    for _ in 0..N { assert_eq!(commit(), (N, N)); }
}

#[test]
fn failed_serialization_is_not_counted() {
    let mut count = 0;
    assert!(tree::serialize_for_commit(1, &mut count, || Err("page too small".into())).is_err());
    assert_eq!(count, 0);
}

#[test]
fn codec_still_serializes_the_gated_tree() {
    let value = br#"{"key":"root","parent":null,"role":"window","name":"gate","value":7,"bounds":[0,0,1,1],"clip":[0,0,1,1],"flags":[]}\0"#;
    let mut bytes = value.to_vec(); bytes.truncate(bytes.len()-2); bytes.push(0);
    let node = tree::Tree::parse(&bytes).unwrap();
    let mut page = [0; 4096];
    let mut count = 0;
    tree::serialize_for_commit(1, &mut count, || node.write(&mut page).map(|_| ())).unwrap();
    assert_eq!(count, 1);
    assert_eq!(tree::Tree::parse(&page).unwrap(), node);
}
