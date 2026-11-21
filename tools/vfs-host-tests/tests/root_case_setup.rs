//! Root-suite reclamation oracles through production VFS, E24 ledger 465.
use crate::{
    attrs::InodeAttrs,
    block::cache::BufferCache,
    breenfs::vfs::BreenFs,
    read_conformance::{Disk, NoWait},
    vfs::*,
};
use std::sync::{Arc, Mutex, OnceLock};

// Same populated FS/SPACE geometry as 558(x1), with the design reserve.
pub(crate) fn populated_image() -> Vec<u8> {
    use crate::breenfs::{commit::{Committer, DeviceStore}, format::{Superblock, Time}, ops::Files};
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    IMAGE.get_or_init(|| {
        let disk = Disk { bytes: Mutex::new(vec![0; 80000 * 4096]), writable: true };
        let sb = Superblock { fs_uuid: [0x59;16], generation: 1, slot: 1,
            total_blocks: 80000, global_reserve: 4096, next_root_id: 2,
            dirhash_key: [0x31;16], ..Superblock::default() };
        let at = Time { seconds: 1, nanos: 0 };
        let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
        files.mkdir(2, b"tmp", 0o1777, at).unwrap();
        for i in 0..994 {
            let ino = files.create(2, format!("populated-{i}").as_bytes(), 0o644, at).unwrap();
            files.write(&disk, ino, 0, &vec![0x61; 64 * 4096], at).unwrap();
        }
        files.fsync(&disk, &mut Committer::format(sb)).unwrap();
        drop(files);
        disk.bytes.into_inner().unwrap()
    }).clone()
}

// Decode durable FREE and PENDING lengths independently.
pub(crate) fn raw(disk: &Disk) -> (u64, u64) {
    use crate::breenfs::{commit::DeviceStore, format::{self, Pending, Free}, recover};
    let sb = recover::recover(disk, false).unwrap();
    let items = recover::space_items(&DeviceStore(disk), &sb).unwrap();
    let mut totals = (0, 0);
    for (key, bytes) in items {
        if key.item_type == format::FREE { totals.0 += Free::decode(&bytes).unwrap().length; }
        if key.item_type == format::PENDING { totals.1 += Pending::decode(&bytes).unwrap().length; }
    }
    assert_eq!(totals.0, sb.free_blocks);
    totals
}

fn create(fs: &dyn FileSystem, name: &[u8]) -> Arc<VNode> {
    fs.open(fs.create(2, name, NewNode::Regular {
        attrs: InodeAttrs::new(0o600, 0, 0),
    }).unwrap()).unwrap()
}

fn write(fs: &dyn FileSystem, node: &VNode, offset: u64, bytes: &[u8]) {
    assert_eq!(fs.write(node, WriteReq {
        at: WriteAt::Offset(offset), limit: u64::MAX, clear_setid: false,
    }, bytes).unwrap().bytes, bytes.len());
}

fn commits(fs: &dyn FileSystem, marker: &VNode, disk: &Disk) {
    for byte in 0..3 {
        let before = raw(disk);
        report_raw(fs, before);
        fs.resize(marker, 0, false, &|_| Ok(())).unwrap();
        write(fs, marker, 0, &[byte]);
        fs.fsync(marker, false).unwrap();
        let after = raw(disk);
        report_raw(fs, after);
        println!("MARKER {byte} FREE_PENDING {before:?}->{after:?}");
    }
}

fn report_raw(fs: &dyn FileSystem, (free, pending): (u64, u64)) {
    let mut report = String::new();
    fs.report(Report::State, "root", &mut report).unwrap();
    assert!(report.contains(&format!("free_blocks={free} pending_blocks={pending}")), "raw accounting differs: {report}");
}

fn bytes(fs: &dyn FileSystem, node: &VNode, expected: &[u8]) {
    let mut actual = vec![0; expected.len() + 1];
    assert_eq!(fs.read(node, 0, &mut actual).unwrap(), expected.len());
    assert_eq!(&actual[..expected.len()], expected);
}

fn with_root(populated: bool, run: impl FnOnce(&dyn FileSystem, &VNode, &VNode, &Disk)) {
    let disk = Arc::new(Disk { bytes: Mutex::new(if populated { populated_image() } else { crate::model::image() }), writable: true });
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 2415, 1).unwrap();
    if populated {
        let stats = fs.statfs().unwrap();
        println!("GEOMETRY free={} reserve=4096", stats.free_blocks);
        assert!((16000..=16400).contains(&stats.free_blocks));
        assert_eq!(stats.available_blocks, stats.free_blocks - 4096);
    }
    let file = create(fs.as_ref(), b"data");
    let marker = create(fs.as_ref(), b"marker");
    run(fs.as_ref(), &file, &marker, &disk);
    if populated {
        report_raw(fs.as_ref(), raw(&disk));
    }
}

#[test]
fn large_shrink_and_regrowth_return_truncated_data_blocks() {
    with_root(true, |fs, file, marker, disk| {
        write(fs, file, 0, &vec![0x5a; 129 * 4096]);
        commits(fs, marker, disk);
        let before = fs.statfs().unwrap().free_blocks;
        fs.resize(file, 3, false, &|_| Ok(())).unwrap();
        commits(fs, marker, disk);
        let after = fs.statfs().unwrap().free_blocks;
        println!("large shrink FREE {before}->{after}");
        assert!(after >= before + 128);
        bytes(fs, file, &[0x5a; 3]);
        fs.resize(file, 129 * 4096, false, &|_| Ok(())).unwrap();
        let mut expected = vec![0; 129 * 4096];
        expected[..3].fill(0x5a);
        bytes(fs, file, &expected);
        write(fs, file, (expected.len() - 1) as u64, b"Z");
        *expected.last_mut().unwrap() = b'Z';
        bytes(fs, file, &expected);
        commits(fs, marker, disk);
        let before = fs.statfs().unwrap().free_blocks;
        fs.resize(file, 0, false, &|_| Ok(())).unwrap();
        commits(fs, marker, disk);
        let after = fs.statfs().unwrap().free_blocks;
        println!("regrowth zero FREE {before}->{after}");
        assert!(after >= before + 2);
        bytes(fs, file, b"");
    });
}

#[test]
fn open_truncate_returns_129_data_blocks() {
    with_root(true, |fs, file, marker, disk| {
        write(fs, file, 0, &vec![0x5a; 129 * 4096]);
        commits(fs, marker, disk);
        let before = fs.statfs().unwrap().free_blocks;
        let opened = fs.open(file.ino).unwrap();
        // O_TRUNC dispatches resize through this VFS boundary.
        fs.resize(&opened, 0, false, &|_| Ok(())).unwrap();
        commits(fs, marker, disk);
        let after = fs.statfs().unwrap().free_blocks;
        println!("open truncate FREE {before}->{after}");
        assert!(after >= before + 129);
        bytes(fs, &opened, b"");
        write(fs, &opened, 0, b"fresh");
        bytes(fs, &opened, b"fresh");
    });
}

#[test]
fn statfs_allocation_returns_two_data_blocks() {
    with_root(false, |fs, file, marker, disk| {
        write(fs, file, 0, &[0x5a; 8192]);
        commits(fs, marker, disk);
        let before = fs.statfs().unwrap();
        assert_eq!(before.available_blocks, before.free_blocks - 4096);
        assert_eq!((before.inodes, before.free_inodes), (0, 0));
        assert_eq!(fs.getattr(file.ino).unwrap().blocks, 16);
        bytes(fs, file, &[0x5a; 8192]);
        fs.resize(file, 0, false, &|_| Ok(())).unwrap();
        commits(fs, marker, disk);
        let after = fs.statfs().unwrap();
        println!("statfs allocation FREE {}->{}", before.free_blocks, after.free_blocks);
        assert!(after.free_blocks >= before.free_blocks + 2);
        assert_eq!(after.available_blocks, after.free_blocks - 4096);
        bytes(fs, file, b"");
    });
}
