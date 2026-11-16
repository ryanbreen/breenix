//! E24 ledger 309 C2: durability through mapping entry points, not direct
//! adapter writeback calls. Host pages replace only frames and page tables.
use super::*;
use alloc::vec;
use std::sync::Mutex as HostMutex;

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("sequential mapping check waited for I/O")
    }
    fn wake_all(&self) {}
}
struct Disk(HostMutex<Vec<u8>>);
impl BlockDevice for Disk {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        bytes.copy_from_slice(&self.0.lock().unwrap()[start..start + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        self.0.lock().unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        16384
    }
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
}
fn fixture() -> (Arc<BreenFs<NoWait>>, Arc<Disk>, Arc<VNode>) {
    let disk = Arc::new(Disk(HostMutex::new(vec![0; 64 * 1024 * 1024])));
    let sb = Superblock {
        fs_uuid: [0x22; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        next_root_id: 2,
        ..Superblock::default()
    };
    let mut files = Files::format(super::super::commit::DeviceStore(disk.as_ref()), &sb).unwrap();
    files
        .fsync(disk.as_ref(), &mut Committer::format(sb))
        .unwrap();
    let fs =
        BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 72, 1).unwrap();
    fs.init_service().unwrap();
    let ino = fs
        .create(
            2,
            b"mapped",
            NewNode::Regular {
                attrs: InodeAttrs::new(0o100600, 0, 0),
            },
        )
        .unwrap();
    let node = fs.open(ino).unwrap();
    fs.resize(&node, 67 * 4096, false, &|_| Ok(())).unwrap();
    fs.fsync(&node, false).unwrap();
    // mmap obtains a MappingIo transfer before it publishes shared pages.
    drop(fs.mapping_io(&node).unwrap());
    (fs, disk, node)
}
fn expected(tag: u8) -> Vec<u8> {
    (0..67 * 4096)
        .map(|i| tag.wrapping_add((i % 239) as u8))
        .collect()
}
fn cold_bytes(disk: &Arc<Disk>, ino: u64) -> Vec<u8> {
    let observer = BreenFs::mount(disk.clone(), Arc::new(BufferCache::new(NoWait)), 73, 1).unwrap();
    let node = observer.open(ino).unwrap();
    let mut bytes = vec![0; 67 * 4096];
    assert_eq!(observer.read(&node, 0, &mut bytes).unwrap(), bytes.len());
    bytes
}
fn fsync_stores(data_only: bool) {
    let (fs, disk, node) = fixture();
    let bytes = expected(if data_only { 23 } else { 11 });
    node.map.shared_stores(&bytes, true);
    if data_only {
        node.map.request_writeback();
    }
    fs.fsync(&node, data_only).unwrap();
    assert!(
        cold_bytes(&disk, node.ino) == bytes,
        "fsync data_only={data_only} lost shared stores"
    );
}
#[test]
fn c2_fsync_publishes_shared_stores() {
    fsync_stores(false);
}
#[test]
fn c2_fdatasync_after_async_publishes_shared_stores() {
    fsync_stores(true);
}
#[test]
fn c2_sync_range_publishes_shared_stores_u29() {
    let (fs, disk, node) = fixture();
    let bytes = expected(17);
    node.map.shared_stores(&bytes, true);
    crate::memory::file_map::sync_range(&node, 0, u64::MAX).unwrap();
    assert!(
        cold_bytes(&disk, node.ino) == bytes,
        "MS_SYNC lost shared stores"
    );
    drop(fs);
}
#[test]
fn c2_async_request_is_serviced_by_commit_worker() {
    let (fs, disk, node) = fixture();
    let bytes = expected(31);
    node.map.shared_stores(&bytes, false);
    // MS_ASYNC's entry point marks the mapping pending and requests eviction.
    node.map.request_writeback();
    let ino = node.ino;
    let map = node.map.clone();
    // Close the final descriptor and binding before the service takes custody.
    drop(node);
    // Two bounded worker passes must cover 67 pages without fsync or teardown.
    for _ in 0..2 {
        fs.commit_due(monotonic_ns()).unwrap();
    }
    assert!(
        cold_bytes(&disk, ino) == bytes,
        "MS_ASYNC was not serviced after final close"
    );
    assert!(
        !map.has_dirty(),
        "durable unbound pages retained dirty custody"
    );
}
#[test]
fn c2_shutdown_publishes_shared_stores_before_stopping() {
    let (fs, disk, node) = fixture();
    let bytes = expected(47);
    node.map.shared_stores(&bytes, false);
    fs.shutdown("c2-mapping", u64::MAX).unwrap();
    assert!(
        cold_bytes(&disk, node.ino) == bytes,
        "shutdown lost shared stores"
    );
}
