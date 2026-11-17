//! Scrub controls through the production adapter; worker steps run synchronously.
use crate::{
    adapter::Ext2Adapter, block::cache::BufferCache,
    breenfs::{abi::*, commit::{Committer, DeviceStore}, format::{self, Superblock, Time, BLOCK_SIZE},
        ops::{self, Files}, recover, vfs::{BreenFs, measurement}},
    errno::Errno, host_mounts::Fixture, read_conformance::{Disk, NoWait},
    syscall_errno::*, vfs::*,
};
use std::sync::{Arc, Mutex};

fn image() -> (Arc<Disk>, u64) {
    let disk = Arc::new(Disk { bytes: Mutex::new(vec![0; 64 * 1024 * 1024]), writable: true });
    let sb = Superblock { fs_uuid: [0x13; 16], generation: 1, slot: 1,
        total_blocks: 16384, global_reserve: 4096, next_root_id: 2, ..Superblock::default() };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let ino = files.create(2, b"target", 0o644, Time::default()).unwrap();
    files.write(disk.as_ref(), ino, 0, &vec![0x5a; 32 * BLOCK_SIZE], Time::default()).unwrap();
    let durable = files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    let roots = recover::space_items(&DeviceStore(disk.as_ref()), &durable).unwrap();
    let root = format::Root::decode(&roots.into_iter().find(|(k, _)| k.item_type == format::ROOT).unwrap().1).unwrap();
    let items = ops::items(&DeviceStore(disk.as_ref()), root.root).unwrap();
    let extent = format::Extent::decode(&items.into_iter().find(|(k, _)| k.object == ino && k.item_type == format::EXTENT).unwrap().1).unwrap();
    let bad = extent.phys_start + 3;
    disk.bytes.lock().unwrap()[bad as usize * BLOCK_SIZE + 19] ^= 1;
    (disk, bad)
}
fn status(fs: &dyn FileSystem, node: &VNode) -> ScrubStatus {
    let mut status = ScrubStatus::default();
    assert_eq!(fs.ioctl(node, SCRUB_STATUS, &mut status as *mut _ as u64), Ok(0));
    status
}
fn finish(fs: &BreenFs<NoWait>) {
    for _ in 0..128 {
        if !measurement::scrub_step(fs, format::NODE_SIZE).unwrap() { return; }
    }
    panic!("scrub did not complete bounded fixture");
}
#[test]
fn scrub_ioctl_reports_corruption_and_records_only_writable_completions() {
    let (disk, bad) = image();
    for writable in [false, true] {
        let cache = Arc::new(BufferCache::new(NoWait));
        let fs = if writable { BreenFs::mount_writable(disk.clone(), cache, 113, 1) }
            else { BreenFs::mount(disk.clone(), cache, 113, 1) }.unwrap();
        fs.init_service().unwrap();
        let node = fs.open(2).unwrap();
        let before = disk.bytes.lock().unwrap().clone();
        assert_eq!(fs.ioctl(&node, SCRUB_START, 0), Ok(0));
        assert_eq!(fs.ioctl(&node, SCRUB_START, 0), Err(Errno(EBUSY)));
        finish(&fs);
        let done = status(fs.as_ref(), &node);
        assert_eq!(done.state, SCRUB_DONE);
        assert!(done.bytes_scanned >= (32 * BLOCK_SIZE) as u64);
        assert_eq!(done.errors, 1);
        assert_eq!(done.bad_count, 1);
        assert_eq!(done.bad[0], BadBlock { block: bad, kind: BAD_DATA, reserved: 0 });
        assert_eq!(done.last_scrub_generation, 0, "status must report durable fields");
        fs.sync().unwrap();
        let recorded = status(fs.as_ref(), &node);
        let sb = recover::recover(disk.as_ref(), false).unwrap();
        if writable {
            assert!(recorded.last_scrub_generation > 0);
            assert_eq!(recorded.last_scrub_errors, 1);
            assert_eq!(sb.last_scrub_generation, recorded.last_scrub_generation);
            let mut report = String::new();
            fs.report(Report::State, "/scrub", &mut report).unwrap();
            assert!(report.contains(&format!("last_scrub_generation={} last_scrub_errors=1", sb.last_scrub_generation)));
        } else {
            assert_eq!(recorded.last_scrub_generation, 0);
            assert_eq!(*disk.bytes.lock().unwrap(), before);
        }
        drop(node);
        fs.shutdown("scrub-complete", u64::MAX).unwrap();
    }
}
#[test]
fn scrub_cancel_and_unmount_stop_between_steps_and_ext2_refuses_controls() {
    let (disk, _) = image();
    let fs = BreenFs::mount(disk.clone(), Arc::new(BufferCache::new(NoWait)), 114, 1).unwrap();
    let node = fs.open(2).unwrap();
    fs.ioctl(&node, SCRUB_START, 0).unwrap();
    assert!(measurement::scrub_step(&fs, format::NODE_SIZE).unwrap());
    let running = status(fs.as_ref(), &node);
    assert_eq!(running.state, SCRUB_RUNNING);
    assert!(running.bytes_scanned > 0);
    fs.ioctl(&node, SCRUB_CANCEL, 0).unwrap();
    assert!(!measurement::scrub_step(&fs, format::NODE_SIZE).unwrap());
    assert_eq!(status(fs.as_ref(), &node).state, SCRUB_CANCELLED);
    fs.ioctl(&node, SCRUB_START, 0).unwrap();
    assert!(measurement::scrub_step(&fs, format::NODE_SIZE).unwrap());
    fs.shutdown("scrub-in-progress", u64::MAX).unwrap();
    let stopped = status(fs.as_ref(), &node);
    assert_eq!(stopped.state, SCRUB_CANCELLED);
    assert!(!measurement::scrub_step(&fs, format::NODE_SIZE).unwrap());
    assert_eq!(status(fs.as_ref(), &node), stopped);
    assert_eq!(fs.ioctl(&node, SCRUB_START, 0), Err(Errno(EBUSY)));
    let fixture = Fixture::new();
    let ext2 = Ext2Adapter::new(fixture.pin());
    let dir = ext2.open(ext2.root().unwrap()).unwrap();
    for request in [SCRUB_START, SCRUB_CANCEL, SCRUB_STATUS] {
        assert_eq!(ext2.ioctl(&dir, request, 0), Err(Errno(ENOTTY)));
    }
}
