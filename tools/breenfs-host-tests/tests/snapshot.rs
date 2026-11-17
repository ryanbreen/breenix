use breenfs_host_tests::{
    btree::NodeStore,
    commit::{Committer, DeviceStore},
    format::{self, BlockPtr, InternalEntry, NodeHeader, Root, Superblock, Time, BLOCK_SIZE},
    ops::{self, Error, Files, SetAttr},
    recording::RecordingDevice,
    recover, space,
};
use std::{collections::BTreeMap, time::Instant};

#[path = "../src/test_image.rs"]
mod test_image;

fn roots(device: &RecordingDevice, sb: &Superblock) -> BTreeMap<u64, Root> {
    ops::items(&DeviceStore(device), sb.space_root)
        .unwrap()
        .into_iter()
        .filter(|(key, _)| key.item_type == format::ROOT)
        .map(|(key, bytes)| (key.offset, Root::decode(&bytes).unwrap()))
        .collect()
}

// Compare encoded tree nodes as well as the file's readback.
fn tree_bytes(store: &impl NodeStore, ptr: BlockPtr) -> BTreeMap<u64, Vec<u8>> {
    let bytes = store.get(ptr.addr).unwrap();
    let header = NodeHeader::decode(&bytes).unwrap();
    let mut nodes = BTreeMap::new();
    if header.level != 0 {
        for slot in 0..usize::from(header.nitems) {
            let at = 48 + slot * 37;
            let entry = InternalEntry::decode(&bytes[at..at + 37]).unwrap();
            nodes.extend(tree_bytes(
                store,
                BlockPtr {
                    addr: entry.child_addr,
                    generation: entry.child_generation,
                    csum: entry.child_csum,
                    level: header.level - 1,
                },
            ));
        }
    }
    nodes.insert(ptr.addr, bytes);
    nodes
}

fn commit(
    files: &mut Files<'_, DeviceStore<'_>>,
    device: &RecordingDevice,
    mount: &mut Committer,
) -> Superblock {
    let sb = files.fsync(device, mount).unwrap();
    test_image::check(&device.durable_image(), "snapshot");
    sb
}

fn read(files: &Files<'_, DeviceStore<'_>>, device: &RecordingDevice, ino: u64) -> Vec<u8> {
    let mut bytes = vec![0; files.inode(ino).unwrap().size as usize];
    assert_eq!(files.read(device, ino, 0, &mut bytes).unwrap(), bytes.len());
    bytes
}

#[test]
fn snapshot_rejects_unmountable_root_name() {
    let device = RecordingDevice::new(test_image::image()).unwrap();
    let sb = recover::recover(&device, false).unwrap();
    let mut files = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    assert_eq!(files.snapshot(b"a,b", true), Err(Error::EINVAL));
}

#[test]
fn snapshot_rejects_colliding_next_root_id_without_replacing_root() {
    let device = RecordingDevice::new(test_image::image()).unwrap();
    let mut sb = recover::recover(&device, false).unwrap();
    sb.next_root_id = 1;
    let mut files = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    assert_eq!(files.snapshot(b"collision", true), Err(Error::EIO));
    assert_eq!(files.inode(2).unwrap().parent, 2);
}

#[test]
fn clone_cannot_delete_default_root() {
    let device = RecordingDevice::new(test_image::image()).unwrap();
    let sb = recover::recover(&device, false).unwrap();
    let mut mount = Committer::new(sb.clone());
    let mut files = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    let id = files.snapshot(b"clone", false).unwrap();
    let sb = files.fsync(&device, &mut mount).unwrap();
    let mut clone = Files::new(DeviceStore(&device), &sb, id, true).unwrap();
    assert_eq!(clone.delete_root(b"default"), Err(Error::EBUSY));
}

#[test]
fn readonly_snapshot_keeps_tree_bytes_after_live_writes() {
    let start = Instant::now();
    let device = RecordingDevice::new(test_image::image()).unwrap();
    let sb = recover::recover(&device, false).unwrap();
    let mut mount = Committer::new(sb.clone());
    let mut live = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    let ino = live.create(2, b"file", 0o644, Time::default()).unwrap();
    let original = vec![0x37; 2 * BLOCK_SIZE + 7];
    live.write(&device, ino, 0, &original, Time::default())
        .unwrap();
    for n in 0..96 {
        live.create(2, format!("entry{n:03}").as_bytes(), 0o600, Time::default())
            .unwrap();
    }
    let sb = commit(&mut live, &device, &mut mount);
    let source = roots(&device, &sb)[&1].clone();
    assert!(
        source.root.level > 0,
        "exercise shared ancestors and leaves"
    );
    let frozen = tree_bytes(&DeviceStore(&device), source.root);
    let mut live = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    for name in [b"".as_slice(), b".", b"..", b"a/b", b"a\0b", &[b'x'; 256]] {
        assert_eq!(live.snapshot(name, true), Err(Error::EINVAL));
    }
    assert_eq!(live.snapshot(b"frozen", true).unwrap(), sb.next_root_id);
    assert_eq!(live.snapshot(b"frozen", false), Err(Error::EEXIST));
    assert_eq!(live.snapshot(b"default", true), Err(Error::EEXIST));
    let snap_sb = commit(&mut live, &device, &mut mount);
    let all = roots(&device, &snap_sb);
    let snap = &all[&sb.next_root_id];
    assert_eq!(snap.root, source.root);
    assert_eq!(snap.next_inode, source.next_inode);
    assert_eq!(snap.source_root, 1);
    assert_eq!(snap.created_generation, snap_sb.generation);
    assert_eq!(snap.flags, 1);
    assert_eq!(snap_sb.next_root_id, sb.next_root_id + 1);
    assert_eq!(
        space::reference(
            &DeviceStore(&device),
            snap_sb.space_root,
            source.root.addr,
            4
        )
        .unwrap()
        .unwrap()
        .refcount,
        2
    );
    assert!(matches!(
        Files::new(DeviceStore(&device), &snap_sb, sb.next_root_id, true),
        Err(Error::EROFS)
    ));
    let mut snapshot = Files::new(DeviceStore(&device), &snap_sb, sb.next_root_id, false).unwrap();
    assert_eq!(
        snapshot.create(2, b"refused", 0o600, Time::default()),
        Err(Error::EROFS)
    );
    assert_eq!(
        snapshot.write(&device, ino, 0, b"refused", Time::default()),
        Err(Error::EROFS)
    );
    let mut live = Files::new(DeviceStore(&device), &snap_sb, 1, true).unwrap();
    live.write(&device, ino, BLOCK_SIZE as u64, b"changed", Time::default())
        .unwrap();
    live.unlink(2, b"entry050", Time::default()).unwrap();
    let sb = commit(&mut live, &device, &mut mount);
    let all = roots(&device, &sb);
    assert_eq!(all[&2].root, source.root);
    assert_eq!(tree_bytes(&DeviceStore(&device), all[&2].root), frozen);
    assert_ne!(all[&1].root, source.root);
    let snapshot = Files::new(DeviceStore(&device), &sb, 2, false).unwrap();
    assert_eq!(read(&snapshot, &device, ino), original);
    assert!(snapshot.lookup(2, b"entry050").is_ok());
    assert!(start.elapsed().as_secs_f64() < 60.0);
}

#[test]
fn clone_diverges_and_same_transaction_snapshot_stays_frozen() {
    let start = Instant::now();
    let device = RecordingDevice::new(test_image::image()).unwrap();
    let sb = recover::recover(&device, false).unwrap();
    let mut mount = Committer::new(sb.clone());
    let mut live = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    let ino = live.create(2, b"file", 0o644, Time::default()).unwrap();
    live.write(&device, ino, 0, b"live bytes", Time::default())
        .unwrap();
    let sb = commit(&mut live, &device, &mut mount);
    let mut live = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    let clone_id = live.snapshot(b"clone", false).unwrap();
    let sb = commit(&mut live, &device, &mut mount);
    let live_root = roots(&device, &sb)[&1].root;
    let frozen_live = tree_bytes(&DeviceStore(&device), live_root);
    let mut clone = Files::new(DeviceStore(&device), &sb, clone_id, true).unwrap();
    clone
        .write(&device, ino, 0, b"clone data", Time::default())
        .unwrap();
    let before = clone.items().unwrap();
    // This root was allocated by this transaction and is now shared. The
    // inode fast edit and ordinary B-tree edits must both copy it again.
    let frozen_id = clone.snapshot(b"clone-before", true).unwrap();
    clone
        .setattr(
            ino,
            &SetAttr {
                mode: Some(0o600),
                ..Default::default()
            },
            Time::default(),
        )
        .unwrap();
    clone
        .write(&device, ino, 0, b"clone next", Time::default())
        .unwrap();
    let sb = commit(&mut clone, &device, &mut mount);
    let all = roots(&device, &sb);
    assert_eq!(all[&clone_id].flags, 0);
    assert_eq!(all[&frozen_id].source_root, clone_id);
    assert_eq!(all[&1].root, live_root);
    assert_eq!(tree_bytes(&DeviceStore(&device), live_root), frozen_live);
    let live = Files::new(DeviceStore(&device), &sb, 1, false).unwrap();
    assert_eq!(read(&live, &device, ino), b"live bytes");
    let frozen = Files::new(DeviceStore(&device), &sb, frozen_id, false).unwrap();
    assert_eq!(frozen.items().unwrap(), before);
    assert_eq!(read(&frozen, &device, ino), b"clone data");
    let clone = Files::new(DeviceStore(&device), &sb, clone_id, true).unwrap();
    assert_eq!(read(&clone, &device, ino), b"clone next");
    assert_eq!(clone.inode(ino).unwrap().mode & 0o777, 0o600);
    assert!(start.elapsed().as_secs_f64() < 60.0);
}

#[test]
fn delete_removes_root_and_retains_dead_nodes_children() {
    let start = Instant::now();
    let device = RecordingDevice::new(test_image::image()).unwrap();
    let sb = recover::recover(&device, false).unwrap();
    let mut mount = Committer::new(sb.clone());
    let mut live = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    assert_eq!(live.delete_root(b"default"), Err(Error::EBUSY));
    assert_eq!(live.delete_root(b"missing"), Err(Error::ENOENT));
    let clone_id = live.snapshot(b"clone", false).unwrap();
    let sb = commit(&mut live, &device, &mut mount);
    let mut clone = Files::new(DeviceStore(&device), &sb, clone_id, true).unwrap();
    let ino = clone.create(2, b"owned", 0o600, Time::default()).unwrap();
    clone
        .write(&device, ino, 0, b"retained by DEAD", Time::default())
        .unwrap();
    let sb = commit(&mut clone, &device, &mut mount);
    let root = roots(&device, &sb)[&clone_id].root;
    let frozen = tree_bytes(&DeviceStore(&device), root);
    let mut live = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    live.delete_root(b"clone").unwrap();
    assert_eq!(live.delete_root(b"clone"), Err(Error::ENOENT));
    let sb = commit(&mut live, &device, &mut mount);
    assert!(!roots(&device, &sb).contains_key(&clone_id));
    let entries = ops::items(&DeviceStore(&device), sb.space_root).unwrap();
    assert!(entries.iter().any(|(key, value)| key.object == root.addr
        && key.item_type == format::DEAD
        && key.offset == 4
        && value.is_empty()));
    assert!(
        space::reference(&DeviceStore(&device), sb.space_root, root.addr, 4)
            .unwrap()
            .is_none()
    );
    assert_eq!(tree_bytes(&DeviceStore(&device), root), frozen);
    // IDs remain monotonic after deletion and after reopening.
    let mut live = Files::new(DeviceStore(&device), &sb, 1, true).unwrap();
    assert_eq!(
        live.snapshot(b"replacement", false).unwrap(),
        sb.next_root_id
    );
    assert!(sb.next_root_id > clone_id);
    commit(&mut live, &device, &mut mount);
    assert!(start.elapsed().as_secs_f64() < 60.0);
}
