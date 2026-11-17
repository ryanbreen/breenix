//! E24 ledger 485(h): mapped-file properties from memory and signals, through
//! the production BreenFS VFS and file_map mutation/writeback algorithms.
//! Host bindings model user leaves; they do not execute signals or a scheduler.
use crate::{
    attrs::InodeAttrs,
    block::cache::BufferCache,
    breenfs::{commit::{Committer, DeviceStore}, format::{Superblock, Time}, ops::Files, vfs::BreenFs},
    memory::file_map::{bind_host, sync_range, HostFault},
    read_conformance::{Disk, NoWait},
    vfs::*,
};
use std::sync::{Arc, Mutex};
const PAGE: u64 = 4096;

fn fixture(bytes: &[u8]) -> (Arc<BreenFs<NoWait>>, Arc<Disk>, Arc<VNode>) {
    let disk = Arc::new(Disk { bytes: Mutex::new(vec![0; 64 * 1024 * 1024]), writable: true });
    let sb = Superblock { fs_uuid: [0x48; 16], generation: 1, slot: 1,
        total_blocks: 16384, global_reserve: 4096, next_root_id: 2,
        ..Superblock::default() };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let ino = files.create(2, b"mapped", 0o600, Time::default()).unwrap();
    if !bytes.is_empty() { files.write(disk.as_ref(), ino, 0, bytes, Time::default()).unwrap(); }
    files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    drop(files);
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap();
    let node = fs.open(ino).unwrap();
    (fs, disk, node)
}
fn read(fs: &dyn FileSystem, node: &VNode, offset: u64, length: usize) -> Vec<u8> {
    let mut bytes = vec![0xee; length];
    let n = fs.read(node, offset, &mut bytes).unwrap();
    bytes.truncate(n);
    bytes
}
fn write(fs: &dyn FileSystem, node: &VNode, offset: u64, bytes: &[u8]) {
    let result = fs.write(node, WriteReq { at: WriteAt::Offset(offset), limit: u64::MAX,
        clear_setid: false }, bytes).unwrap();
    assert_eq!(result.bytes, bytes.len());
}
fn cold(disk: Arc<Disk>, ino: u64, length: usize) -> Vec<u8> {
    let fs = BreenFs::mount(disk, Arc::new(BufferCache::new(NoWait)), 72, 1).unwrap();
    read(fs.as_ref(), &fs.open(ino).unwrap(), 0, length)
}

#[test]
fn shared_child_stores_and_msync_async_are_visible_to_read() {
    // memory shared/file-child-sees and locking/msync-async: no writeback
    // may be needed to make a shared store visible to read(2).
    let (fs, disk, node) = fixture(&[0; 4096]);
    let parent = bind_host(node.clone(), PAGE, true, true, true);
    let child = bind_host(fs.open(node.ino).unwrap(), PAGE, true, true, true);
    for (i, byte) in b"child".iter().enumerate() { child.store(100 + i as u64, *byte).unwrap(); }
    assert_eq!((100..105).map(|i| parent.load(i).unwrap()).collect::<Vec<_>>(), b"child");
    assert_eq!(read(fs.as_ref(), &node, 100, 5), b"child");
    let expected: Vec<_> = (0..4096).map(|i| (i % 239) as u8).collect();
    for (i, byte) in expected.iter().enumerate() { child.store(i as u64, *byte).unwrap(); }
    node.map.request_writeback(); // MS_ASYNC queues; read must already see it.
    assert_eq!(read(fs.as_ref(), &fs.open(node.ino).unwrap(), 0, 4096), expected);
    assert_eq!(cold(disk.clone(), node.ino, 4096), vec![0; 4096]);
    fs.fsync(&node, false).unwrap();
    assert_eq!(cold(disk, node.ino, 4096), expected);
}

#[test]
fn file_writes_update_cached_aliases_and_bound_holes_before_msync_invalidate() {
    // memory shared/file-write-seen and locking/msync-invalidate; the sparse
    // second page also requires prepare_write before any file bytes change.
    let (fs, disk, node) = fixture(&[b'o'; 4096]);
    fs.resize(&node, 2 * PAGE, false, &|_| Ok(())).unwrap();
    let cached = bind_host(node.clone(), 2 * PAGE, true, true, true);
    assert_eq!(cached.load(0), Ok(b'o'));
    write(fs.as_ref(), &node, 0, b"new");
    assert_eq!((0..3).map(|i| cached.load(i).unwrap()).collect::<Vec<_>>(), b"new");
    sync_range(&node, 0, 2).unwrap(); // MS_SYNC|MS_INVALIDATE writes current bytes.
    assert_eq!((0..3).map(|i| cached.load(i).unwrap()).collect::<Vec<_>>(), b"new");
    // A fresh empty file's existing binding has no resident frame at all.
    let ino = fs.create(2, b"hole", NewNode::Regular { attrs: InodeAttrs::new(0o600, 0, 0) }).unwrap();
    let hole_node = fs.open(ino).unwrap();
    fs.resize(&hole_node, PAGE, false, &|_| Ok(())).unwrap();
    let hole = bind_host(hole_node.clone(), PAGE, true, false, false);
    write(fs.as_ref(), &hole_node, 50, b"fresh");
    assert_eq!((50..55).map(|i| hole.load(i).unwrap()).collect::<Vec<_>>(), b"fresh");
    fs.fsync(&hole_node, false).unwrap();
    assert_eq!(&cold(disk, ino, 4096)[50..55], b"fresh");
}

#[test]
fn eof_tail_is_zeroed_and_truncated_leaves_are_revoked() {
    // memory shared/eof-zero-tail and eof-truncated: stores outside EOF are
    // discarded on extension or shrink/regrowth.
    let (fs, disk, node) = fixture(&[b'e'; 100]);
    let map = bind_host(node.clone(), 2 * PAGE, true, true, true);
    assert!((100..PAGE).all(|i| map.load(i) == Ok(0)));
    map.store(200, b'z').unwrap();
    sync_range(&node, 0, 2).unwrap();
    assert_eq!(fs.getattr(node.ino).unwrap().size, 100);
    assert_eq!(cold(disk.clone(), node.ino, 4096), vec![b'e'; 100]);
    fs.resize(&node, 2 * PAGE, false, &|_| Ok(())).unwrap();
    assert_eq!(map.load(200), Ok(0));
    assert_eq!(read(fs.as_ref(), &node, 100, (2 * PAGE - 100) as usize), vec![0; (2 * PAGE - 100) as usize]);
    write(fs.as_ref(), &node, PAGE, &[b't'; 4096]);
    assert_eq!(map.load(PAGE + 8), Ok(b't'));
    assert!(map.has_leaf(1));
    fs.resize(&node, 100, false, &|_| Ok(())).unwrap();
    assert!(!map.has_leaf(1), "truncate must revoke an already-present leaf");
    assert_eq!(map.load(PAGE + 8), Err(HostFault::Sigbus));
    assert_eq!(map.load(50), Ok(b'e'));
    fs.resize(&node, 2 * PAGE, false, &|_| Ok(())).unwrap();
    assert_eq!(map.load(PAGE + 8), Ok(0));
    assert_eq!(map.load(200), Ok(0));
    fs.fsync(&node, false).unwrap();
    let mut expected = vec![0; 8192]; expected[..100].fill(b'e');
    assert_eq!(cold(disk, node.ino, 8192), expected);
}

#[test]
fn bound_empty_private_file_grows_for_owner_and_clone_vm_sibling() {
    // signals frames/cache-hole and the file-backed part of frames/clone-vm:
    // mmap precedes ftruncate, then an untouched private stack takes a frame.
    // Also cover cached private bytes, as clone-vm calls fresh_file_frame twice.
    for cached in [false, true] {
        let contents = vec![if cached { 0x6b } else { 0 }; 3 * 4096];
        let (fs, disk, node) = fixture(if cached { &contents } else { &[] });
        let owner = bind_host(node.clone(), 3 * PAGE, false, true, cached);
        let sibling = owner.clone(); // CLONE_VM uses the owner's binding.
        if !cached { fs.resize(&node, 3 * PAGE, false, &|_| Ok(())).unwrap(); }
        assert_eq!(node.size.load(core::sync::atomic::Ordering::Acquire), 3 * PAGE);
        assert_eq!(node.map.mapped_size.load(core::sync::atomic::Ordering::Acquire), 3 * PAGE,
            "fault delivery must not retry a finished bound EOF transition forever");
        for i in 2 * PAGE - 128..2 * PAGE + 128 { sibling.store(i, 0xa7).unwrap(); }
        assert!((2 * PAGE - 128..2 * PAGE + 128).all(|i| owner.load(i) == Ok(0xa7)));
        assert_eq!(read(fs.as_ref(), &node, 0, contents.len()), contents);
        fs.fsync(&node, false).unwrap();
        assert_eq!(cold(disk, node.ino, contents.len()), contents);
    }
}
