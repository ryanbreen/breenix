#[path = "../../../userspace/programs/src/memfsd/tree.rs"]
mod tree;
use tree::Tree;
use ufs_abi::{payload::{Attr, Reader, Writer, ROOT, S_IFDIR, S_IFREG}, Op, Request};

fn payload(f: impl FnOnce(&mut Writer<'_>)) -> Vec<u8> {
    let mut buf = vec![0; ufs_abi::MAX_DATA];
    let mut w = Writer::new(&mut buf);
    f(&mut w);
    let n = w.len();
    buf.truncate(n);
    buf
}
fn request(t: &mut Tree, op: Op, node: u64, payload: &[u8]) -> Vec<u8> {
    t.serve(&Request { id: 7, op, node, payload }).unwrap()
}
fn create(t: &mut Tree, parent: u64, name: &str, op: Op) -> Attr {
    let p = payload(|w| { w.u32(0o755).unwrap(); w.name(name.as_bytes()).unwrap(); });
    Attr::decode(&request(t, op, parent, &p)).unwrap()
}

#[test]
fn files_replace_directories_and_open_unlinked_handles() {
    let mut t = Tree::new();
    let dir = create(&mut t, ROOT, "dir", Op::Mkdir);
    assert_eq!(dir.mode & 0o170000, S_IFDIR);
    let a = create(&mut t, dir.node, "a", Op::Create);
    assert_eq!(a.mode & 0o170000, S_IFREG);
    let open = request(&mut t, Op::Open, a.node, &2u32.to_le_bytes());
    let handle = Reader::new(&open).u64().unwrap();
    let data = payload(|w| { w.u64(handle).unwrap(); w.u64(3).unwrap(); w.bytes(b"hello").unwrap(); });
    assert_eq!(request(&mut t, Op::Write, a.node, &data), 5u32.to_le_bytes());
    let read = payload(|w| { w.u64(handle).unwrap(); w.u64(0).unwrap(); w.u32(100).unwrap(); });
    assert_eq!(request(&mut t, Op::Read, a.node, &read), b"\0\0\0hello");
    create(&mut t, dir.node, "b", Op::Create);
    let rename = payload(|w| { w.u64(dir.node).unwrap(); w.name(b"a").unwrap(); w.name(b"b").unwrap(); });
    request(&mut t, Op::Rename, dir.node, &rename);
    let list = request(&mut t, Op::Readdir, dir.node, &0u64.to_le_bytes());
    let mut list = Reader::new(&list);
    assert_eq!(list.u64().unwrap(), 1); // next directory cookie
    assert_eq!(list.u64().unwrap(), a.node);
    assert_eq!(list.u32().unwrap(), a.mode);
    assert_eq!(list.name().unwrap(), b"b");
    list.finish().unwrap();
    let truncate = payload(|w| { w.u64(2).unwrap(); });
    assert_eq!(Attr::decode(&request(&mut t, Op::Setattr, a.node, &truncate)).unwrap().size, 2);
    request(&mut t, Op::Fsync, a.node, &handle.to_le_bytes());
    request(&mut t, Op::Unlink, dir.node, &payload(|w| { w.name(b"b").unwrap(); }));
    assert_eq!(request(&mut t, Op::Read, a.node, &read), b"\0\0");
    request(&mut t, Op::Release, a.node, &handle.to_le_bytes());
    request(&mut t, Op::Rmdir, ROOT, &payload(|w| { w.name(b"dir").unwrap(); }));
    let stats = request(&mut t, Op::Statfs, ROOT, &[]);
    let mut stats = Reader::new(&stats);
    assert_eq!(stats.u64().unwrap(), 4096);
    let blocks = stats.u64().unwrap();
    assert!(blocks > 0);
    assert!(stats.u64().unwrap() <= blocks);
    assert!(stats.u64().unwrap() <= blocks);
    assert!(stats.u64().unwrap() > 0);
    assert!(stats.u64().unwrap() > 0);
    assert_eq!(stats.u64().unwrap(), 255);
    stats.finish().unwrap();
}

#[test]
fn malformed_requests_and_invalid_namespace_changes_fail_without_mutation() {
    let mut t = Tree::new();
    let dir = create(&mut t, ROOT, "dir", Op::Mkdir);
    create(&mut t, dir.node, "child", Op::Create);
    for (op, node, p, errno) in [
        (Op::Lookup, ROOT, vec![4, 0, b'x'], 22),
        (Op::Getattr, ROOT, vec![0], 22),
        (Op::Lookup, ROOT, vec![3, 0, b'a', b'/', b'b'], 22),
        (Op::Rmdir, ROOT, payload(|w| { w.name(b"dir").unwrap(); }), 39),
        (Op::Rename, ROOT, payload(|w| { w.u64(dir.node).unwrap(); w.name(b"dir").unwrap(); w.name(b"loop").unwrap(); }), 22),
    ] {
        assert_eq!(t.serve(&Request { id: 1, op, node, payload: &p }), Err(errno));
    }
    let lookup = payload(|w| { w.name(b"child").unwrap(); });
    assert!(Attr::decode(&request(&mut t, Op::Lookup, dir.node, &lookup)).is_ok());
}
