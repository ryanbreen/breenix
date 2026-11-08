//! Execute the production VMA lease and BreenGL commit gate off hardware.
extern crate alloc;
use std::sync::{Arc, atomic::Ordering};
#[path = "../libs/breengel/src/tree.rs"]
mod tree;
#[path = "../libs/breengel/src/tree_commit.rs"]
mod tree_commit;
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
        tree_commit::serialize_for_commit(status.reader_state().readers.load(Ordering::Acquire),
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
    assert!(tree_commit::serialize_for_commit(1, &mut count, || Err("page too small".into())).is_err());
    assert_eq!(count, 0);
}

#[test]
fn codec_still_serializes_the_gated_tree() {
    let value = br#"{"key":"root","parent":null,"role":"window","name":"gate","value":7,"bounds":[0,0,1,1],"clip":[0,0,1,1],"flags":[]}\0"#;
    let mut bytes = value.to_vec(); bytes.truncate(bytes.len()-2); bytes.push(0);
    let node = tree::Tree::parse(&bytes).unwrap();
    let mut page = [0; 4096];
    let mut count = 0;
    tree_commit::serialize_for_commit(1, &mut count, || node.write(&mut page).map(|_| ())).unwrap();
    assert_eq!(count, 1);
    assert_eq!(tree::Tree::parse(&page).unwrap(), node);
}

#[test]
fn production_mapping_transfers_and_splits_one_reader_lease() {
    use std::{fs, process::Command};
    let source = include_str!("../kernel/src/ipc/shmem/object.rs");
    let begin = source.find("/// Origin and the original maximum permission").unwrap();
    let mapping = &source[begin..];
    let reader_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("kernel/src/window/readers.rs");
    let dir = std::env::temp_dir().join(format!("tree-mapping-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let program = format!(r#"
extern crate alloc;
use std::sync::{{Arc, atomic::Ordering}};
pub mod window {{
    pub mod mem {{
        use std::sync::atomic::AtomicU64;
        #[derive(Debug, Default)] pub struct State {{ pub readers: AtomicU64 }}
        #[derive(Debug, Default)] pub struct Mem(pub State);
        impl Mem {{ pub fn reader_state(&self) -> &State {{ &self.0 }} }}
    }}
    #[path = {reader_path:?}] pub mod readers;
}}
pub mod mapping {{
    use std::sync::{{Arc, atomic::{{AtomicUsize, Ordering}}}};
    #[derive(Debug, Default)] pub struct ShmObject {{ pub maps: AtomicUsize }}
    impl ShmObject {{ pub fn record_activity(&self, _: bool, _: i32) {{}} }}
    fn current_pid() -> i32 {{ 7 }}
    {mapping}
}}
fn main() {{
    use mapping::*;
    let state = Arc::new(window::mem::Mem::default());
    let object = Arc::new(ShmObject::default());
    let mut vma = ShmMapping::with_origin(&object, MappingOrigin::Window, false);
    vma.tree_reader = Some(Arc::new(window::readers::Reader::attach(state.clone())));
    let piece = vma.clone();
    let replacement = vma.replacement();
    drop(vma);
    assert_eq!(state.0.readers.load(Ordering::Acquire), 1);
    drop(piece);
    assert_eq!(state.0.readers.load(Ordering::Acquire), 1);
    // retain_file_vmas clears shm before exit becomes waitable.
    drop(replacement);
    assert_eq!(state.0.readers.load(Ordering::Acquire), 0);
    assert_eq!(object.maps.load(Ordering::Acquire), 0);
}}
"#);
    fs::write(dir.join("mapping.rs"), program).unwrap();
    let compile = Command::new("rustc").args(["--edition=2021", "-Dwarnings"])
        .arg(dir.join("mapping.rs")).arg("-o").arg(dir.join("mapping")).output().unwrap();
    assert!(compile.status.success(), "{}", String::from_utf8_lossy(&compile.stderr));
    let run = Command::new(dir.join("mapping")).output().unwrap();
    fs::remove_dir_all(dir).unwrap();
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let retirement = include_str!("../kernel/src/memory/process_memory.rs");
    let detach = retirement.split("pub(crate) fn retain_file_vmas").nth(1).unwrap().split("pub(crate) fn release_exec_file_vmas").next().unwrap();
    assert!(detach.contains("vma.shm = None;"));
}
