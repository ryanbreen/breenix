//! DEV-50(3): the first edits in a writable clone of a populated root.
//!
//! A new clone shares its source tree's root node. The first copy-on-write of
//! a shared node takes a reference to each of its children, or to each data
//! extent of a leaf (txn.rs `prepare`). Each reference is a deferred SPACE
//! change that publication owes, so the grant of the edit that causes the
//! copy has to fund them. The R240's paired deploy latched its writer
//! failed with ENOSPC on the first directory create in its draft.
use super::*;
use super::incremental_files_view::{populated_image, Disk};
use super::staging_tree::{clone_root, file, write};
use std::sync::Mutex as HostMutex;

/// 700 files of 30 one-block extents: a one-level FS tree whose root has a
/// few hundred children, and a SPACE tree of about a hundred nodes, so its
/// whole-tree bound does not cover a short grant.
fn populated_clone() -> (Arc<BreenFs<outstanding_grants::NoWait>>, u64) {
    let disk = Arc::new(Disk(HostMutex::new(populated_image(700, 30, 32768))));
    let fs = BreenFs::mount_writable(
        disk, Arc::new(BufferCache::new(outstanding_grants::NoWait)), 3263050, 1,
    ).unwrap();
    clone_root(&fs, b"paired.new", 0);
    let stage = fs.attach_staging(b"paired.new").unwrap();
    (fs, stage)
}

fn writable<W: Wait>(fs: &BreenFs<W>, step: &str) {
    let state = fs.inner.state.lock();
    assert!(!state.failed, "{step} latched the writer failed: {:?}", state.failure);
}

/// The paired installer's `begin` makes `.breenix-incoming` and its
/// `objects` directory in the draft (persistent_install/paired/guest.rs),
/// then the upload creates, writes and renames objects there, and `apply`
/// rewrites files the draft still shares with the running root. Each edit
/// must succeed, leave the writer writable, and publish only the draft.
#[test]
fn first_creates_and_object_writes_in_a_populated_clone() {
    let (fs, stage) = populated_clone();
    let attrs = InodeAttrs::new(0o700, 0, 0);
    let incoming = fs.create(stage, b".breenix-incoming", NewNode::Directory { attrs });
    writable(&fs, "the first directory create in the clone");
    let incoming = incoming.unwrap();
    let objects = fs.create(incoming, b"objects", NewNode::Directory { attrs }).unwrap();
    writable(&fs, "the objects directory create");
    let object = file(&fs, objects, b"object.part").unwrap();
    let bytes = vec![0x5a; 3 * BLOCK_SIZE];
    assert_eq!(write(&fs, &object, 0, &bytes), Ok(bytes.len()));
    fs.rename(objects, b"object.part", objects, b"object", RenameMode::Replace, &|_, _, _, _| Ok(()))
        .unwrap();
    writable(&fs, "the object write and rename");
    let shared = fs.open(fs.lookup(stage, b"file-0350").unwrap()).unwrap();
    assert_eq!(write(&fs, &shared, 7 * BLOCK_SIZE as u64, &bytes), Ok(bytes.len()));
    writable(&fs, "the rewrite of a shared file");
    fs.fsync(&fs.open(stage).unwrap(), false).unwrap();
    writable(&fs, "publication");
    assert_eq!(fs.lookup(objects, b"object").map(|_| ()), Ok(()));
    assert_eq!(fs.lookup(incoming, b"objects"), Ok(objects));
    assert_eq!(fs.lookup(2, b".breenix-incoming"), Err(Errno(ENOENT)));
    assert_eq!(fs.inner.state.lock().reservations.outstanding(), 0);
}
