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
    read_error: HostMutex<Option<BlockError>>,
    writes: AtomicU64,
}
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { 16384 }
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        if let Some(error) = *self.read_error.lock().unwrap() { return Err(error); }
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
        fail_reads: core::sync::atomic::AtomicBool::new(false), read_error: HostMutex::new(None), writes: AtomicU64::new(0) });
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
fn ahci_preissue_refusal_leaves_root_writable() {
    let (fs, disk, sb, ptr) = fixture();
    let cache = BufferCache::new(NoWait);
    let store = CachedNodeStore::new(&cache, disk.as_ref(), &sb);
    let writes = disk.writes.load(Ordering::Relaxed);
    // AHCI's port gate refuses before submitting a command.
    *disk.read_error.lock().unwrap() = Some(BlockError::DeviceNotReady);
    let result = store.read_node(ptr).map_err(ops::Error::from);
    assert_eq!(fs.inner.state.lock().observed(result).err(), Some(Errno(EAGAIN)));
    assert_eq!(fs.failure(), None);
    assert_eq!(fs.read_only(), Ok(false));
    assert_eq!(disk.writes.load(Ordering::Relaxed), writes);

    *disk.read_error.lock().unwrap() = None;
    assert!(store.read_node(ptr).is_ok());
    assert_eq!(fs.getattr(16).unwrap().kind, NodeKind::Regular);
    fs.create(2, b"after-refusal", NewNode::Regular {
        attrs: InodeAttrs::new(0o600, 0, 0),
    }).unwrap();
    fs.sync().unwrap();
}

#[test]
fn ahci_media_error_on_metadata_latches_root() {
    let (fs, disk, sb, ptr) = fixture();
    let cache = BufferCache::new(NoWait);
    let store = CachedNodeStore::new(&cache, disk.as_ref(), &sb);
    *disk.read_error.lock().unwrap() = Some(BlockError::IoError);
    let result = store.read_node(ptr).map_err(ops::Error::from);
    assert_eq!(fs.inner.state.lock().observed(result).err(), Some(Errno(EIO)));
    assert_eq!(fs.failure(), Some(("observed-metadata", EIO)));
    assert_eq!(fs.read_only(), Ok(true));
    assert_eq!(fs.create(2, b"after-media-error", NewNode::Regular {
        attrs: InodeAttrs::new(0o600, 0, 0),
    }).err(), Some(Errno(EROFS)));
}

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
    // Keep the binding live while the worker observes the failed mount.
    assert_eq!(fs.commit_due(0), Err(Errno(EIO)));
    assert!(map.nonempty());
    drop(binding);
    // The kernel Binding::drop also queues writeback; the host binding only
    // models unbinding, so issue that same request explicitly here.
    map.request_writeback();
    drop(node);
    assert_eq!(fs.mapped_nodes(core::convert::identity).unwrap().len(), 1, "failed mount hid its mapping pin");
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

// The driver receipt crosses the production node store, observed() and both
// renderers. The wrapper boundary itself is executed by virtio_read_cause_test.py.
#[test]
fn virtio_read_receipt_reaches_metadata_serial_and_report() {
    use crate::block::read_failure::VirtioReadFailure;
    super::failed_mount_teardown::init_log();
    let (fs, disk, sb, ptr) = fixture();
    let receipt = VirtioReadFailure {
        reason: "Block MMIO read failed", stage: "finish-status", device: 0,
        sector: ptr.addr * 8, bytes: NODE_SIZE, thread: Some(73), preempt_count: 1, kill_custody: Some(1),
        in_interrupt: false, interrupts_enabled: true, timer_ready: true,
        gate_locked: true, gate_wedged: false, pending_token: 17, completed_token: 0, waiter_thread: 0,
        completed_desc: 0, completed_status: 1, used_idx: 9, last_used_idx: 9,
        device_status: 15,
    };
    *disk.read_error.lock().unwrap() = Some(BlockError::VirtioRead(receipt));
    let cache = BufferCache::new(NoWait);
    let store = CachedNodeStore::new(&cache, disk.as_ref(), &sb);
    let error = store.read_node(ptr).err().expect("injected read succeeded");
    assert_eq!(error, super::super::btree::Error::Read(super::super::btree::ReadFailure {
        cause: super::super::btree::ReadCause::Driver(receipt), addr: ptr.addr,
    }));
    assert_eq!(fs.inner.state.lock().observed::<()>(Err(error.into())), Err(Errno(EIO)));
    let mut report = String::new();
    fs.report(Report::State, "/", &mut report).unwrap();
    let detail = alloc::format!("metadata_cause=device-read {} metadata_node={}", receipt, ptr.addr);
    assert!(report.contains(&detail), "{report}");
    let serial = super::failed_mount_teardown::messages();
    assert!(serial.iter().any(|line| line.contains(&detail) && line.contains("latched read-only")), "{serial:?}");
    assert_eq!(fs.failure(), Some(("observed-metadata", EIO)));
}

#[test]
fn virtio_context_refusal_returns_eagain_without_latching() {
    use crate::block::read_failure::VirtioReadFailure;
    super::failed_mount_teardown::init_log();
    for stage in ["gate-cannot-sleep", "gate-publish-refused", "gate-kill-claimed"] {
        let (fs, disk, sb, ptr) = fixture();
        let receipt = VirtioReadFailure {
            reason: "request gate refused before issue", stage, device: 0,
            sector: ptr.addr * 8, bytes: NODE_SIZE, thread: Some(73), preempt_count: 1, kill_custody: Some(1),
            in_interrupt: false, interrupts_enabled: true, timer_ready: true,
            gate_locked: true, gate_wedged: false, pending_token: 17, completed_token: 0, waiter_thread: 0,
            completed_desc: u32::MAX, completed_status: u32::MAX,
            used_idx: 9, last_used_idx: 9, device_status: 15,
        };
        let writes = disk.writes.load(Ordering::Relaxed);
        *disk.read_error.lock().unwrap() = Some(BlockError::VirtioRead(receipt));
        let cache = BufferCache::new(NoWait);
        let store = CachedNodeStore::new(&cache, disk.as_ref(), &sb);
        let error = store.read_node(ptr).err().expect("injected refusal succeeded");
        assert_eq!(fs.inner.state.lock().observed::<()>(Err(error.into())), Err(Errno(EAGAIN)), "{stage}");
        assert_eq!(fs.failure(), None, "{stage}");
        assert_eq!(fs.read_only(), Ok(false), "{stage}");
        assert_eq!(disk.writes.load(Ordering::Relaxed), writes);
        let mut report = String::new();
        fs.report(Report::State, "/", &mut report).unwrap();
        assert!(report.contains("metadata_cause=io-context"), "{report}");
        assert!(report.contains(&alloc::format!("virtio_stage={stage}")), "{report}");
        assert!(super::failed_mount_teardown::messages().iter().any(|line|
            line.contains("metadata read refused") && line.contains(&alloc::format!("virtio_stage={stage}"))),
            "non-latching refusal lost its serial receipt: {stage}");
        // Duplicate propagation of this same failure must not flood serial.
        assert_eq!(fs.inner.state.lock().observed::<()>(Err(error.into())), Err(Errno(EAGAIN)));
        assert_eq!(super::failed_mount_teardown::messages().iter().filter(|line|
            line.contains("metadata read refused") && line.contains(&alloc::format!("virtio_stage={stage}"))).count(), 1);
    }
}
