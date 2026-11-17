//! Root-suite reclamation oracles through production VFS, E24 ledger 465.
use crate::{
    attrs::InodeAttrs,
    block::cache::BufferCache,
    breenfs::vfs::BreenFs,
    read_conformance::{Disk, NoWait},
    vfs::*,
};
use std::sync::{Arc, Mutex};

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

fn commits(fs: &dyn FileSystem, marker: &VNode) {
    for byte in 0..3 {
        fs.resize(marker, 0, false, &|_| Ok(())).unwrap();
        write(fs, marker, 0, &[byte]);
        fs.fsync(marker, false).unwrap();
    }
}

fn bytes(fs: &dyn FileSystem, node: &VNode, expected: &[u8]) {
    let mut actual = vec![0; expected.len() + 1];
    assert_eq!(fs.read(node, 0, &mut actual).unwrap(), expected.len());
    assert_eq!(&actual[..expected.len()], expected);
}

fn with_root(run: impl FnOnce(&dyn FileSystem, &VNode, &VNode)) {
    let disk = Arc::new(Disk { bytes: Mutex::new(crate::model::image()), writable: true });
    let fs = BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 2415, 1).unwrap();
    let file = create(fs.as_ref(), b"data");
    let marker = create(fs.as_ref(), b"marker");
    run(fs.as_ref(), &file, &marker);
}

#[test]
fn large_shrink_and_regrowth_return_truncated_data_blocks() {
    with_root(|fs, file, marker| {
        write(fs, file, 0, &vec![0x5a; 129 * 4096]);
        commits(fs, marker);
        let before = fs.statfs().unwrap().free_blocks;
        fs.resize(file, 3, false, &|_| Ok(())).unwrap();
        commits(fs, marker);
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
        commits(fs, marker);
        let before = fs.statfs().unwrap().free_blocks;
        fs.resize(file, 0, false, &|_| Ok(())).unwrap();
        commits(fs, marker);
        let after = fs.statfs().unwrap().free_blocks;
        println!("regrowth zero FREE {before}->{after}");
        assert!(after >= before + 2);
        bytes(fs, file, b"");
    });
}

#[test]
fn open_truncate_returns_129_data_blocks() {
    with_root(|fs, file, marker| {
        write(fs, file, 0, &vec![0x5a; 129 * 4096]);
        commits(fs, marker);
        let before = fs.statfs().unwrap().free_blocks;
        let opened = fs.open(file.ino).unwrap();
        // O_TRUNC dispatches resize through this VFS boundary.
        fs.resize(&opened, 0, false, &|_| Ok(())).unwrap();
        commits(fs, marker);
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
    with_root(|fs, file, marker| {
        write(fs, file, 0, &[0x5a; 8192]);
        commits(fs, marker);
        let before = fs.statfs().unwrap();
        assert_eq!(before.available_blocks, before.free_blocks - 4096);
        assert_eq!((before.inodes, before.free_inodes), (0, 0));
        assert_eq!(fs.getattr(file.ino).unwrap().blocks, 16);
        bytes(fs, file, &[0x5a; 8192]);
        fs.resize(file, 0, false, &|_| Ok(())).unwrap();
        commits(fs, marker);
        let after = fs.statfs().unwrap();
        println!("statfs allocation FREE {}->{}", before.free_blocks, after.free_blocks);
        assert!(after.free_blocks >= before.free_blocks + 2);
        assert_eq!(after.available_blocks, after.free_blocks - 4096);
        bytes(fs, file, b"");
    });
}
