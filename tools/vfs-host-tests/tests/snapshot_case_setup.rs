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

fn remove(fs: &dyn FileSystem, dir: u64, name: &[u8], directory: bool) {
    let ino = fs.lookup(dir, name).unwrap();
    fs.remove(dir, name, Remove {
        directory, expected: ino, owner: fs.getattr(ino).unwrap().attrs.uid,
        parent: fs.getattr(dir).unwrap().attrs,
    }).unwrap();
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

fn marker_commit(fs: &dyn FileSystem, dir: &VNode, byte: u8) {
    let marker = match fs.lookup(dir.ino, b"commit-marker") {
        Ok(ino) => fs.open(ino).unwrap(),
        Err(error) => {
            assert_eq!(error, crate::errno::Errno(crate::syscall_errno::ENOENT));
            create(fs, dir.ino, b"commit-marker", false)
        }
    };
    write_synced(fs, &marker, &[byte]);
}

fn marker_commits(fs: &dyn FileSystem, dir: &VNode) {
    for byte in 0..3 { marker_commit(fs, dir, byte); }
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

#[test]
fn inline_symlinks_leave_more_than_16_dead_nodes_during_resumed_deletion() {
    const LINKS: usize = 68;
    const BYTES: usize = 2 * 1024 * 1024;
    let disk = Arc::new(Disk { bytes: Mutex::new(crate::model::image()), writable: true });
    let mount = || BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 2415, 1).unwrap();
    let fs = mount();
    let dir = create(fs.as_ref(), 2, b"case", true);
    let files = create(fs.as_ref(), dir.ino, b"files", true);
    // INLINE holds the long target in an FS leaf, with no data write per link.
    let target = vec![b'x'; 4095];
    for index in 0..LINKS {
        fs.create(files.ino, format!("link-{index:02}").as_bytes(), NewNode::Symlink {
            attrs: InodeAttrs::new(0o777, 0, 0), target: &target,
        }).unwrap();
        if (index + 1) % 16 == 0 { fs.fsync(&files, false).unwrap(); }
    }
    let file = create(fs.as_ref(), files.ino, b"data", false);
    write_synced(fs.as_ref(), &file, &vec![0x63; BYTES]);
    snapshot(fs.as_ref(), &dir, SNAPSHOT_CREATE);
    // Diverge in live first: the old leaves and data belong only to frozen.
    drop(file);
    for index in 0..LINKS {
        remove(fs.as_ref(), files.ino, format!("link-{index:02}").as_bytes(), false);
    }
    remove(fs.as_ref(), files.ino, b"data", false);
    drop(files);
    remove(fs.as_ref(), dir.ino, b"files", true);
    fs.fsync(&dir, false).unwrap();
    // Count the setup's mutating VFS calls, including write_synced's truncate;
    // opens/status are not mutations.
    let creates = LINKS + 3;
    let removes = LINKS + 2;
    let fsyncs = LINKS / 16 + 2;
    println!("setup creates={creates} truncates=1 writes=1 fsyncs={fsyncs} snapshots=1 removes={removes} total={}",
        creates + removes + fsyncs + 3);
    let before = fs.statfs().unwrap().free_blocks;
    snapshot(fs.as_ref(), &dir, SNAPSHOT_DELETE);
    let queued = dead(fs.as_ref(), &dir);
    assert!(queued > 0);
    // Deletion queues the tree root first; shutdown must not consume it.
    fs.shutdown("snapshot-case", u64::MAX).unwrap();
    let dir_ino = dir.ino;
    drop((dir, fs));
    let fs = mount();
    let dir = fs.open(dir_ino).unwrap();
    let recovered = dead(fs.as_ref(), &dir);
    assert!(recovered > 0);
    assert_eq!(recovered, queued);
    marker_commit(fs.as_ref(), &dir, 0);
    let frontier = dead(fs.as_ref(), &dir);
    println!("delete queued={queued} remounted={recovered} after_first_resumed_commit={frontier}");
    assert!(frontier > 16, "setup did not leave more than a reaper budget of exclusive DEAD nodes");
    wait(&fs, &dir);
    assert_eq!(dead(fs.as_ref(), &dir), 0);
    for byte in 1..3 { marker_commit(fs.as_ref(), &dir, byte); }
    let after = fs.statfs().unwrap().free_blocks;
    println!("resumed free_before={before} free_after={after} delta={}", after as i64 - before as i64);
    assert!(after >= before + (BYTES / 4096) as u64);
    assert_eq!(fs.lookup(dir.ino, b"files"), Err(crate::errno::Errno(crate::syscall_errno::ENOENT)));
}
