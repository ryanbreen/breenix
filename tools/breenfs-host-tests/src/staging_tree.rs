//! The staging tree on the live writer and the root epoch
//! (docs/metal/paired-generations.md 3.1 and 3.3, checks T-A3 and T-A5).
//! These run against the production adapter, so they live beside it rather
//! than in tests/snapshot.rs, which reaches only the Files layer.
use super::*;
use alloc::{format, vec};
use std::sync::Mutex as HostMutex;

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("sequential staging check waited for I/O")
    }
    fn wake_all(&self) {}
}

pub(super) struct Disk(HostMutex<Vec<u8>>);
impl BlockDevice for Disk {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        bytes.copy_from_slice(&self.0.lock().unwrap()[start..start + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        self.0.lock().unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        16384
    }
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
}

/// A formatted volume holding `gen-A` and the default tree.
fn image() -> Arc<Disk> {
    let disk = Arc::new(Disk(HostMutex::new(vec![0; 64 * 1024 * 1024])));
    let sb = Superblock {
        fs_uuid: [0x5a; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        next_root_id: 2,
        ..Superblock::default()
    };
    let mut files = Files::format(super::super::commit::DeviceStore(disk.as_ref()), &sb).unwrap();
    files.create_empty_root(b"gen-A", Time::default()).unwrap();
    files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    disk
}

/// A persistent writer on `disk`: `gen-A` booted, with the default tree, as
/// the kernel's `mount_persistent_cached` builds it.
fn mount_persistent(disk: Arc<Disk>) -> Arc<BreenFs<impl Wait>> {
    // The kernel mounts the booted pair from the system cache, which lives forever.
    let cache: &'static BufferCache<NoWait> = Box::leak(Box::new(BufferCache::new(NoWait)));
    let fs = BreenFs::mount_persistent_cached(disk, cache, 91, b"gen-A").unwrap();
    fs.init_service().unwrap();
    fs
}

pub(super) fn persistent() -> Arc<BreenFs<impl Wait>> {
    mount_persistent(image())
}

pub(super) fn request(name: &[u8], flags: u32) -> SnapshotRequest {
    let mut request = SnapshotRequest { flags, name_len: name.len() as u32, name: [0; 256] };
    request.name[..name.len()].copy_from_slice(name);
    request
}

pub(super) fn ioctl<W: Wait, T>(fs: &BreenFs<W>, request: u64, arg: &mut T) -> Result<u64, Errno> {
    let node = fs.open(2)?;
    fs.ioctl(&node, request, arg as *mut T as u64)
}

pub(super) fn clone_root<W: Wait>(fs: &BreenFs<W>, name: &[u8], flags: u32) {
    ioctl(fs, SNAPSHOT_CREATE, &mut request(name, flags)).unwrap();
}

pub(super) fn epoch<W: Wait>(fs: &BreenFs<W>) -> u64 {
    let mut out = RootEpoch::default();
    ioctl(fs, ROOT_EPOCH, &mut out).unwrap();
    out.epoch
}

pub(super) fn freeze<W: Wait>(fs: &BreenFs<W>) -> Result<u64, (Errno, RootEpoch)> {
    let mut out = RootEpoch::default();
    ioctl(fs, ROOT_FREEZE, &mut out).map(|_| out.epoch).map_err(|error| (error, out))
}

pub(super) fn thaw<W: Wait>(fs: &BreenFs<W>) {
    ioctl(fs, ROOT_THAW, &mut RootEpoch::default()).unwrap();
}

pub(super) fn file<W: Wait>(fs: &BreenFs<W>, dir: u64, name: &[u8]) -> Result<Arc<VNode>, Errno> {
    let ino = fs.create(dir, name, NewNode::Regular { attrs: InodeAttrs::new(0o100644, 0, 0) })?;
    fs.open(ino)
}

pub(super) fn write<W: Wait>(fs: &BreenFs<W>, node: &VNode, offset: u64, bytes: &[u8]) -> Result<usize, Errno> {
    let req = WriteReq { at: WriteAt::Offset(offset), limit: u64::MAX, clear_setid: false };
    fs.write(node, req, bytes).map(|result| result.bytes).map_err(|error| error.errno())
}

fn read<W: Wait>(fs: &BreenFs<W>, ino: u64) -> Vec<u8> {
    let node = fs.open(ino).unwrap();
    let mut bytes = vec![0; fs.getattr(ino).unwrap().size as usize];
    assert_eq!(fs.read(&node, 0, &mut bytes).unwrap(), bytes.len());
    bytes
}

fn names<W: Wait>(fs: &BreenFs<W>, dir: u64) -> Vec<Vec<u8>> {
    let mut names = Vec::new();
    fs.readdir(dir, 0, &mut |entry, _| {
        names.push(entry.name);
        Ok(true)
    })
    .unwrap();
    names
}

const STAGED_ROOT: u64 = ops::STAGING_TREE | 2;


/// T-A3: attach refuses the booted root, root 1, a read-only root, a missing
/// root and a second staging tree; the attached root cannot be deleted or
/// renamed; detach waits for an open staged file. The epoch ignores reads,
/// atime touches, default-tree and staging writes, and counts booted writes.
/// ROOT_FREEZE refuses booted-root changes with EROFS and holds the epoch
/// while default-tree writes and atime touches continue; ROOT_THAW admits
/// writes again. A booted write split around the freeze is counted in the
/// epoch the freeze returns for the part before it, and refused after it.
#[test]
fn staging_refusals_epoch_and_freeze() {
    let fs = persistent();
    clone_root(&fs, b"B.new", 0);
    clone_root(&fs, b"C.new", 0);
    clone_root(&fs, b"ro", SNAPSHOT_READ_ONLY);
    assert_eq!(fs.attach_staging(b"gen-A"), Err(Errno(EBUSY)), "booted root");
    assert_eq!(fs.attach_staging(b"default"), Err(Errno(EBUSY)), "root 1");
    assert_eq!(fs.attach_staging(b"ro"), Err(Errno(EROFS)), "read-only root");
    assert_eq!(fs.attach_staging(b"missing"), Err(Errno(ENOENT)));
    assert_eq!(fs.attach_staging(b"B.new"), Ok(STAGED_ROOT));
    assert_eq!(fs.attach_staging(b"C.new"), Err(Errno(EBUSY)), "second staging tree");
    assert_eq!(ioctl(&fs, SNAPSHOT_DELETE, &mut request(b"B.new", 0)), Err(Errno(EBUSY)));
    let mut rename = SnapshotRename { from: request(b"B.new", 0), to: request(b"gen-B", 0) };
    assert_eq!(ioctl(&fs, SNAPSHOT_RENAME, &mut rename), Err(Errno(EBUSY)));

    let booted = file(&fs, 2, b"booted").unwrap();
    let start = epoch(&fs);
    let mut bytes = [0; 4];
    fs.read(&booted, 0, &mut bytes).unwrap();
    fs.touch(&booted, true);
    fs.getattr(booted.ino).unwrap();
    assert_eq!(epoch(&fs), start, "a read and an atime touch");
    let log = file(&fs, fs.default_root_inode(), b"log").unwrap();
    write(&fs, &log, 0, b"default").unwrap();
    assert_eq!(epoch(&fs), start, "a default-tree write");
    let staged = file(&fs, STAGED_ROOT, b"staged").unwrap();
    write(&fs, &staged, 0, b"staged").unwrap();
    assert_eq!(epoch(&fs), start, "a staging write");
    write(&fs, &booted, 0, b"live").unwrap();
    assert!(epoch(&fs) > start, "a booted-root write");

    assert_eq!(fs.detach_staging(), Err(Errno(EBUSY)), "an open staged file");
    drop(staged);
    fs.detach_staging().unwrap();
    assert_eq!(fs.getattr(STAGED_ROOT).map(|attrs| attrs.ino), Err(Errno(ENOENT)));

    let frozen = freeze(&fs).unwrap();
    assert_eq!(frozen, epoch(&fs));
    let file_attrs = || NewNode::Regular { attrs: InodeAttrs::new(0o100644, 0, 0) };
    assert_eq!(fs.create(2, b"refused", file_attrs()), Err(Errno(EROFS)));
    assert_eq!(write(&fs, &booted, 0, b"late"), Err(Errno(EROFS)));
    let renamed = fs.rename(2, b"booted", 2, b"moved", RenameMode::Replace, &|_, _, _, _| Ok(()));
    assert_eq!(renamed, Err(Errno(EROFS)));
    let chmod = SetAttr { expected: None, mode: Some(0o600), uid: None, gid: None, atime: None, mtime: None };
    assert_eq!(fs.setattr(booted.ino, chmod), Err(Errno(EROFS)));
    assert_eq!(epoch(&fs), frozen);
    write(&fs, &log, 0, b"DEFAULT").unwrap();
    fs.touch(&booted, true);
    fs.sync().unwrap();
    assert_eq!(epoch(&fs), frozen, "default-tree writes and atime touches while frozen");
    thaw(&fs);
    write(&fs, &booted, 0, b"open").unwrap();
    assert!(epoch(&fs) > frozen);

    // The fixed interleaving: a write of two parts, with the freeze between them.
    let large = file(&fs, 2, b"large").unwrap();
    let before = epoch(&fs);
    let returned = std::rc::Rc::new(core::cell::Cell::new(None));
    let (hook_fs, hook_returned) = (fs.clone(), returned.clone());
    measurement::on_write_part(Some(Box::new(move || hook_returned.set(Some(freeze(&hook_fs))))));
    let bytes = vec![0x5a; WRITE_PART + 4096];
    let written = write(&fs, &large, 0, &bytes);
    measurement::on_write_part(None);
    let frozen = returned.get().expect("the freeze ran between the parts").unwrap();
    assert_eq!(written, Ok(WRITE_PART), "the second part is refused");
    assert!(frozen > before, "the first part is counted in the frozen epoch");
    assert_eq!(epoch(&fs), frozen);
    assert_eq!(fs.getattr(large.ino).unwrap().size, WRITE_PART as u64);
    thaw(&fs);
    fs.shutdown("staging-refusals", u64::MAX).unwrap();
}

/// T-A5: two clones whose local inodes alias (a file in A is a directory in
/// B at the same number; the next number has different bytes and extents).
/// After A's inodes are stat'ed, read, listed and mapped, detaching A and
/// attaching B serves B's inode, entries and bytes from the inode cache, the
/// directory view and the mapping cache.
#[test]
fn reattach_serves_the_new_root_not_cached_inodes() {
    let fs = persistent();
    clone_root(&fs, b"A.new", 0);
    clone_root(&fs, b"B.new", 0);
    fs.attach_staging(b"A.new").unwrap();
    let first = file(&fs, STAGED_ROOT, b"first").unwrap();
    let second = file(&fs, STAGED_ROOT, b"second").unwrap();
    write(&fs, &first, 0, b"A's first file").unwrap();
    write(&fs, &second, 0, b"A's second file").unwrap();
    let (aliased, other) = (first.ino, second.ino);
    drop((first, second));
    fs.detach_staging().unwrap();
    fs.attach_staging(b"B.new").unwrap();
    let dir = fs.create(STAGED_ROOT, b"dir", NewNode::Directory { attrs: InodeAttrs::new(0o040755, 0, 0) }).unwrap();
    assert_eq!(dir, aliased, "the fixture needs aliased inode numbers");
    let second = file(&fs, dir, b"second").unwrap();
    assert_eq!(second.ino, other);
    let b_bytes: Vec<u8> = (0..3 * 4096).map(|i| (i % 251) as u8).collect();
    write(&fs, &second, 0, &b_bytes).unwrap();
    file(&fs, dir, b"entry").unwrap();
    drop(second);
    fs.detach_staging().unwrap();

    fs.attach_staging(b"A.new").unwrap();
    assert_eq!(read(&fs, aliased), b"A's first file");
    assert_eq!(read(&fs, other), b"A's second file");
    let mapped = crate::memory::file_map::bind_host(fs.open(other).unwrap(), 4096, true, false, true);
    assert_eq!(mapped.load(0), Ok(b'A'));
    drop(mapped);
    // Stat last, so A's inodes are what the inode cache and NodeStates hold
    // when A is detached.
    assert_eq!(fs.getattr(aliased).unwrap().kind, NodeKind::Regular);
    assert_eq!(fs.getattr(other).unwrap().size, b"A's second file".len() as u64);
    {
        let state = fs.inner.state.lock();
        assert!(state.inodes.get(aliased).is_some() && state.nodes.contains_key(&other),
            "the fixture needs A's inodes cached at detach");
    }
    fs.detach_staging().unwrap();

    fs.attach_staging(b"B.new").unwrap();
    assert_eq!(fs.getattr(aliased).unwrap().kind, NodeKind::Directory);
    assert_eq!(names(&fs, aliased), [b"second".to_vec(), b"entry".to_vec()]);
    assert_eq!(fs.getattr(other).unwrap().size, b_bytes.len() as u64);
    assert_eq!(read(&fs, other), b_bytes);
    let mapped = crate::memory::file_map::bind_host(fs.open(other).unwrap(), 3 * 4096, true, false, true);
    assert_eq!(mapped.load(7), Ok(b_bytes[7]), "a page A cached for the same inode number");
    assert_eq!(mapped.load(4096 + 7), Ok(b_bytes[4096 + 7]));
    drop(mapped);
    fs.detach_staging().unwrap();
    fs.shutdown("staging-reattach", u64::MAX).unwrap();
}

/// Ruling DEV-44(2a). A writable shared mapping of a booted-root file
/// changes the file with no filesystem admission. With one alive across the
/// clone, a store after the clone leaves the epoch unchanged, so a freeze that
/// admitted it would return E2 == E1 for a root that changed. ROOT_FREEZE
/// refuses with EBUSY and the mapping count instead. Once the mapping is
/// gone, its stores are written back and counted before the freeze. While
/// frozen, a writable shared mapping of a booted-root file and mprotect
/// adding write to a shared one are refused; private and default-tree
/// mappings are not.
#[test]
fn freeze_refuses_writable_shared_root_mappings() {
    use crate::memory::file_map::try_bind_host;
    let fs = persistent();
    let node = file(&fs, 2, b"db").unwrap();
    write(&fs, &node, 0, &[b'a'; 4096]).unwrap();
    let mapping = try_bind_host(node.clone(), 4096, true, true, true).unwrap();
    let e1 = epoch(&fs);
    clone_root(&fs, b"B.new", 0);
    mapping.store(0, b'Z').unwrap();
    match freeze(&fs) {
        Ok(e2) => panic!(
            "ROOT_FREEZE admitted a booted root with a writable shared mapping alive: \
             E1 = {e1}, E2 = {e2} after a mapped store changed the file (the E1 == E2 race)"
        ),
        Err((error, busy)) => {
            assert_eq!(error, Errno(EBUSY));
            assert_eq!(busy.writable_mappings, 1, "{busy:?}");
        }
    }
    drop(mapping);
    let e2 = freeze(&fs).unwrap();
    assert!(e2 > e1, "the mapped store is counted before the freeze: E1 = {e1}, E2 = {e2}");
    assert_eq!(read(&fs, node.ino)[0], b'Z');

    assert!(try_bind_host(node.clone(), 4096, true, true, false).is_err(), "mmap(PROT_WRITE, MAP_SHARED)");
    let reader = try_bind_host(node.clone(), 4096, true, false, false).unwrap();
    assert!(reader.protect_write().is_err(), "mprotect adding PROT_WRITE");
    let private = try_bind_host(node.clone(), 4096, false, true, false).unwrap();
    let log = file(&fs, fs.default_root_inode(), b"log").unwrap();
    write(&fs, &log, 0, b"log").unwrap();
    let shared_log = try_bind_host(log.clone(), 4096, true, true, false).unwrap();
    assert_eq!(epoch(&fs), e2);
    drop((private, shared_log));
    thaw(&fs);
    reader.protect_write().expect("mprotect after thaw");
    drop(reader);
    fs.shutdown("freeze-mappings", u64::MAX).unwrap();
}

/// Review of PR #3273: ROOT_FREEZE and ROOT_THAW change the flag only when
/// the caller gets the result. A freeze whose output cannot be written
/// returns EFAULT with the root still writable; a thaw whose output cannot be
/// written returns EFAULT with the root still frozen.
#[test]
fn root_freeze_and_thaw_change_nothing_on_efault() {
    let fs = persistent();
    let booted = file(&fs, 2, b"booted").unwrap();
    let root = fs.open(2).unwrap();
    assert_eq!(fs.ioctl(&root, ROOT_FREEZE, 0), Err(Errno(EFAULT)));
    write(&fs, &booted, 0, b"open").expect("a freeze that returned EFAULT left the root frozen");
    let frozen = freeze(&fs).unwrap();
    assert_eq!(fs.ioctl(&root, ROOT_THAW, 0), Err(Errno(EFAULT)));
    assert_eq!(write(&fs, &booted, 0, b"late"), Err(Errno(EROFS)), "a thaw that returned EFAULT thawed the root");
    assert_eq!(epoch(&fs), frozen);
    thaw(&fs);
    write(&fs, &booted, 0, b"open").unwrap();
    drop((root, booted));
    fs.shutdown("freeze-efault", u64::MAX).unwrap();
}

/// Review of PR #3273: the freeze writes back only the booted tree's
/// mappings. A default-tree file (a log in /var) with an unwritten mapped
/// store is neither counted nor written back, so its writeback errors (such
/// as ENOSPC) cannot refuse a booted-root freeze.
#[test]
fn root_freeze_leaves_default_tree_mappings_alone() {
    use crate::memory::file_map::try_bind_host;
    let fs = persistent();
    let log = file(&fs, fs.default_root_inode(), b"log").unwrap();
    write(&fs, &log, 0, &[b'a'; 4096]).unwrap();
    let mapping = try_bind_host(log.clone(), 4096, true, true, true).unwrap();
    mapping.store(0, b'Z').unwrap();
    // Unmapped, so a writeback would leave the file clean.
    drop(mapping);
    assert!(log.map.has_dirty(), "the fixture needs an unwritten default-tree store");
    let frozen = freeze(&fs).expect("a default-tree mapping does not block the freeze");
    assert!(log.map.has_dirty(), "ROOT_FREEZE wrote back a default-tree mapping");
    assert_eq!(epoch(&fs), frozen);
    thaw(&fs);
    fs.sync().unwrap();
    assert!(!log.map.has_dirty(), "the store is written back by the next sync");
    fs.shutdown("freeze-default-mapping", u64::MAX).unwrap();
}

/// Review of PR #3273: while frozen, an unprivileged shared-write mmap or
/// mprotect of a set-ID booted-root file first clears the set-ID bits
/// (clear_mapping_privilege). That refusal is the freeze's EBUSY (DEV-44),
/// not EROFS.
#[test]
fn frozen_set_id_shared_write_is_busy() {
    let fs = persistent();
    let ino = fs.create(2, b"setid", NewNode::Regular { attrs: InodeAttrs::new(0o104755, 0, 0) }).unwrap();
    let node = fs.open(ino).unwrap();
    freeze(&fs).unwrap();
    assert_eq!(fs.clear_mapping_privilege(&node), Err(Errno(EBUSY)));
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o4000, 0o4000);
    thaw(&fs);
    fs.clear_mapping_privilege(&node).unwrap();
    assert_eq!(fs.getattr(ino).unwrap().attrs.mode & 0o4000, 0);
    drop(node);
    fs.shutdown("freeze-setid", u64::MAX).unwrap();
}

/// Review of PR #3273: a FIFO write sets the FIFO's mtime and ctime, a
/// persistent change to the booted tree made through `touch(node, false)`
/// with no mutation admission. It advances the epoch, and while frozen it
/// is refused: the timestamps stay as they were, as on a read-only mount.
#[test]
fn fifo_writes_count_in_the_epoch_and_respect_the_freeze() {
    let fs = persistent();
    let ino = fs.create(2, b"fifo", NewNode::Fifo { attrs: InodeAttrs::new(0o010644, 0, 0) }).unwrap();
    let fifo = fs.open(ino).unwrap();
    let old = Timestamp { sec: 1000, nsec: 0 };
    let set_old = || {
        let attrs = SetAttr { mtime: Some(old), ..SetAttr::default() };
        fs.setattr(ino, attrs).unwrap();
    };
    set_old();
    let e1 = epoch(&fs);
    fs.touch(&fifo, false);
    fs.sync().unwrap();
    assert_ne!(fs.getattr(ino).unwrap().times.mtime, old, "the fixture's FIFO write sets mtime");
    assert!(epoch(&fs) > e1, "a FIFO write after the clone left E1 == E2");
    set_old();
    let e2 = freeze(&fs).unwrap();
    fs.touch(&fifo, false);
    fs.sync().unwrap();
    assert_eq!(fs.getattr(ino).unwrap().times.mtime, old, "a FIFO write changed mtime after ROOT_FREEZE");
    assert_eq!(epoch(&fs), e2);
    thaw(&fs);
    drop(fifo);
    fs.shutdown("freeze-fifo", u64::MAX).unwrap();
}

/// The leaf of the tree at `ptr` that holds `key`.
fn leaf_of(disk: &Disk, mut ptr: format::BlockPtr, key: format::Key) -> format::BlockPtr {
    use super::super::{btree::NodeStore, commit::DeviceStore};
    while ptr.level != 0 {
        let node = DeviceStore(disk).read_node(ptr).unwrap();
        let child = node.children.partition_point(|entry| entry.key <= key).saturating_sub(1);
        ptr = node.child(child);
    }
    ptr
}

/// Review of PR #3273: attach reads the draft's root directory. A draft
/// whose root-directory leaf is unreadable is refused at attach with EIO,
/// instead of mounting and failing the first pathname lookup. The refusal
/// leaves the staging slot free and the booted root writable.
#[test]
fn attach_refuses_a_draft_whose_root_directory_is_unreadable() {
    let disk = image();
    let leaf = {
        let fs = mount_persistent(disk.clone());
        clone_root(&fs, b"B.new", 0);
        clone_root(&fs, b"C.new", 0);
        fs.attach_staging(b"B.new").unwrap();
        // Enough entries for a tree of more than one leaf, so the orphan
        // range attach already reads is not in the root directory's leaf.
        for n in 0..200 {
            file(&fs, STAGED_ROOT, format!("file-{n:03}").as_bytes()).unwrap();
        }
        fs.sync().unwrap();
        let ptr = fs.inner.state.lock().files.files.staging_pointer_for_test().unwrap();
        let inode = format::Key { object: 2, item_type: format::INODE, offset: 0 };
        let orphans = format::Key { object: ops::ORPHAN_OBJECT, item_type: format::ORPHAN, offset: 0 };
        let leaf = leaf_of(&disk, ptr, inode);
        assert_ne!(leaf.addr, leaf_of(&disk, ptr, orphans).addr, "the fixture needs a multi-leaf draft");
        fs.detach_staging().unwrap();
        fs.shutdown("corrupt-draft-build", u64::MAX).unwrap();
        leaf
    };
    disk.0.lock().unwrap()[leaf.addr as usize * BLOCK_SIZE + 100] ^= 0xff;

    let fs = mount_persistent(disk);
    assert_eq!(fs.attach_staging(b"B.new"), Err(Errno(EIO)), "a draft with an unreadable root directory");
    assert_eq!(fs.attach_staging(b"C.new"), Ok(STAGED_ROOT), "the refused attach left the staging slot free");
    fs.detach_staging().unwrap();
    file(&fs, 2, b"after").expect("the booted root stays writable");
    fs.sync().unwrap();
    fs.shutdown("corrupt-draft", u64::MAX).unwrap();
}
