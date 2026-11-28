//! Device and B-tree observations through the production BreenFS mount.
use super::super::{
    btree::{self, NodeStore},
    commit::DeviceStore,
    format::{BlockPtr, Key, NodeHeader},
};
use super::*;
use alloc::{format, vec};
use std::sync::Mutex as HostMutex;

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("synchronous metadata cache test waited")
    }
    fn wake_all(&self) {}
}

struct Disk {
    bytes: HostMutex<Vec<u8>>,
    reads: AtomicU64,
}
impl BlockDevice for Disk {
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        (self.bytes.lock().unwrap().len() / BLOCK_SIZE) as u64
    }
    fn read_block(&self, start: u64, out: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(start, 1, out)
    }
    fn read_blocks(&self, start: u64, count: usize, out: &mut [u8]) -> Result<(), BlockError> {
        assert_eq!(out.len(), count * BLOCK_SIZE);
        let start = start as usize * BLOCK_SIZE;
        out.copy_from_slice(
            self.bytes
                .lock()
                .unwrap()
                .get(start..start + out.len())
                .ok_or(BlockError::OutOfBounds)?,
        );
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn write_block(&self, start: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let start = start as usize * BLOCK_SIZE;
        self.bytes
            .lock()
            .unwrap()
            .get_mut(start..start + bytes.len())
            .ok_or(BlockError::OutOfBounds)?
            .copy_from_slice(bytes);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
}

fn name(index: usize) -> alloc::string::String {
    format!("file-{index:05}")
}

fn fixture(count: usize) -> (Arc<BreenFs<NoWait>>, Arc<Disk>, Vec<u64>) {
    let disk = Arc::new(Disk {
        bytes: HostMutex::new(vec![0; 64 * 1024 * 1024]),
        reads: AtomicU64::new(0),
    });
    let sb = Superblock {
        fs_uuid: [0x29; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        global_reserve: 4096,
        next_root_id: 2,
        dirhash_key: [0x71; 16],
        ..Superblock::default()
    };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let inodes = (0..count)
        .map(|index| {
            files
                .create(2, name(index).as_bytes(), 0o600, Time::default())
                .unwrap()
        })
        .collect();
    files
        .fsync(disk.as_ref(), &mut Committer::format(sb))
        .unwrap();
    let fs =
        BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 197, 1).unwrap();
    fs.init_service().unwrap();
    (fs, disk, inodes)
}

fn leaf(disk: &Disk, mut ptr: BlockPtr, ino: u64) -> BlockPtr {
    let key = Key {
        object: ino,
        item_type: format::INODE,
        offset: 0,
    };
    while ptr.level != 0 {
        let node = DeviceStore(disk).read_node(ptr).unwrap();
        let child = node
            .children
            .partition_point(|entry| entry.key <= key)
            .saturating_sub(1);
        ptr = node.child(child);
    }
    ptr
}

fn chmod(fs: &BreenFs<NoWait>, ino: u64, mode: u16) {
    fs.setattr(
        ino,
        SetAttr {
            mode: Some(mode),
            ..SetAttr::default()
        },
    )
    .unwrap();
}

#[test]
fn metadata_cache_keeps_an_unchanged_leaf_across_install() {
    let (fs, disk, inodes) = fixture(256);
    let unchanged = inodes[129];
    let ptr = {
        let state = fs.inner.state.lock();
        let ptr = leaf(&disk, state.files.root_pointer(), unchanged);
        state.files.read_node_for_test(ptr).unwrap();
        ptr
    };

    chmod(&fs, inodes[0], 0o644);
    fs.sync().unwrap();
    let state = fs.foreground_lock().unwrap();
    assert_eq!(leaf(&disk, state.files.root_pointer(), unchanged), ptr);
    let reads = disk.reads.load(Ordering::Relaxed);
    btree::measurement::take_decodes();
    state.files.read_node_for_test(ptr).unwrap();
    assert_eq!(
        (
            disk.reads.load(Ordering::Relaxed) - reads,
            btree::measurement::take_decodes()
        ),
        (0, 0),
        "unchanged leaf missed the decoded cache after install",
    );
}

#[test]
fn metadata_cache_rejects_a_reclaimed_pointer() {
    let (fs, disk, inodes) = fixture(2);
    let ptr = {
        let state = fs.inner.state.lock();
        let ptr = state.durable.space_root;
        state.files.read_node_for_test(ptr).unwrap();
        ptr
    };
    chmod(&fs, inodes[0], 0o644);
    fs.sync().unwrap();
    let released = fs.inner.state.lock().durable.generation;

    chmod(&fs, inodes[0], 0o640);
    fs.sync().unwrap();
    let quarantined_reads = {
        let state = fs.foreground_lock().unwrap();
        assert_eq!(state.durable.generation, released + 1);
        let reads = disk.reads.load(Ordering::Relaxed);
        state.files.read_node_for_test(ptr).unwrap();
        disk.reads.load(Ordering::Relaxed) - reads
    };

    chmod(&fs, inodes[0], 0o660);
    fs.sync().unwrap();
    let state = fs.foreground_lock().unwrap();
    assert_eq!(state.durable.generation, released + 2);
    let bytes = DeviceStore(disk.as_ref()).get(ptr.addr).unwrap();
    let header = NodeHeader::decode(&bytes).unwrap();
    assert_eq!((header.addr, header.generation), (ptr.addr, released + 2));
    let new_ptr = BlockPtr {
        addr: header.addr,
        generation: header.generation,
        csum: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        level: header.level,
    };
    assert!(
        state.files.read_node_for_test(ptr).is_err(),
        "recycled address returned the old node"
    );
    state.files.read_node_for_test(new_ptr).unwrap();
    assert_eq!(quarantined_reads, 0, "quarantined node was evicted");
}

#[test]
fn metadata_cache_keeps_an_unrelated_inode_through_write_and_install() {
    let (fs, _, inodes) = fixture(2);
    let expected = fs.getattr(inodes[0]).unwrap();
    let writer = fs.open(inodes[1]).unwrap();
    let write = WriteReq {
        at: WriteAt::Offset(0),
        limit: u64::MAX,
        clear_setid: false,
    };
    fs.write(&writer, write, b"new data").unwrap();
    let check = || {
        btree::measurement::take_mutations();
        assert_eq!(fs.getattr(inodes[0]).unwrap(), expected);
        assert_eq!(
            btree::measurement::take_mutations()[6],
            0,
            "cached inode traversed the tree"
        );
    };
    check();
    fs.sync().unwrap();
    // Process the writer's bounded install refresh before measuring the
    // unrelated lookup. The refresh reads its newly allocated block count.
    let attrs = fs.getattr(inodes[1]).unwrap();
    assert_eq!((attrs.size, attrs.blocks), (8, 8));
    check();
}

#[test]
fn metadata_cache_stat_10000() {
    let (fs, disk, inodes) = fixture(10_000);
    let pass = || {
        let reads = disk.reads.load(Ordering::Relaxed);
        for (index, &expected) in inodes.iter().enumerate() {
            let ino = fs.lookup(2, name(index).as_bytes()).unwrap();
            assert_eq!(ino, expected);
            assert_eq!(fs.getattr(ino).unwrap().size, 0);
        }
        disk.reads.load(Ordering::Relaxed) - reads
    };
    let cold = pass();
    let warm = pass();
    chmod(&fs, inodes[0], 0o644);
    fs.sync().unwrap();
    let after_commit = pass();
    std::println!("STAT_10000 device_requests cold={cold} warm={warm} after_commit={after_commit}");
    assert!(cold > 0);
    assert!(
        after_commit < cold,
        "the commit cold-started the metadata working set"
    );
}
