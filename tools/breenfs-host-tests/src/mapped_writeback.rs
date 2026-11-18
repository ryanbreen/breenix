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
    fs.mapping_io(&node)
        .unwrap()
        .publish(&mut |_| Ok(()))
        .unwrap();
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
    fs.shutdown("c2-fsync-cleanup", u64::MAX).unwrap();
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
    fs.shutdown("c2-sync-cleanup", u64::MAX).unwrap();
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
    // Eviction shares the page budget with writeback, so one final pass
    // retires the remaining clean pages and the service-owned VNode.
    fs.commit_due(monotonic_ns()).unwrap();
    assert!(!map.nonempty(), "unbound mapped cache was not retired");
    fs.shutdown("c2-async-cleanup", u64::MAX).unwrap();
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

#[test]
fn n1_async_mapping_makes_progress_at_full_dirty_budget() {
    let (fs, disk, node) = fixture();
    let bytes = expected(61);
    let other = fs.create(2, b"pressure", NewNode::Regular {
        attrs: InodeAttrs::new(0o100600, 0, 0),
    }).unwrap();
    let other = fs.open(other).unwrap();
    let chunk = vec![0x73; 4096];
    for page in 0..DIRTY_BUDGET / 4096 - 1 {
        fs.write(&other, WriteReq {
            at: WriteAt::Offset(page * 4096), limit: u64::MAX, clear_setid: false,
        }, &chunk).ok().unwrap();
    }
    assert_eq!(fs.dirty_bytes(), DIRTY_BUDGET - 4096);
    node.map.shared_stores(&bytes, false);
    node.map.request_writeback();
    for _ in 0..2 { fs.commit_due(monotonic_ns()).unwrap(); }
    assert_eq!(cold_bytes(&disk, node.ino), bytes);
    fs.fsync(&other, false).unwrap();
    fs.shutdown("n1-pressure", u64::MAX).unwrap();
}

fn allocation_retry(shutdown: bool, size: usize, nth: usize) {
    let (fs, disk, node) = fixture();
    let bytes = expected(79);
    node.map.shared_stores(&bytes, false);
    node.map.request_writeback();
    let result = super::dirty_heap_bound::fail_allocation(size, nth, || {
        if shutdown { fs.shutdown("n3-retry", u64::MAX) }
        else { fs.commit_due(monotonic_ns()).map(|_| ()) }
    });
    if shutdown {
        assert_eq!(result, Err(Errno(ENOMEM)), "temporary allocation failure lost its errno");
    } else {
        assert_eq!(result, Ok(()), "temporary allocation failure stopped the worker");
        assert!(fs.inner.signal.mappings.load(Ordering::Acquire), "worker lost its retry");
    }
    assert_eq!(fs.read_only(), Ok(false), "ENOMEM failed the mount");
    assert!(node.map.has_dirty(), "ENOMEM dropped dirty custody");
    assert!(!fs.inner.signal.stop.load(Ordering::Acquire));
    if shutdown {
        fs.shutdown("n3-retry", u64::MAX).unwrap();
    } else {
        for _ in 0..2 { fs.commit_due(monotonic_ns()).unwrap(); }
    }
    assert_eq!(cold_bytes(&disk, node.ino), bytes, "retry lost mapped stores");
    if !shutdown { fs.shutdown("n3-cleanup", u64::MAX).unwrap(); }
}
#[test]
fn n3_service_retries_node_collection_enomem() {
    // Vec<Arc<VNode>>::try_reserve(1) uses its four-entry minimum capacity.
    allocation_retry(false, 4 * core::mem::size_of::<Arc<VNode>>(), 1);
}
#[test]
fn n3_service_retries_writeback_buffer_enomem() {
    allocation_retry(false, 64 * 4096, 1);
}
#[test]
fn n3_shutdown_retries_writeback_buffer_enomem() {
    allocation_retry(true, 64 * 4096, 1);
}
#[test]
fn n3_shutdown_preserves_staging_enomem() {
    // The first allocation is the reusable snapshot, the second is BreenFS's
    // private writeback copy made by stage_range_with_buffer.
    allocation_retry(true, 64 * 4096, 2);
}

#[test]
fn minor_failed_mapping_fsync_returns_eio() {
    let (fs, _disk, node) = fixture();
    node.map.shared_stores(&expected(83), false);
    fs.inner.state.lock().edited::<()>(&[], Err(ops::Error::EIO)).unwrap_err();
    assert_eq!(fs.fsync(&node, false), Err(Errno(EIO)));
    fs.shutdown("minor-failed-fsync", u64::MAX).unwrap();
}

#[test]
fn minor_stopped_shutdown_retry_releases_mapping_pins() {
    let (fs, _disk, node) = fixture();
    // A timed-out wait for worker exit leaves this state; on retry the
    // worker has stopped but VFS has not yet tested external mount custody.
    {
        let mut state = fs.inner.state.lock();
        assert!(state.nodes.get(&node.ino).unwrap().mapped.is_some());
        state.closed = true;
        state.failed = true;
    }
    fs.inner.signal.stop.store(true, Ordering::Release);
    fs.inner.signal.started.store(false, Ordering::Release);
    fs.shutdown("minor-timeout-retry", u64::MAX).unwrap();
    assert!(fs.inner.state.lock().nodes.values().all(|entry| entry.mapped.is_none()));
}

// E24 coordinator ledger 531(v): healthy cache retirement after UI-style
// shared stores, final munmap and close, crossing the 64-page service budget.
#[test]
fn healthy_cache_returns_to_baseline_after_unmap_and_close() {
    use crate::memory::file_map::{bind_host, EVICT_BUDGET};
    let (fs, disk, first) = fixture();
    fs.resize(&first, 64 * 4096, false, &|_| Ok(())).unwrap();
    let ino = fs.create(2, b"second", NewNode::Regular {
        attrs: InodeAttrs::new(0o100600, 0, 0),
    }).unwrap();
    let second = fs.open(ino).unwrap();
    fs.resize(&second, 36 * 4096, false, &|_| Ok(())).unwrap();
    fs.fsync(&second, false).unwrap();
    let nodes = [first, second];
    let maps = [nodes[0].map.clone(), nodes[1].map.clone()];
    let weak = [Arc::downgrade(&nodes[0]), Arc::downgrade(&nodes[1])];
    let inos = [nodes[0].ino, nodes[1].ino];
    let baseline: usize = maps.iter().map(|m| m.cache_counts().0).sum();
    assert_eq!(baseline, 0);
    for (node, pages) in nodes.iter().zip([64, 36]) {
        let binding = bind_host(node.clone(), pages * 4096, true, true, true);
        for page in 0..pages { binding.store(page * 4096, 0x59).unwrap(); }
        // Bound pages remain observable even when explicitly considered for eviction.
        let mut budget = EVICT_BUDGET;
        assert!(!node.map.evict(&mut budget));
        assert_eq!(budget, EVICT_BUDGET);
        assert_eq!(node.map.cache_counts(), (pages as usize, pages as usize, 1));
        drop(binding); // Compiles Binding::drop from file_map.rs.
    }
    drop(nodes); // Only NodeState.mapped can now retain these VNodes.
    assert!(maps.iter().all(|m| m.cache_counts().2 == 0));
    // Deterministic settle through the production worker, without a foreground
    // fsync/umount. Four budgets cover 100 writes + 100 evictions.
    for pass in 0..4 {
        fs.commit_due(0).unwrap();
        eprintln!("healthy settle pass={pass} cache={:?}", maps.iter()
            .map(|m| (m.cache_counts(), m.eviction_pending(), m.writeback_pending()))
            .collect::<Vec<_>>());
        assert_eq!(fs.read_only(), Ok(false));
    }
    assert!(maps.iter().all(|m| !m.has_dirty()), "settle left dirty pages");
    assert_eq!(maps.iter().map(|m| m.cache_counts().0).sum::<usize>(), baseline,
        "clean unbound pages survived service_mappings settle");
    assert!(weak.iter().all(|n| n.upgrade().is_none()), "NodeState.mapped kept a cache owner");
    // Cache release must follow durable writeback, rather than discard stores.
    let observer = BreenFs::mount(disk, Arc::new(BufferCache::new(NoWait)), 73, 1).unwrap();
    for (ino, pages) in inos.into_iter().zip([64, 36]) {
        let node = observer.open(ino).unwrap();
        let mut bytes = vec![0; pages * 4096];
        assert_eq!(observer.read(&node, 0, &mut bytes).unwrap(), bytes.len());
        for page in bytes.chunks(4096) {
            assert_eq!(page[0], 0x59);
            assert!(page[1..].iter().all(|b| *b == 0));
        }
    }
    fs.shutdown("healthy-cache-cleanup", u64::MAX).unwrap();
}
