//! Guest snapshot case geometry through the production VFS (E24 ledger 450).
use crate::{
    attrs::InodeAttrs,
    block::cache::BufferCache,
    breenfs::{
        abi::*,
        commit::DeviceStore,
        commit_policy::COMMIT_INTERVAL_NS,
        format::{self, Pending, Ref},
        recover,
        vfs::{measurement, BreenFs},
    },
    read_conformance::{Disk, NoWait},
    vfs::*,
};
use std::sync::{Arc, Mutex};

fn create(fs: &dyn FileSystem, dir: u64, name: &[u8], directory: bool) -> Arc<VNode> {
    let attrs = InodeAttrs::new(if directory { 0o755 } else { 0o644 }, 0, 0);
    let node = if directory { NewNode::Directory { attrs } } else { NewNode::Regular { attrs } };
    fs.open(fs.create(dir, name, node).unwrap()).unwrap()
}

fn write_synced(fs: &dyn FileSystem, file: &VNode, bytes: &[u8]) {
    // File::create truncates even an existing file in the guest routine.
    fs.resize(file, 0, false, &|_| Ok(())).unwrap();
    assert_eq!(fs.write(file, WriteReq {
        at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false,
    }, bytes).unwrap().bytes, bytes.len());
    fs.fsync(file, false).unwrap();
}

fn snapshot(fs: &dyn FileSystem, dir: &VNode, op: u64) {
    let mut request = SnapshotRequest {
        flags: if op == SNAPSHOT_CREATE { SNAPSHOT_READ_ONLY } else { 0 },
        name_len: 6, name: [0; 256],
    };
    request.name[..6].copy_from_slice(b"frozen");
    fs.ioctl(dir, op, &request as *const _ as u64).unwrap();
}

fn dead(fs: &dyn FileSystem, dir: &VNode) -> u64 {
    let mut status = SnapshotStatus::default();
    fs.ioctl(dir, SNAPSHOT_STATUS, &mut status as *mut _ as u64).unwrap();
    status.dead_remaining
}

fn wait(fs: &BreenFs<NoWait>, dir: &VNode) {
    for _ in 0..128 {
        if dead(fs, dir) == 0 { return; }
        measurement::service_at(fs, COMMIT_INTERVAL_NS).unwrap();
    }
    panic!("snapshot wait left DEAD work");
}

#[derive(Debug)]
struct Counts {
    free: u64,
    data: u64,
    nodes: u64,
    pending: u64,
    dead: u64,
}

fn counts(fs: &dyn FileSystem, disk: &Disk) -> Counts {
    let sb = recover::recover(disk, false).unwrap();
    let mut counts = Counts { free: fs.statfs().unwrap().free_blocks,
        data: 0, nodes: 0, pending: 0, dead: 0 };
    for (key, bytes) in recover::space_items(&DeviceStore(disk), &sb).unwrap() {
        match key.item_type {
            format::REF => {
                if Ref::decode(&bytes).unwrap().kind == 3 { counts.data += key.offset; }
                else { counts.nodes += key.offset; }
            }
            format::PENDING => counts.pending += Pending::decode(&bytes).unwrap().length,
            format::DEAD => counts.dead += 1,
            _ => {}
        }
    }
    counts
}

fn marker_commits(fs: &dyn FileSystem, dir: &VNode) {
    let marker = match fs.lookup(dir.ino, b"commit-marker") {
        Ok(ino) => fs.open(ino).unwrap(),
        Err(error) => {
            assert_eq!(error, crate::errno::Errno(crate::syscall_errno::ENOENT));
            create(fs, dir.ino, b"commit-marker", false)
        }
    };
    for byte in 0..3 { write_synced(fs, &marker, &[byte]); }
}

#[test]
fn equalized_marker_commits_return_all_1024_snapshot_blocks() {
    const BYTES: usize = 4 * 1024 * 1024;
    let disk = Arc::new(Disk { bytes: Mutex::new(crate::model::image()), writable: true });
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 2414, 1).unwrap();
    let geometry = fs.statfs().unwrap();
    assert_eq!((geometry.blocks, geometry.block_size), (16384, 4096));
    let sb = recover::recover(disk.as_ref(), false).unwrap();
    assert_eq!(sb.global_reserve, 4096);
    let dir = create(fs.as_ref(), 2, b"case", true);
    let file = create(fs.as_ref(), dir.ino, b"data", false);
    write_synced(fs.as_ref(), &file, &vec![0x51; BYTES]);
    assert_eq!(fs.getattr(file.ino).unwrap().blocks / 8, 1024);
    snapshot(fs.as_ref(), &dir, SNAPSHOT_CREATE);
    write_synced(fs.as_ref(), &file, &vec![0xa7; BYTES]);
    marker_commits(fs.as_ref(), &dir);
    let before = counts(fs.as_ref(), &disk);
    snapshot(fs.as_ref(), &dir, SNAPSHOT_DELETE);
    wait(&fs, &dir);
    marker_commits(fs.as_ref(), &dir);
    let after = counts(fs.as_ref(), &disk);
    println!("equalized before={before:?} after={after:?} delta={}", after.free as i64 - before.free as i64);
    assert!(after.free >= before.free + 1024, "before={before:?}; after={after:?}");
    let mut bytes = vec![0; BYTES];
    assert_eq!(fs.read(&file, 0, &mut bytes).unwrap(), BYTES);
    assert_eq!(bytes, vec![0xa7; BYTES]);
}
