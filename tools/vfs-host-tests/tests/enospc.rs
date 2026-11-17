//! Data admission on the model's 64 MiB image, through the production VFS.
use crate::{
    attrs::InodeAttrs,
    block::cache::BufferCache,
    breenfs::vfs::BreenFs,
    errno::Errno,
    model::image,
    read_conformance::{Disk, NoWait},
    syscall_errno::ENOSPC,
    vfs::*,
};
use std::sync::{Arc, Mutex};

#[path = "../../../libs/libbreenix/src/crypto/sha256.rs"]
mod sha256;

fn mount(disk: Arc<Disk>) -> Arc<BreenFs<NoWait>> {
    BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap()
}
fn fixture() -> (Arc<Disk>, Arc<BreenFs<NoWait>>, Arc<VNode>) {
    let disk = Arc::new(Disk { bytes: Mutex::new(image()), writable: true });
    let fs = mount(disk.clone());
    let ino = fs.create(2, b"data", NewNode::Regular {
        attrs: InodeAttrs::new(0o640, 1000, 1000),
    }).unwrap();
    let node = fs.open(ino).unwrap();
    fs.fsync(&node, false).unwrap();
    (disk, fs, node)
}
fn write(fs: &dyn FileSystem, node: &VNode, offset: u64, bytes: &[u8]) -> Result<WriteResult, WriteError> {
    fs.write(node, WriteReq { at: WriteAt::Offset(offset), limit: u64::MAX, clear_setid: false }, bytes)
}
// This fixture has one regular file. Include the root's complete name list,
// metadata and file digest, so refusal cannot hide a size or namespace edit.
fn manifest(fs: &dyn FileSystem, node: &VNode) -> (NodeAttrs, Vec<(Vec<u8>, u64)>, NodeAttrs, [u8; 32]) {
    let mut names = Vec::new();
    fs.readdir(2, 0, &mut |entry, _| { names.push((entry.name, entry.ino)); Ok(true) }).unwrap();
    let attrs = fs.getattr(node.ino).unwrap();
    let mut bytes = vec![0; attrs.size as usize];
    assert_eq!(fs.read(node, 0, &mut bytes).unwrap(), bytes.len());
    (fs.getattr(2).unwrap(), names, attrs, sha256::sha256(&bytes))
}

#[test]
fn unsynced_data_reserves_bavail_without_changing_bfree() {
    let (_disk, fs, node) = fixture();
    let before = fs.statfs().unwrap();
    assert_eq!(write(fs.as_ref(), &node, 0, &[0x61; 8 * 4096]).unwrap().bytes, 8 * 4096);
    let dirty = fs.statfs().unwrap();
    assert_eq!(dirty.free_blocks, before.free_blocks);
    assert!(dirty.available_blocks <= before.available_blocks - 8,
        "8 dirty pages must be excluded from f_bavail: before={} after={}", before.available_blocks, dirty.available_blocks);
    fs.fsync(&node, false).unwrap();
    let durable = fs.statfs().unwrap();
    assert_eq!(durable.available_blocks, durable.free_blocks.saturating_sub(4096),
        "durable grants must be released");
    // An overwrite needs COW blocks even though st_blocks cannot grow.
    assert_eq!(write(fs.as_ref(), &node, 0, &[0x62; 8 * 4096]).unwrap().bytes, 8 * 4096);
    let overwritten = fs.statfs().unwrap();
    assert_eq!(overwritten.free_blocks, durable.free_blocks);
    assert!(overwritten.available_blocks <= durable.available_blocks - 8);
    // Rewriting already dirty pages must not reserve them again.
    write(fs.as_ref(), &node, 0, &[0x63; 8 * 4096]).unwrap();
    assert_eq!(fs.statfs().unwrap().available_blocks, overwritten.available_blocks);
}

#[test]
fn full_disk_write_refuses_without_changing_manifest_or_failing_mount() {
    // Pin time so the manifests compare mutation, including timestamps.
    crate::time::with_unix_time(10, || {
        let (disk, fs, node) = fixture();
        let bytes = vec![0x65; 1024 * 1024];
        let mut offset = 0;
        let before = loop {
            assert!(offset < 64 * 1024 * 1024, "64 MiB image must fill");
            let before = manifest(fs.as_ref(), &node);
            match write(fs.as_ref(), &node, offset, &bytes) {
                Ok(written) => {
                    assert!(written.bytes > 0 && written.bytes <= bytes.len());
                    offset += written.bytes as u64;
                    fs.fsync(&node, false).unwrap();
                }
                Err(error) => {
                    assert_eq!(error.errno(), Errno(ENOSPC));
                    break before;
                }
            }
        };
        assert_eq!(manifest(fs.as_ref(), &node), before, "failing write changed the manifest");
        assert_eq!(fs.read_only(), Ok(false));
        assert_eq!(fs.fsync(&node, false), Ok(()), "reservation ENOSPC must not latch");
        let ino = node.ino;
        drop(node);
        drop(fs);
        let fs = mount(disk);
        let node = fs.open(ino).unwrap();
        assert_eq!(manifest(fs.as_ref(), &node), before, "remount changed the manifest");
        assert_eq!(fs.fsync(&node, false), Ok(()));
    });
}

#[test]
fn mapped_enospc_keeps_dirty_pages_for_fsync_and_msync() {
    let (_disk, fs, node) = fixture();
    let before = fs.statfs().unwrap();
    fs.resize(&node, 1024 * 1024, false, &|_| Ok(())).unwrap();
    assert!(fs.statfs().unwrap().available_blocks < before.available_blocks,
        "extending truncate must reserve its metadata");
    fs.fsync(&node, false).unwrap();
    fs.mapping_io(&node).unwrap().publish(&mut |_| Ok(())).unwrap();
    let fill = fs.create(2, b"fill", NewNode::Regular {
        attrs: InodeAttrs::new(0o600, 0, 0),
    }).unwrap();
    let fill = fs.open(fill).unwrap();
    let mut offset = 0;
    // Fill with large requests, then consume the remaining admission headroom
    // a page at a time, so even one 64-page mapping batch cannot reserve.
    for length in [1024 * 1024, 256 * 1024, 64 * 1024, 16 * 1024, 4096] {
        let bytes = vec![0x69; length];
        loop {
            assert!(offset < 64 * 1024 * 1024);
            match write(fs.as_ref(), &fill, offset, &bytes) {
                Ok(written) => {
                    assert!(written.bytes > 0);
                    offset += written.bytes as u64;
                    fs.fsync(&fill, false).unwrap();
                }
                Err(error) => { assert_eq!(error.errno(), Errno(ENOSPC)); break; }
            }
        }
    }
    node.map.shared_stores(&vec![0x72; 1024 * 1024], false);
    assert_eq!(fs.fsync(&node, false), Err(Errno(ENOSPC)));
    assert!(node.map.has_dirty(), "fsync ENOSPC lost mapped dirty custody");
    assert_eq!(crate::memory::file_map::sync_range(&node, 0, u64::MAX), Err(Errno(ENOSPC)));
    assert!(node.map.has_dirty(), "msync ENOSPC lost mapped dirty custody");
    assert_eq!(fs.read_only(), Ok(false));
    // Release the synthetic host frames after the dirty-custody assertions, so this
    // ENOSPC fixture does not retain a service-owned VNode after the test.
    node.map.shared_stores(&[], false);
    fs.shutdown("enospc-cleanup", u64::MAX).unwrap();
}
