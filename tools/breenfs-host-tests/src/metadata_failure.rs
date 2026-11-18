//! E24 ledger 509(o), #3065: distinguish metadata failure from memory pressure.
use super::*;
use super::super::{btree::NodeStore, commit::DeviceStore, format::NODE_SIZE};
use alloc::{string::String, vec};
use std::sync::Mutex as HostMutex;

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 { 0 }
    fn wait(&self, _: u64) { panic!("synchronous metadata test waited") }
    fn wake_all(&self) {}
}
struct Disk {
    bytes: HostMutex<Vec<u8>>,
    fail_reads: core::sync::atomic::AtomicBool,
    writes: AtomicU64,
}
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { 16384 }
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        if self.fail_reads.load(Ordering::Acquire) { return Err(BlockError::IoError); }
        let start = block as usize * BLOCK_SIZE;
        bytes.copy_from_slice(&self.bytes.lock().unwrap()[start..start + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        self.bytes.lock().unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
        self.writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
fn fixture() -> (Arc<BreenFs<NoWait>>, Arc<Disk>, Superblock, format::BlockPtr) {
    let disk = Arc::new(Disk { bytes: HostMutex::new(vec![0; 64 * 1024 * 1024]),
        fail_reads: core::sync::atomic::AtomicBool::new(false), writes: AtomicU64::new(0) });
    let sb = Superblock { fs_uuid: [0x6f; 16], generation: 1, slot: 1,
        total_blocks: 16384, global_reserve: 4096, next_root_id: 2,
        ..Superblock::default() };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    files.create(2, b"cold", 0o600, Time::default()).unwrap();
    let sb = files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    drop(files);
    let root = recover::space_items(&DeviceStore(disk.as_ref()), &sb).unwrap()
        .into_iter().find(|(key, _)| key.item_type == format::ROOT && key.offset == 1).unwrap().1;
    let ptr = Root::decode(&root).unwrap().root;
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 79, 1).unwrap();
    fs.init_service().unwrap();
    (fs, disk, sb, ptr)
}
fn cause(expected: &str, pointer_mismatch: bool) {
    let (fs, disk, sb, mut ptr) = fixture();
    let cache = BufferCache::new(NoWait);
    let store = CachedNodeStore::new(&cache, disk.as_ref(), &sb);
    match expected {
        "device-read" => disk.fail_reads.store(true, Ordering::Release),
        "verification" if pointer_mismatch => ptr.csum ^= 1,
        "verification" => disk.bytes.lock().unwrap()[ptr.addr as usize * BLOCK_SIZE + NODE_SIZE - 1] ^= 1,
        "allocation" => (),
        _ => panic!("unknown fixture cause"),
    }
    let result = if expected == "allocation" {
        super::dirty_heap_bound::fail_allocation(NODE_SIZE, 1, || store.read_node(ptr))
    } else { store.read_node(ptr) };
    let error = result.err().expect("fault did not reach node read");
    assert!(fs.inner.state.lock().observed::<()>(Err(error.into())).is_err());
    let mut report = String::new();
    fs.report(Report::State, "/", &mut report).unwrap();
    assert!(report.contains(&alloc::format!("metadata_cause={expected}")), "{report}");
    assert!(report.contains(&alloc::format!("metadata_node={}", ptr.addr)), "{report}");
    if expected != "allocation" {
        assert_eq!(fs.failure(), Some(("observed-metadata", EIO)));
        assert_eq!(fs.read_only(), Ok(true));
        // Later observations must not replace the first integrity/device failure.
        fs.inner.state.lock().observed::<()>(Err(ops::Error::EIO)).unwrap_err();
        let mut later = String::new();
        fs.report(Report::State, "/", &mut later).unwrap();
        assert_eq!(later, report);
    }
}
#[test]
fn metadata_device_read_cause_and_node_are_recorded() { cause("device-read", false); }
#[test]
fn metadata_verification_cause_and_node_are_recorded() {
    cause("verification", false);
    cause("verification", true);
}
#[test]
fn metadata_allocation_cause_and_node_are_recorded() { cause("allocation", false); }

#[test]
fn node_buffer_enomem_returns_to_caller_without_latching() {
    let (fs, disk, _, _) = fixture();
    let writes = disk.writes.load(Ordering::Relaxed);
    let result = super::dirty_heap_bound::fail_allocation(NODE_SIZE, 1, || fs.getattr(16));
    assert_eq!(result, Err(Errno(ENOMEM)), "node allocation was treated as disk failure");
    assert_eq!(fs.failure(), None);
    assert_eq!(fs.read_only(), Ok(false));
    assert_eq!(disk.writes.load(Ordering::Relaxed), writes);
    // Retry the same cold lookup, then mutate and publish on the same mount.
    assert_eq!(fs.getattr(16).unwrap().kind, NodeKind::Regular);
    fs.create(2, b"after-pressure", NewNode::Regular {
        attrs: InodeAttrs::new(0o600, 0, 0),
    }).unwrap();
    fs.sync().unwrap();
    assert_eq!(fs.read_only(), Ok(false));
}

fn retire_after_latch(dirty: bool) {
    use crate::memory::file_map::bind_host;
    let (fs, disk, _, _) = fixture();
    let node = fs.open(16).unwrap();
    let pages = 67;
    fs.resize(&node, pages * BLOCK_SIZE as u64, false, &|_| Ok(())).unwrap();
    fs.fsync(&node, false).unwrap();
    let binding = bind_host(node.clone(), pages * BLOCK_SIZE as u64, true, true, true);
    if dirty {
        for page in 0..pages { binding.store(page * BLOCK_SIZE as u64, 0x6f).unwrap(); }
    }
    let map = node.map.clone();
    let weak = Arc::downgrade(&node);
    let writes = disk.writes.load(Ordering::Relaxed);
    fs.inner.state.lock().observed::<()>(Err(ops::Error::EIO)).unwrap_err();
    // Live bindings retain their pages; service must never publish after EIO.
    assert_eq!(fs.commit_due(0), Err(Errno(EIO)));
    assert!(map.nonempty());
    drop(binding);
    // The kernel Binding::drop also queues writeback; the host binding only
    // models unbinding, so issue that same request explicitly here.
    map.request_writeback();
    drop(node);
    assert_eq!(fs.mapped_nodes().unwrap().len(), 1, "failed mount hid its mapping pin");
    let live = super::dirty_heap_bound::live();
    for _ in 0..pages.div_ceil(crate::memory::file_map::EVICT_BUDGET as u64) {
        assert_eq!(fs.commit_due(0), Err(Errno(EIO)));
    }
    assert!(!map.nonempty(), "failed mount retained unbound mapped pages");
    assert!(!map.has_dirty());
    assert!(weak.upgrade().is_none(), "service retained the last VNode pin");
    assert!(live - super::dirty_heap_bound::live() >= (pages * BLOCK_SIZE as u64) as isize,
        "page flags cleared without releasing the host frames");
    assert_eq!(disk.writes.load(Ordering::Relaxed), writes);
    assert_eq!(fs.failure(), Some(("observed-metadata", EIO)));
    assert_eq!(fs.read_only(), Ok(true));
    assert_eq!(fs.sync(), Err(Errno(EIO)));
}
#[test]
fn failed_mount_releases_clean_mapped_pages_after_final_unbind() { retire_after_latch(false); }
#[test]
fn failed_mount_releases_dirty_unobserved_pages_without_writeback() { retire_after_latch(true); }
