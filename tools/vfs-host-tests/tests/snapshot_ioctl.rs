//! Snapshot durability and bounded worker reclamation through the kernel adapter.
use crate::{
    attrs::InodeAttrs, block::cache::BufferCache,
    breenfs::{abi::*, commit::{Committer, DeviceStore}, commit_policy::COMMIT_INTERVAL_NS,
        format::{self, Superblock, Time}, ops::Files, recover, vfs::{BreenFs, measurement}},
    errno::Errno, read_conformance::{Disk, NoWait}, syscall_errno::*,
    trait_adapter::{Device, TraitAdapter}, vfs::*,
};
use breenfs_host_tests::recording::RecordingDevice;
use std::sync::{Arc, Mutex};

fn mount(device: Arc<Device>) -> Arc<BreenFs<NoWait>> {
    BreenFs::mount_writable(device, Arc::new(BufferCache::new(NoWait)), 149, 1).unwrap()
}
fn named(device: Arc<Device>, name: &[u8], writable: bool) -> Result<Arc<BreenFs<NoWait>>, Errno> {
    let data = [b"root=".as_slice(), name].concat();
    BreenFs::mount_named(device, Arc::new(BufferCache::new(NoWait)), 150, Some(&data), writable)
}
fn request(name: &[u8], flags: u32) -> SnapshotRequest {
    let mut req = SnapshotRequest { flags, name_len: name.len() as u32, name: [0; 256] };
    req.name[..name.len()].copy_from_slice(name);
    req
}
fn control(fs: &dyn FileSystem, node: &VNode, op: u64, name: &[u8], flags: u32) -> Result<u64, Errno> {
    fs.ioctl(node, op, &request(name, flags) as *const _ as u64)
}
fn status(fs: &dyn FileSystem, node: &VNode) -> SnapshotStatus {
    let mut status = SnapshotStatus::default();
    assert_eq!(fs.ioctl(node, SNAPSHOT_STATUS, &mut status as *mut _ as u64), Ok(0));
    status
}
fn cold(recorder: &RecordingDevice) -> Arc<Device> {
    Arc::new(Device(Arc::new(RecordingDevice::new(recorder.durable_image()).unwrap())))
}
fn write(fs: &dyn FileSystem, node: &VNode, bytes: &[u8]) {
    fs.write(node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false }, bytes).unwrap();
}

#[test]
fn create_and_clone_are_fsync_class_and_delete_refuses_live_roots() {
    let recorder = Arc::new(RecordingDevice::new(crate::model::image()).unwrap());
    let device = Arc::new(Device(recorder.clone()));
    let fs = mount(device.clone());
    let dir = fs.open(2).unwrap();
    let ino = fs.create(2, b"file", NewNode::Regular { attrs: InodeAttrs::new(0o644, 0, 0) }).unwrap();
    let file = fs.open(ino).unwrap();
    write(fs.as_ref(), &file, b"before");
    assert!(fs.dirty_bytes() > 0);
    control(fs.as_ref(), &dir, SNAPSHOT_CREATE, b"frozen", SNAPSHOT_READ_ONLY).unwrap();
    assert_eq!(fs.dirty_bytes(), 0);
    // A separate stopped image must contain both the data and the new ROOT.
    let frozen = named(cold(&recorder), b"frozen", false).unwrap();
    let mut bytes = [0; 6];
    frozen.read(&frozen.open(ino).unwrap(), 0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"before");
    assert_eq!(status(frozen.as_ref(), &frozen.open(2).unwrap()).flags, SNAPSHOT_READ_ONLY);
    control(fs.as_ref(), &dir, SNAPSHOT_CREATE, b"clone", 0).unwrap();
    write(fs.as_ref(), &file, b"after!");
    fs.sync().unwrap();
    assert_eq!(control(fs.as_ref(), &dir, SNAPSHOT_DELETE, b"default", 0), Err(Errno(EBUSY)));
    let reader = named(device.clone(), b"frozen", false).unwrap();
    let held = reader.open(ino).unwrap();
    assert_eq!(control(fs.as_ref(), &dir, SNAPSHOT_DELETE, b"frozen", 0), Err(Errno(EBUSY)));
    reader.shutdown("snapshot-reader", u64::MAX).unwrap();
    drop(reader);
    assert_eq!(control(fs.as_ref(), &dir, SNAPSHOT_DELETE, b"frozen", 0), Err(Errno(EBUSY)));
    drop(held);
    control(fs.as_ref(), &dir, SNAPSHOT_DELETE, b"frozen", 0).unwrap();
    assert_eq!(named(cold(&recorder), b"frozen", false).err(), Some(Errno(ENOENT)));
    assert_eq!(status(fs.as_ref(), &dir).roots, 2);
    drop((dir, file, fs));
    let clone = named(device.clone(), b"clone", true).unwrap();
    let file = clone.open(ino).unwrap();
    clone.read(&file, 0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"before");
    write(clone.as_ref(), &file, b"clone!");
    clone.sync().unwrap();
    let original = named(cold(&recorder), b"default", false).unwrap();
    original.read(&original.open(ino).unwrap(), 0, &mut bytes).unwrap();
    assert_eq!(&bytes, b"after!");
}

#[test]
fn reaper_waits_for_policy_then_drains_across_commits_and_never_reaps_at_shutdown() {
    let disk = Disk { bytes: Mutex::new(vec![0; 64 * 1024 * 1024]), writable: true };
    let sb = Superblock { fs_uuid: [0xf4; 16], generation: 1, slot: 1, total_blocks: 16384,
        global_reserve: 4096, next_root_id: 2, ..Superblock::default() };
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
    let sb = files.fsync(&disk, &mut committer).unwrap();
    let mut files = Files::new(DeviceStore(&disk), &sb, 1, true).unwrap();
    let trash = files.snapshot(b"trash", false).unwrap();
    let sb = files.fsync(&disk, &mut committer).unwrap();
    let mut files = Files::new(DeviceStore(&disk), &sb, trash, true).unwrap();
    for n in 0..1200 {
        files.create(2, format!("file-{n:04}").as_bytes(), 0o644, Time::default()).unwrap();
    }
    files.fsync(&disk, &mut committer).unwrap();
    drop(files);
    let recorder = Arc::new(RecordingDevice::new(disk.bytes.into_inner().unwrap()).unwrap());
    let device = Arc::new(Device(recorder.clone()));
    let fs = mount(device.clone());
    fs.init_service().unwrap();
    let dir = fs.open(2).unwrap();
    control(fs.as_ref(), &dir, SNAPSHOT_DELETE, b"trash", 0).unwrap();
    let deleted = status(fs.as_ref(), &dir);
    assert!(deleted.dead_remaining > 0, "delete must retain the DEAD frontier");
    assert_eq!(named(cold(&recorder), b"trash", false).err(), Some(Errno(ENOENT)));
    assert!(!measurement::service_at(&fs, 0).unwrap().0, "delete must wait for policy");
    // Even a due final shutdown commit must leave the frontier intact.
    fs.shutdown("snapshot-reaper", u64::MAX).unwrap();
    assert_eq!(status(fs.as_ref(), &dir).dead_remaining, deleted.dead_remaining);
    drop((dir, fs));
    let fs = mount(device);
    let dir = fs.open(2).unwrap();
    assert_eq!(status(fs.as_ref(), &dir).dead_remaining, deleted.dead_remaining);
    assert!(!measurement::service_at(&fs, 0).unwrap().0);
    let mut generations = 0;
    for _ in 0..128 {
        if status(fs.as_ref(), &dir).dead_remaining == 0 { break; }
        assert!(measurement::service_at(&fs, COMMIT_INTERVAL_NS).unwrap().0);
        generations += 1;
    }
    assert!(generations > 1, "fixture must span the per-commit reaper budget");
    assert_eq!(status(fs.as_ref(), &dir).dead_remaining, 0);
    let sb = recover::recover(cold(&recorder).as_ref(), false).unwrap();
    assert!(!recover::space_items(&DeviceStore(cold(&recorder).as_ref()), &sb).unwrap()
        .iter().any(|(k, _)| k.item_type == format::DEAD));
}

#[test]
fn snapshot_mutation_gate_requires_root_but_status_does_not() {
    let source = include_str!("../../../kernel/src/syscall/ioctl.rs");
    let gate = source.split("if let Some(node) = node {").nth(1).unwrap()
        .split("return match node.filesystem.ioctl").next().unwrap();
    assert!(gate.contains("SCRUB_START | SCRUB_CANCEL | SNAPSHOT_CREATE | SNAPSHOT_DELETE"));
    assert!(gate.contains("sys_geteuid(), SyscallResult::Ok(0)"));
    assert!(gate.contains("return SyscallResult::Err(EPERM)"));
    assert!(!gate.contains("SNAPSHOT_STATUS"));
}

#[test]
fn snapshot_stream_compares_named_roots_through_the_vfs_adapter() {
    use breenfs_host_tests::{compare, model::Model, stream::{snapshot_seed, Stream}, *};
    let device = Arc::new(RecordingDevice::new(crate::model::image()).unwrap());
    let mut fs = TraitAdapter::new(device);
    let mut model = Model::new();
    for (index, op) in Stream::new(snapshot_seed(0x2414)).take(32).enumerate() {
        assert_eq!(fs.execute(&op), model.execute(&op), "op={index}: {op:?}");
        if op.action == Action::Remount || index == 17 {
            assert!(compare(&model, &fs).unwrap().is_empty(), "op={index}: {op:?}");
        }
        if index == 17 {
            assert!(fs.roots().unwrap().len() >= 4);
            for driver in [&mut fs as &mut dyn Fs, &mut model as &mut dyn Fs] {
                driver.select_root(b"clone").unwrap();
                driver.execute(&Operation { at: op.at, action: Action::Write {
                    ino: 16, offset: 0, bytes: b"clone diverges".to_vec(),
                }}).unwrap();
                driver.select_root(b"default").unwrap();
            }
            assert!(compare(&model, &fs).unwrap().is_empty());
        }
    }
}
