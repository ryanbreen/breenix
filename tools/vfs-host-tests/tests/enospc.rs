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
fn a_write_does_not_refresh_untouched_open_inodes() {
    let (_disk, fs, node) = fixture();
    let mut held = Vec::new();
    for index in 0..16 {
        let name = format!("held-{index}");
        let ino = fs.create(2, name.as_bytes(), NewNode::Regular {
            attrs: InodeAttrs::new(0o640, 1000, 1000),
        }).unwrap();
        held.push(fs.open(ino).unwrap());
    }
    write(fs.as_ref(), &node, 0, &[0x5a; 4096]).unwrap();
    crate::breenfs::btree::measurement::take_decodes();
    write(fs.as_ref(), &node, 4096, &[0xa5; 4096]).unwrap();
    let with_unrelated = crate::breenfs::btree::measurement::take_decodes();
    for other in &held {
        assert_eq!(other.size.load(std::sync::atomic::Ordering::Acquire), 0);
    }
    drop(held);
    crate::breenfs::btree::measurement::take_decodes();
    write(fs.as_ref(), &node, 8192, &[0xcc; 4096]).unwrap();
    let alone = crate::breenfs::btree::measurement::take_decodes();
    assert_eq!(with_unrelated, alone,
        "one-inode write decoded {with_unrelated} nodes with unrelated open files versus {alone} alone");
    assert_eq!(node.size.load(std::sync::atomic::Ordering::Acquire), 12288);
    let mut bytes = [0; 12288];
    assert_eq!(fs.read(&node, 0, &mut bytes).unwrap(), bytes.len());
    assert_eq!(&bytes[..4096], &[0x5a; 4096]);
    assert_eq!(&bytes[4096..8192], &[0xa5; 4096]);
    assert_eq!(&bytes[8192..], &[0xcc; 4096]);
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
    // munmap/MS_ASYNC asks the real worker to stage these dirty pages.
    node.map.request_writeback();
    use crate::breenfs::vfs::measurement;
    measurement::service(&fs).unwrap();
    assert!(node.map.has_dirty(), "worker ENOSPC lost mapped dirty custody");
    measurement::take_work();
    // Follow the worker's immediate self-wakes at a fixed monotonic time.
    // With no new free space, they must settle without more publications.
    for _ in 0..8 {
        assert_eq!(measurement::service_at(&fs, 0).unwrap(), (false, false),
            "failed mapping writeback must sleep rather than publish and self-wake");
    }
    assert_eq!(measurement::take_work(), [0; 3]);
    assert_eq!(fs.fsync(&node, false), Err(Errno(ENOSPC)));
    assert!(node.map.has_dirty(), "fsync ENOSPC lost mapped dirty custody");
    assert_eq!(crate::memory::file_map::sync_range(&node, 0, u64::MAX), Err(Errno(ENOSPC)));
    assert!(node.map.has_dirty(), "msync ENOSPC lost mapped dirty custody");
    assert_eq!(fs.read_only(), Ok(false));
    // Free space and advance the retry deadline: async writeback must resume.
    fs.resize(&fill, 0, false, &|_| Ok(())).unwrap();
    fs.fsync(&fill, false).unwrap();
    for _ in 0..8 {
        measurement::service_at(&fs, crate::breenfs::commit_policy::COMMIT_INTERVAL_NS).unwrap();
    }
    assert!(!node.map.has_dirty(), "worker did not retry after space was freed");
    let mut bytes = vec![0; 1024 * 1024];
    assert_eq!(fs.read(&node, 0, &mut bytes).unwrap(), bytes.len());
    assert!(bytes.iter().all(|byte| *byte == 0x72));
    node.map.shared_stores(&[], false);
    fs.shutdown("enospc-cleanup", u64::MAX).unwrap();
}

// The flat fixture includes empty directories, symlinks, xattrs and link counts.
// Pin observation reads to the fixture time so they do not advance atime.
fn namespace_manifest(fs: &dyn FileSystem) -> Vec<(Vec<u8>, NodeAttrs, Vec<u8>, Vec<u8>)> {
    let mut names = vec![(Vec::new(), 2)];
    fs.readdir(2, 0, &mut |entry, _| { names.push((entry.name, entry.ino)); Ok(true) }).unwrap();
    names.sort();
    crate::time::with_unix_time(10, || names.into_iter().map(|(name, ino)| {
        let attrs = fs.getattr(ino).unwrap();
        let content = match attrs.kind {
            NodeKind::Regular => {
                let node = fs.open(ino).unwrap();
                let mut bytes = vec![0; attrs.size as usize];
                assert_eq!(fs.read(&node, 0, &mut bytes).unwrap(), bytes.len());
                sha256::sha256(&bytes).to_vec()
            }
            NodeKind::Symlink => fs.readlink(ino).unwrap(),
            _ => Vec::new(),
        };
        let mut keys = vec![0; fs.xattr(ino, Xattr::List { names: &mut [] }).unwrap()];
        fs.xattr(ino, Xattr::List { names: &mut keys }).unwrap();
        let mut xattrs = Vec::new();
        for key in keys.split(|b| *b == 0).filter(|k| !k.is_empty()) {
            let mut value = vec![0; fs.xattr(ino, Xattr::Get { name: key, value: &mut [] }).unwrap()];
            fs.xattr(ino, Xattr::Get { name: key, value: &mut value }).unwrap();
            xattrs.extend_from_slice(key);
            xattrs.extend_from_slice(&(value.len() as u64).to_le_bytes());
            xattrs.extend_from_slice(&value);
        }
        (name, attrs, content, xattrs)
    }).collect())
}
fn remove_name(fs: &dyn FileSystem, name: &[u8], directory: bool) {
    let ino = fs.lookup(2, name).unwrap();
    fs.remove(2, name, Remove {
        directory, expected: ino, owner: fs.getattr(ino).unwrap().attrs.uid,
        parent: fs.getattr(2).unwrap().attrs,
    }).unwrap();
}
// Appending data leaves only metadata in PENDING. After the deletion's two
// quarantine commits, that is also true of the post-delete image.
fn metadata_blocks(disk: &Disk) -> u64 {
    use crate::breenfs::{commit::DeviceStore, format, recover};
    let sb = recover::recover(disk, false).unwrap();
    recover::space_items(&DeviceStore(disk), &sb).unwrap().into_iter().map(|(key, value)| {
        match key.item_type {
            format::REF if format::Ref::decode(&value).unwrap().kind != 3 => key.offset,
            format::PENDING => {
                let length = format::Pending::decode(&value).unwrap().length;
                assert_eq!(length % 4, 0, "PENDING data is not metadata overhead");
                let bytes = disk.bytes.lock().unwrap();
                for addr in (key.offset..key.offset + length).step_by(4) {
                    let start = addr as usize * format::BLOCK_SIZE;
                    let header = format::NodeHeader::decode(&bytes[start..start + format::NODE_SIZE])
                        .expect("PENDING file data must become FREE after quarantine");
                    assert_eq!(header.addr, addr);
                    assert_eq!(header.fs_uuid, sb.fs_uuid);
                }
                length
            },
            _ => 0,
        }
    }).sum()
}

#[test]
fn full_disk_namespace_refuses_atomically_and_deletes_use_reserve() {
    use crate::breenfs::{
        commit::{Committer, DeviceStore},
        format::{Time, BLOCK_SIZE},
        ops::Files,
        recover,
        reserve::observation,
    };

    crate::time::with_unix_time(10, || {
        let (disk, fs, fill) = fixture();
        let attrs = InodeAttrs::new(0o640, 1000, 1000);
        let victim = fs.create(2, b"victim", NewNode::Regular { attrs }).unwrap();
        let victim_node = fs.open(victim).unwrap();
        write(fs.as_ref(), &victim_node, 0, &vec![0x76; 1024 * 1024]).unwrap();
        drop(victim_node);
        let truncated = fs.create(2, b"truncate", NewNode::Regular { attrs }).unwrap();
        let truncated = fs.open(truncated).unwrap();
        write(fs.as_ref(), &truncated, 0, &vec![0x74; 1024 * 1024]).unwrap();
        fs.create(2, b"empty", NewNode::Directory { attrs }).unwrap();
        fs.create(2, b"symlink", NewNode::Symlink { attrs, target: b"data" }).unwrap();
        fs.xattr(fill.ino, Xattr::Set { name: b"user.old", value: b"kept", mode: XattrSet::Create }).unwrap();
        fs.fsync(&fill, false).unwrap();
        let mut offset = 0;
        for length in [1024 * 1024, 256 * 1024, 64 * 1024, 16 * 1024, 4096] {
            let bytes = vec![0x66; length];
            loop {
                assert!(offset < 64 * 1024 * 1024);
                match write(fs.as_ref(), &fill, offset, &bytes) {
                    Ok(written) => {
                        assert!(written.bytes > 0);
                        offset += written.bytes as u64;
                        fs.fsync(&fill, false).unwrap();
                    }
                    Err(e) => { assert_eq!(e.errno(), Errno(ENOSPC)); break; }
                }
            }
        }
        // Even a one-block VFS write can refuse while its worst-case
        // metadata budget leaves enough room for a create. Finish staging
        // the disposable image with the offline writer, one block at a time.
        // Leave room for its metadata before remounting through the VFS.
        let available = fs.statfs().unwrap().available_blocks;
        let fill_ino = fill.ino;
        let truncate_ino = truncated.ino;
        drop(fill);
        drop(truncated);
        drop(fs);
        {
            let sb = recover::recover(disk.as_ref(), false).unwrap();
            let mut files = Files::new(DeviceStore(disk.as_ref()), &sb, 1, true).unwrap();
            let block = [0x66; BLOCK_SIZE];
            const METADATA_HEADROOM: u64 = 32;
            for _ in METADATA_HEADROOM..available {
                assert_eq!(files.write(disk.as_ref(), fill_ino, offset, &block, Time {
                    seconds: 10, nanos: 0,
                }).unwrap(), BLOCK_SIZE);
                offset += BLOCK_SIZE as u64;
            }
            files.fsync(disk.as_ref(), &mut Committer::new(sb)).unwrap();
        }
        let fs = mount(disk.clone());
        let fill = fs.open(fill_ino).unwrap();
        let truncated = fs.open(truncate_ino).unwrap();
        assert!(fs.statfs().unwrap().free_blocks >= 4096, "setup spent the global reserve");
        let before = namespace_manifest(fs.as_ref());
        crate::time::with_unix_time(20, || {
            for new in [NewNode::Directory { attrs }, NewNode::Regular { attrs },
                NewNode::Symlink { attrs, target: b"data" }] {
                let available_before = fs.statfs().unwrap().available_blocks;
                observation::take();
                assert_eq!(fs.create(2, b"refused", new), Err(Errno(ENOSPC)));
                let [_, _, _, charge, available] =
                    observation::take().expect("create must refuse a block reservation");
                println!("FULL_DISK_CREATE available_before={available_before} available={available} charge={charge}");
                assert!(available_before < charge && available < charge,
                    "fixture must be short of the create's charge");
                assert_eq!(namespace_manifest(fs.as_ref()), before);
            }
            assert_eq!(fs.xattr(fill.ino, Xattr::Set {
                name: b"user.new", value: b"refused", mode: XattrSet::Create,
            }), Err(Errno(ENOSPC)));
            assert_eq!(fs.link(fill.ino, 2, b"alias", &|_, _| Ok(())), Err(Errno(ENOSPC)));
            assert_eq!(fs.rename(2, b"symlink", 2, b"renamed", RenameMode::Replace,
                &|_, _, _, _| Ok(())), Err(Errno(ENOSPC)));
            assert_eq!(fs.setattr(fill.ino, SetAttr { mode: Some(0o600), ..SetAttr::default() }),
                Err(Errno(ENOSPC)));
            assert_eq!(fs.create_open(2, b"opened", NodeKind::Regular, &|_| Ok(attrs),
                &mut |_| panic!("prepare ran without a reservation")), Err(Errno(ENOSPC)));
        });
        assert_eq!(namespace_manifest(fs.as_ref()), before);
        assert_eq!(fs.read_only(), Ok(false));
        fs.fsync(&fill, false).unwrap();
        drop(fill); drop(truncated); drop(fs);
        let fs = mount(disk.clone());
        assert_eq!(namespace_manifest(fs.as_ref()), before, "refused mutation survived remount");
        let fill = fs.open(fill_ino).unwrap();
        let truncated = fs.open(truncate_ino).unwrap();
        let freed = fs.getattr(victim).unwrap().blocks + fs.getattr(truncate_ino).unwrap().blocks;
        // NodeAttrs counts 512-byte sectors; statfs counts 4096-byte blocks.
        let freed = freed / 8;
        let available = fs.statfs().unwrap().available_blocks;
        let metadata = metadata_blocks(&disk);
        remove_name(fs.as_ref(), b"victim", false);
        remove_name(fs.as_ref(), b"empty", true);
        fs.resize(&truncated, 0, false, &|_| Ok(())).unwrap();
        fs.xattr(fill.ino, Xattr::Remove { name: b"user.old" }).unwrap();
        fs.fsync(&fill, false).unwrap();
        fs.fsync(&fill, false).unwrap();
        fs.fsync(&fill, false).unwrap();
        let after = fs.statfs().unwrap();
        let metadata_after = metadata_blocks(&disk);
        assert!(after.available_blocks > available, "deletion did not return usable space");
        assert_eq!(after.available_blocks as i64 - available as i64,
            freed as i64 - (metadata_after as i64 - metadata as i64));
        assert_eq!(after.available_blocks, after.free_blocks.saturating_sub(4096));
        assert_eq!(fs.getattr(truncate_ino).unwrap().size, 0);
        assert_eq!(fs.lookup(2, b"victim"), Err(Errno(crate::syscall_errno::ENOENT)));
        assert_eq!(fs.lookup(2, b"empty"), Err(Errno(crate::syscall_errno::ENOENT)));
    });
}

#[path = "../../../userspace/programs/src/filesystem_enospc_fill.rs"]
mod guest_fill;

#[test]
fn guest_enospc_observes_the_full_file_once_at_refusal() {
    use std::{cell::RefCell, io, rc::Rc};
    struct LimitedFile(Rc<RefCell<Vec<u8>>>);
    const LIMIT: usize = 48 * 1024 * 1024;
    impl io::Write for LimitedFile {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut file = self.0.borrow_mut();
            if file.len() + bytes.len() > LIMIT {
                return Err(io::Error::from_raw_os_error(28));
            }
            file.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    let bytes = Rc::new(RefCell::new(Vec::new()));
    let mut file = LimitedFile(bytes.clone());
    let mut observations = 0;
    let mut read_bytes = 0;
    let accepted = guest_fill::fill_and_observe(&mut file, &vec![0x5a; 1024 * 1024], |accepted| {
        observations += 1;
        let snapshot = bytes.borrow().clone();
        read_bytes += snapshot.len();
        assert_eq!(snapshot.len(), accepted);
        assert!(snapshot.iter().all(|byte| *byte == 0x5a));
        Ok(())
    }).unwrap();
    assert_eq!(accepted, LIMIT);
    assert_eq!(observations, 1, "full-file reads must wait for the refused call");
    assert_eq!(read_bytes, LIMIT, "manifest observation must be linear in file size");
}

// E24 ledger 377/379(2), #3040: the worker may publish the full file
// before unlink. Its blocks then survive cleanup in PENDING across remount.
#[test]
fn durable_fill_delete_remount_allows_tiny_write() {
    use breenfs_host_tests::recording::RecordingDevice;
    use crate::trait_adapter::Device;
    let recorder = Arc::new(RecordingDevice::new(image()).unwrap());
    let disk = Arc::new(Device(recorder.clone()));
    let mount = || BreenFs::mount_writable(
        disk.clone(), Arc::new(BufferCache::new(NoWait)), 71, 1,
    ).unwrap();
    let attrs = InodeAttrs::new(0o700, 0, 0);
    let fs = mount();
    let dir = fs.create(2, b"work", NewNode::Directory { attrs }).unwrap();
    let ino = fs.create(dir, b"fill", NewNode::Regular { attrs }).unwrap();
    let node = fs.open(ino).unwrap();
    let bytes = vec![0x5a; 1024 * 1024];
    let mut offset = 0;
    loop {
        match write(fs.as_ref(), &node, offset, &bytes) {
            Ok(result) => {
                assert_eq!(result.bytes, bytes.len());
                offset += result.bytes as u64;
            }
            Err(error) => { assert_eq!(error.errno(), Errno(ENOSPC)); break; }
        }
        assert!(offset < 64 * 1024 * 1024, "production image must fill");
    }
    assert!(offset > 0);
    // Force the worker ordering from the lifecycle probe, without a timer.
    fs.fsync(&node, false).unwrap();
    drop(node);
    fs.remove(dir, b"fill", Remove {
        directory: false, expected: ino, owner: 0,
        parent: fs.getattr(dir).unwrap().attrs,
    }).unwrap();
    remove_name(fs.as_ref(), b"work", true);
    fs.shutdown("pressure-test", u64::MAX).unwrap();
    drop(fs);

    let fs = mount();
    assert_eq!(fs.lookup(2, b"work"), Err(Errno(crate::syscall_errno::ENOENT)));
    let dir = fs.create(2, b"work", NewNode::Directory { attrs }).unwrap();
    let ino = fs.create(dir, b"tiny", NewNode::Regular { attrs }).unwrap();
    let node = fs.open(ino).unwrap();
    let before = *recorder.durable_generations().last().unwrap();
    assert_eq!(write(fs.as_ref(), &node, 0, b"tiny").expect(
        "reservation pressure must publish quarantined space before refusing a tiny write",
    ).bytes, 4);
    assert!(*recorder.durable_generations().last().unwrap() - before <= 2,
        "pressure must stop after the existing PENDING quarantine expires");
    fs.fsync(&node, false).unwrap();
    drop(node);
    drop(fs);
    let fs = mount();
    let node = fs.open(ino).unwrap();
    let mut bytes = [0; 4];
    assert_eq!(fs.read(&node, 0, &mut bytes).unwrap(), 4);
    assert_eq!(&bytes, b"tiny");
    // The recording device checks W1-W3 on writes and flushes.
}

#[test]
fn dirty_inode_write_does_not_decode_siblings_or_recount_space() {
    let (disk, fs, node) = fixture();
    // Durable extents enlarge both trees. Subsequent dirty writes replace only
    // the same private INODE payload; they do not change either tree's shape.
    let chunk = vec![0x71; 64 * 1024];
    for index in 0..32 {
        write(fs.as_ref(), &node, index * chunk.len() as u64, &chunk).unwrap();
        fs.fsync(&node, false).unwrap();
    }
    let end = 32 * chunk.len() as u64;
    write(fs.as_ref(), &node, end, &[0x81; 4096]).unwrap();
    // Populate reservation geometry for this unchanged SPACE root.
    write(fs.as_ref(), &node, end + 4096, &[0x82; 4096]).unwrap();
    crate::breenfs::btree::measurement::take_work();
    write(fs.as_ref(), &node, end + 8192, &[0x83; 4096]).unwrap();
    let work = crate::breenfs::btree::measurement::take_work();
    assert_eq!(work, [0; 4],
        "private inode replacement decoded unrelated items or recounted unchanged trees: {work:?}");
    assert_eq!(node.size.load(std::sync::atomic::Ordering::Acquire), end + 12288);
    fs.fsync(&node, false).unwrap();
    let ino = node.ino;
    drop(node); drop(fs);
    let fs = mount(disk);
    let node = fs.open(ino).unwrap();
    let mut bytes = vec![0; 12288];
    assert_eq!(fs.read(&node, end, &mut bytes).unwrap(), bytes.len());
    for (index, expected) in [0x81, 0x82, 0x83].into_iter().enumerate() {
        assert_eq!(&bytes[index * 4096..(index + 1) * 4096], &[expected; 4096]);
    }
}
