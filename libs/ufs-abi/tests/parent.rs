#[path = "../../../userspace/programs/src/memfsd/tree.rs"]
mod tree;
use tree::Tree;
use ufs_abi::{payload::{Attr, Reader, Writer, ROOT}, Op, Request};

fn payload(f: impl FnOnce(&mut Writer<'_>)) -> Vec<u8> {
    let mut bytes = vec![0; 1024];
    let mut w = Writer::new(&mut bytes);
    f(&mut w);
    let n = w.len();
    bytes.truncate(n);
    bytes
}
#[test]
fn held_directory_reports_its_new_parent_and_name_after_rename() {
    // Literal opcode 16: Parent has an empty request and replies parent:u64,name.
    let wire = [1,0,0,0,0,0,0,0, 16,0, 2,0,0,0,0,0,0,0, 0,0,0,0];
    let parent_query = Request::decode(&wire).unwrap();
    let mut tree = Tree::new();
    let mkdir = payload(|w| { w.u32(0o755).unwrap(); w.name(b"old").unwrap(); });
    let attr = tree.serve(&Request { id: 0, op: Op::Mkdir, node: ROOT, payload: &mkdir }).unwrap();
    assert_eq!(Attr::decode(&attr).unwrap().node, parent_query.node);
    let opened = tree.serve(&Request { id: 0, op: Op::Open, node: 2, payload: &0u32.to_le_bytes() }).unwrap();
    let handle = Reader::new(&opened).u64().unwrap();
    let rename = payload(|w| { w.u64(ROOT).unwrap(); w.name(b"old").unwrap(); w.name(b"new").unwrap(); });
    tree.serve(&Request { id: 0, op: Op::Rename, node: ROOT, payload: &rename }).unwrap();
    assert_eq!(tree.serve(&parent_query).unwrap(), [1,0,0,0,0,0,0,0, 3,0,b'n',b'e',b'w']);
    tree.serve(&Request { id: 0, op: Op::Release, node: 2, payload: &handle.to_le_bytes() }).unwrap();
}
