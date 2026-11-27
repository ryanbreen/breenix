//! E24 ledger 309 C2: durability through mapping entry points, not direct
//! adapter writeback calls. Host pages replace only frames and page tables.
use super::*;
use alloc::vec;
use std::sync::Mutex as HostMutex;
use std::time::{Duration, Instant};

const WAIT_LIMIT: Duration = Duration::from_secs(5);

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
const MAPPED_TAG: [u8; 4] = [0xd3, 0xa6, 0xb7, 0x5e];
const MAPPED_SHAPE: [usize; 8] = [1, 1, 1, 1, 1000, 1000, 1000, 1000];
const MAPPED_TOTAL: usize = 4004;

struct Disk(HostMutex<Vec<u8>>, HostMutex<Vec<usize>>);
impl Disk {
    fn new() -> Self {
        Self(HostMutex::new(vec![0; 64 * 1024 * 1024]), HostMutex::new(Vec::new()))
    }
}

impl BlockDevice for Disk {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        bytes.copy_from_slice(&self.0.lock().unwrap()[start..start + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        self.0.lock().unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
        if bytes.starts_with(&MAPPED_TAG) {
            let page = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
            self.1.lock().unwrap()[page] += 1;
        }
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
    let disk = Arc::new(Disk::new());
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

// #3208: mmap populates the cache, then publishes its binding under the mount
// guard. A worker eviction pass in that gap, requested when an earlier mapping
// of the same file was unmapped, must not retire the populated pages: a fault
// on a bound page that is not cached maps a zero page, so the loader read an
// all-zero ELF header from librustc_driver.
#[test]
fn populated_pages_survive_eviction_until_the_binding_is_published() {
    use crate::memory::file_map::{bind_host, bind_populated, populate};
    let (fs, _disk, node) = fixture();
    let bytes = expected(0x7f);
    let length = bytes.len() as u64;
    let req = WriteReq { at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false };
    assert_eq!(fs.write(&node, req, &bytes).unwrap().bytes, bytes.len());
    fs.fsync(&node, false).unwrap();
    // An earlier process mapped the file and exited: its unmap leaves the
    // cached pages unbound and asks the worker to retire them.
    drop(bind_host(node.clone(), length, false, false, true));
    assert!(node.map.eviction_pending());
    let Ok(population) = populate(&node, 0, length / 4096) else {
        panic!("population failed");
    };
    // The worker's pass runs before the new binding is published.
    fs.commit_due(0).unwrap();
    let binding = bind_populated(node.clone(), &*population, length);
    let seen: Vec<u8> = (0..length).map(|offset| binding.load(offset).unwrap()).collect();
    let zero_pages = seen.chunks(4096).zip(bytes.chunks(4096))
        .filter(|(seen, want)| seen != want && seen.iter().all(|byte| *byte == 0)).count();
    assert!(seen == bytes, "the new mapping read {zero_pages} evicted pages as zeros");
    // Once mapping and population are gone the pages are retired as before.
    drop(binding);
    drop(population);
    for _ in 0..2 {
        fs.commit_due(0).unwrap();
    }
    assert_eq!(node.map.cache_counts(), (0, 0, 0), "a finished population kept its pages");
    fs.shutdown("population-eviction-cleanup", u64::MAX).unwrap();
}

fn mapped_batch_fixture() -> (Arc<BreenFs<NoWait>>, Arc<Disk>, Vec<Arc<VNode>>) {
    let disk = Arc::new(Disk::new());
    let sb = Superblock { fs_uuid: [0x84; 16], generation: 1, slot: 1,
        total_blocks: 16384, global_reserve: 4096, next_root_id: 2,
        ..Superblock::default() };
    let mut files = Files::format(super::super::commit::DeviceStore(disk.as_ref()), &sb).unwrap();
    files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    drop(files);
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 84, 1).unwrap();
    fs.init_service().unwrap();
    let mut nodes = Vec::new();
    for (index, pages) in MAPPED_SHAPE.into_iter().enumerate() {
        let ino = fs.create(2, format!("map-{index}").as_bytes(), NewNode::Regular {
            attrs: InodeAttrs::new(0o100600, 0, 0),
        }).unwrap();
        let node = fs.open(ino).unwrap();
        fs.resize(&node, (pages * BLOCK_SIZE) as u64, false, &|_| Ok(())).unwrap();
        nodes.push(node);
    }
    fs.fsync(&nodes[0], false).unwrap();
    (fs, disk, nodes)
}

// E24 coordinator ledger 558(x4): one publication for a whole worker pass,
// including passes crossing mapping boundaries; v's eviction repair stands.
#[test]
fn mapped_4004_pages_batch_publication_and_settle_within_page_budget() {
    use crate::memory::file_map::{bind_host, EVICT_BUDGET};
    const SHAPE: [usize; 8] = MAPPED_SHAPE;
    const TOTAL: usize = MAPPED_TOTAL;
    assert_eq!(SHAPE.iter().sum::<usize>(), TOTAL);
    let started = std::time::Instant::now();
    let (fs, disk, nodes) = mapped_batch_fixture();
    let maps: Vec<_> = nodes.iter().map(|node| node.map.clone()).collect();
    let weak: Vec<_> = nodes.iter().map(Arc::downgrade).collect();
    let inos: Vec<_> = nodes.iter().map(|node| node.ino).collect();
    let counts = || maps.iter().fold((0, 0, 0), |sum, map| {
        let n = map.cache_counts(); (sum.0 + n.0, sum.1 + n.1, sum.2 + n.2)
    });
    let baseline = counts();
    assert_eq!(baseline, (0, 0, 0));
    for (index, (node, pages)) in nodes.iter().zip(SHAPE).enumerate() {
        let binding = bind_host(node.clone(), (pages * BLOCK_SIZE) as u64, true, true, true);
        for page in 0..pages {
            binding.store((page * BLOCK_SIZE) as u64, (index + 1) as u8).unwrap();
        }
        drop(binding); // Production Binding::drop requests writeback/eviction.
    }
    drop(nodes); // Close all external VNode handles before settle.
    assert_eq!(counts(), (TOTAL, TOTAL, 0));
    measurement::take_work();
    // Each page needs one writeback and one eviction, sharing EVICT_BUDGET.
    let max_passes = (2 * TOTAL).div_ceil(EVICT_BUDGET);
    let mut publications = Vec::new();
    let mut remaining = counts();
    for pass in 1..=max_passes {
        let before = remaining;
        fs.commit_due(0).unwrap(); // Production worker pass, no foreground sync.
        let work = measurement::take_work();
        remaining = counts();
        publications.push(work[2]);
        let consumed = before.0 - remaining.0 + before.1 - remaining.1;
        eprintln!("mapped-batch pass={pass} publications={} resident={} dirty={} consumed={consumed}",
            work[2], remaining.0, remaining.1);
        assert!(consumed <= EVICT_BUDGET, "worker exceeded its shared page budget");
        assert_eq!(fs.read_only(), Ok(false), "healthy mount latched during settle");
        if remaining == baseline { break; }
        assert!(consumed > 0, "worker lost writeback/eviction progress");
    }
    eprintln!("mapped-batch total={TOTAL} passes={} bound={max_passes} publications={} max_per_pass={} seconds={:.3}",
        publications.len(), publications.iter().sum::<u64>(), publications.iter().max().unwrap(),
        started.elapsed().as_secs_f64());
    assert_eq!(remaining, baseline, "cache exceeded budget-derived settle bound");
    assert!(weak.iter().all(|node| node.upgrade().is_none()), "closed VNode retained by mapping registry");
    // Independent durable readback makes discarding dirty pages fail too.
    let observer = BreenFs::mount(disk, Arc::new(BufferCache::new(NoWait)), 85, 1).unwrap();
    for (index, (ino, pages)) in inos.into_iter().zip(SHAPE).enumerate() {
        let node = observer.open(ino).unwrap();
        let mut bytes = vec![0xff; pages * BLOCK_SIZE];
        assert_eq!(observer.read(&node, 0, &mut bytes).unwrap(), bytes.len());
        for page in bytes.chunks_exact(BLOCK_SIZE) {
            assert_eq!(page[0], (index + 1) as u8);
            assert!(page[1..].iter().all(|byte| *byte == 0));
        }
    }
    assert!(publications.iter().all(|count| *count <= 1),
        "worker published per mapping/chunk instead of once per pass: {publications:?}");
    fs.shutdown("mapped-batch-cleanup", u64::MAX).unwrap();
}

#[derive(Clone, Copy, Debug)]
enum FreezeCut {
    AfterStaging,
    BeforeInstall,
    AfterInstall,
}

/// Park a caller at the exact production edge and let its peer run. The
/// publication edges here are outside mount exclusion.
fn pause_at(
    select: impl Fn(super::super::preempt::context::Event) -> bool + 'static,
    entered: std::sync::mpsc::Sender<()>,
    resume: std::sync::mpsc::Receiver<()>,
    mut wait_for_worker: impl FnMut(Duration) + 'static,
) {
    let mut entered = Some(entered);
    let mut wait_deadline = None;
    super::super::preempt::context::observer(Some(Box::new(move |event| {
        if matches!(event, super::super::preempt::context::Event::CommitWait { .. }) {
            let deadline = wait_deadline.get_or_insert_with(|| Instant::now() + WAIT_LIMIT);
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "commit receipt wait exceeded its deadline");
            wait_for_worker(remaining);
        }
        if select(event) {
            if let Some(entered) = entered.take() {
                entered.send(()).unwrap();
                resume.recv_timeout(WAIT_LIMIT).expect("publication edge was not released");
            }
        }
    })));
}

fn interleave_pressure<T>(fs: &BreenFs<NoWait>, cut: FreezeCut, inspect: impl FnOnce() -> T) -> T {
    use super::super::preempt::context::Event;
    use std::{sync::mpsc::{channel, RecvTimeoutError, Sender}, thread};
    let (worker_entered, at_worker_edge) = channel();
    let (resume_worker, worker_resume) = channel();
    let (pressure_entered, at_pressure_edge) = channel();
    let (resume_pressure, pressure_resume) = channel();
    let (initial_pass, initial_result) = channel();
    let (run_worker, worker_requests) = channel::<Sender<Result<bool, Errno>>>();
    let start = fs.inner.state.lock().durable.generation;

    thread::scope(move |scope| {
        let (start_worker, worker_start) = channel();
        let worker = scope.spawn(move || {
            worker_start.recv_timeout(WAIT_LIMIT).expect("worker was not started");
            pause_at(move |event| match cut {
                FreezeCut::AfterStaging => matches!(event, Event::MappingsStaged),
                FreezeCut::AfterInstall => matches!(event, Event::Phase {
                    op: "commit", phase: "publication", begin: false,
                }),
                FreezeCut::BeforeInstall => false,
            }, worker_entered, worker_resume, |_| {});
            initial_pass.send(fs.commit_due(0)).unwrap();
            // The first pass witnesses the cut. Keep the same worker alive
            // for any later reservation-pressure request from the creator.
            loop {
                match worker_requests.recv_timeout(WAIT_LIMIT) {
                    Ok(done) => done.send(fs.commit_due(0)).unwrap(),
                    Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => panic!("pressure create did not finish"),
                }
            }
        });
        let (start_pressure, pressure_start) = channel();
        let pressure = scope.spawn(move || {
            pressure_start.recv_timeout(WAIT_LIMIT).expect("pressure create was not started");
            // Leave retained metadata for the real reservation-pressure path
            // even when this caller starts after the worker's own install.
            fs.create(2, b"pressure", NewNode::Regular {
                attrs: InodeAttrs::new(0o100600, 0, 0),
            }).unwrap();
            pause_at(move |event| match cut {
                FreezeCut::BeforeInstall => matches!(event, Event::Phase {
                    op: "commit", phase: "write-cached", begin: false,
                }),
                _ => matches!(event, Event::Phase {
                    op: "commit", phase: "apply-touches", begin: true,
                }),
            }, pressure_entered, pressure_resume, move |remaining| {
                let (done, completed) = channel();
                run_worker.send(done).expect("pressure commit worker exited");
                assert_eq!(
                    completed.recv_timeout(remaining).expect("pressure commit worker did not publish"),
                    Ok(true),
                );
            });
            let state = fs.mutation_lock(CostOp::Create);
            let result = fs.reserve_with_pressure(
                CostOp::Create, state, || Ok(fs.mutation_lock(CostOp::Create)), |state| {
                    state.reserve_blocks(u64::MAX, Operation::Allocate)
                },
            );
            assert!(matches!(result, Err(Errno(ENOSPC))));
        });

        if !matches!(cut, FreezeCut::BeforeInstall) {
            start_worker.send(()).unwrap();
            at_worker_edge.recv_timeout(WAIT_LIMIT).expect("worker did not reach its publication edge");
        }
        start_pressure.send(()).unwrap();
        at_pressure_edge.recv_timeout(WAIT_LIMIT).expect("pressure create did not reach its publication edge");
        {
            let state = fs.inner.state.lock();
            assert!(state.pending.is_some(), "pressure did not freeze a generation");
            let worker_installed = u64::from(matches!(cut, FreezeCut::AfterInstall));
            assert_eq!(state.durable.generation, start + worker_installed);
        }
        if matches!(cut, FreezeCut::BeforeInstall) {
            start_worker.send(()).unwrap();
        } else {
            resume_worker.send(()).unwrap();
        }
        assert_eq!(
            initial_result.recv_timeout(WAIT_LIMIT).expect("worker did not finish its first pass"),
            Ok(matches!(cut, FreezeCut::AfterInstall)),
        );
        let observed = inspect();
        resume_pressure.send(()).unwrap();
        pressure.join().unwrap();
        worker.join().unwrap();
        observed
    })
}

#[test]
fn mapped_writeback_waits_for_install_without_self_waking() {
    use crate::memory::file_map::EVICT_BUDGET;

    // E24 ledger 613(z4), review M1 and m1. The pending generation owns a
    // full pass of snapshots, with more dirty pages behind them.
    let (fs, disk, node) = fixture();
    let bytes = expected(17);
    let pages = bytes.len() / BLOCK_SIZE;
    assert!(pages > EVICT_BUDGET);
    node.map.shared_stores(&bytes, false);
    node.map.request_writeback();

    let (passes, pending, requested) = interleave_pressure(&fs, FreezeCut::AfterStaging, || {
        let passes = [(); 3].map(|()| measurement::service_at(&fs, 0).unwrap());
        (passes, node.map.writeback_pending(), fs.inner.signal.requested.load(Ordering::Acquire))
    });
    assert!(pending, "install still owes completion for the frozen snapshots");
    assert_eq!((passes, requested), ([(false, false); 3], false),
        "blocked passes published, woke themselves, or requested another publication");

    let (published, _) = measurement::service_at(&fs, 0).unwrap();
    assert!(!published, "completing installed receipts alone caused another publication");
    assert_eq!(node.map.cache_counts().1, pages - EVICT_BUDGET);
    assert!(measurement::service_at(&fs, 0).unwrap().0);
    assert_eq!(node.map.cache_counts().1, 0);
    assert!(cold_bytes(&disk, node.ino) == bytes, "cold mapped bytes differ after install");
}

#[test]
fn clean_retirement_does_not_wake_a_create_waiting_for_install() {
    use crate::memory::file_map::{bind_host, EVICT_BUDGET};
    use std::{cell::Cell, rc::Rc, sync::mpsc::channel, thread, time::Duration};
    use super::super::preempt::context::{self, Event};

    let (fs, _, node) = fixture();
    let map = node.map.clone();
    let pages = EVICT_BUDGET + 3;
    drop(bind_host(node.clone(), (pages * BLOCK_SIZE) as u64, false, false, true));
    assert_eq!(map.cache_counts(), (pages, 0, 0));
    fs.setattr(node.ino, SetAttr { mode: Some(0o644), ..SetAttr::default() }).unwrap();
    drop(node);
    // Fill the reservation ledger with a retained grant so an actual
    // open(O_CREAT) must wait until this generation retires its grants.
    let generation = {
        let mut state = fs.inner.state.lock();
        let free = state.durable.free_blocks;
        let available = state.reservations.available(free);
        assert!(available > 0);
        let grant = state.reservations.reserve(free, available, Operation::Allocate).unwrap();
        state.grants.push(grant);
        state.open_generation()
    };
    let report = |fs: &BreenFs<NoWait>| {
        let mut text = alloc::string::String::new();
        fs.mutation_costs("root", &mut text).unwrap();
        text
    };
    let excluded = |text: &str| {
        text.lines().find(|line| line.contains(" op=commit phase=mount-excluded "))
            .and_then(|line| line.split_whitespace().find_map(|field| field.strip_prefix("started=")))
            .map_or(0, |count| count.parse::<usize>().unwrap())
    };
    let waits = Rc::new(Cell::new(0));
    let worker_spans = Rc::new(Cell::new(0));
    thread::scope(|scope| {
        let (frozen_tx, frozen) = channel();
        let (resume, resume_rx) = channel();
        let (installed_tx, installed) = channel();
        let publisher = fs.clone();
        let watchdog = Duration::from_secs(5);
        scope.spawn(move || {
            context::observer(Some(Box::new(move |event| {
                if matches!(event, Event::Phase {
                    op: "commit", phase: "apply-touches", begin: true,
                }) {
                    frozen_tx.send(()).unwrap();
                    resume_rx.recv_timeout(watchdog).unwrap();
                }
            })));
            installed_tx.send(publisher.commit_generation_up_to(0, generation, true)).unwrap();
        });
        frozen.recv_timeout(watchdog).unwrap();
        let before = excluded(&report(&fs));
        let (worker, count, spans) = (fs.clone(), waits.clone(), worker_spans.clone());
        let mut remaining_passes = 2;
        context::observer(Some(Box::new(move |event| {
            let Event::CommitWait { epoch, .. } = event else { return };
            count.set(count.get() + 1);
            while remaining_passes > 0 {
                let (published, self_woke) = measurement::service_at(&worker, 0).unwrap();
                remaining_passes -= 1;
                assert!(!published);
                let remaining_pages = if remaining_passes == 1 { 3 } else { 0 };
                assert_eq!(map.cache_counts(), (remaining_pages, 0, 0));
                assert_eq!(self_woke, remaining_passes == 1);
                assert_eq!(worker.inner.state.lock().durable.generation + 1, generation);
                // The kernel's checked park declines to sleep when this
                // epoch changes. Let the same create retry if that happens.
                if worker.inner.signal.epoch() != epoch {
                    return;
                }
            }
            spans.set(excluded(&report(&worker)) - before);
            resume.send(()).unwrap();
            assert_eq!(installed.recv_timeout(watchdog).unwrap(), Ok(true));
            assert_ne!(worker.inner.signal.epoch(), epoch, "install left the create asleep");
        })));
        let mut opened = None;
        fs.create_open(2, b"waiting-create", NodeKind::Regular,
            &|_| Ok(InodeAttrs::new(0o644, 0, 0)),
            &mut |node| { opened = Some(node); Ok(()) }).unwrap();
        context::observer(None);
        assert_eq!(fs.lookup(2, b"waiting-create"), Ok(opened.unwrap().ino));
    });
    let refusals = report(&fs);
    let receipts: Vec<_> = refusals.lines()
        .filter(|line| line.starts_with("admission_refusal ") && line.contains(" op=create "))
        .collect();
    let named = alloc::format!("release_generation={generation}");
    assert!(receipts.iter().all(|line| {
        line.split_whitespace().any(|field| field == named)
    }));
    assert_eq!((waits.get(), receipts.len(), worker_spans.get()), (1, 1, 0),
        "clean eviction disturbed a parked create (waits, receipts, excluded commit spans)");
}

#[test]
fn gui_reset_retires_exited_child_cache_during_pressure_freeze() {
    use crate::memory::file_map::{bind_host, EVICT_BUDGET};

    // E24 ledger 621(y1). Model the supervisor's child and collector
    // (libuitest supervisor_runtime.rs), and the shared writable evidence
    // arena and read-side mapping (libuitest arena.rs). Drop bindings when
    // their process roots retire, then unlink the scratch arena at RESET.
    let (fs, _, arena) = fixture();
    let image = fs.create(2, b"app-image", NewNode::Regular {
        attrs: InodeAttrs::new(0o100700, 0, 0),
    }).unwrap();
    let image = fs.open(image).unwrap();
    let image_pages = 4;
    fs.resize(&image, image_pages * BLOCK_SIZE as u64, false, &|_| Ok(())).unwrap();
    fs.write(&image, WriteReq {
        at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false,
    }, &vec![0x61; image_pages as usize * BLOCK_SIZE]).ok().unwrap();
    fs.fsync(&image, false).unwrap();

    let weak = [Arc::downgrade(&arena), Arc::downgrade(&image)];
    let arena_ino = arena.ino;
    let counts = || (weak.each_ref().map(|node| node.upgrade().map_or((0, 0, 0), |node| node.map.cache_counts())),
        weak.iter().filter(|node| node.strong_count() != 0).count());
    let baseline = counts().0;
    assert_eq!(baseline, [(0, 0, 0); 2]);

    let child_image = bind_host(image.clone(), image_pages * BLOCK_SIZE as u64, false, false, true);
    let arena_len = 67 * BLOCK_SIZE as u64;
    let child_arena = bind_host(arena.clone(), arena_len, true, true, true);
    for page in 0..67 {
        child_arena.store(page * BLOCK_SIZE as u64, 0x59).unwrap();
    }
    let collector = bind_host(arena.clone(), arena_len, true, false, false);
    assert_eq!(child_image.load(0), Ok(0x61));
    assert_eq!(collector.load(66 * BLOCK_SIZE as u64), Ok(0x59));
    drop(image);
    drop(child_arena);
    drop(collector);
    // The exited child's image binding stays with the retiring root while
    // the worker stages the arena. Retire it during pressure preparation.
    let during_freeze = interleave_pressure(&fs, FreezeCut::AfterStaging, || {
        drop(child_image);
        fs.remove(2, b"mapped", Remove {
            directory: false, expected: arena_ino, owner: 0, parent: fs.getattr(2).unwrap().attrs,
        }).unwrap();
        fs.commit_due(0).unwrap();
        // Ruling 630 releases a closed orphan immediately. Keep the
        // collector descriptor so this cut still owes durable writeback.
        let observed = counts();
        drop(arena);
        observed
    });

    // Two budgets per arena page (stage + evict), plus the freeze retry.
    let limit = (2 * 67 + image_pages as usize).div_ceil(EVICT_BUDGET) + 1;
    let mut passes = 0;
    while counts() != (baseline, 0) && passes < limit {
        fs.commit_due(0).unwrap();
        passes += 1;
    }
    eprintln!("gui-reset frozen={during_freeze:?} settled={:?} passes={passes}", counts());
    assert_eq!(counts(), (baseline, 0), "RESET left cache, bindings, dirty pages or pins");
    assert_eq!(during_freeze.0[0], (67, 67, 0), "frozen arena still owes publication");
    assert_eq!((during_freeze.0[1], during_freeze.1), (baseline[1], 1),
        "retrying frozen arena receipts stranded the clean exited child's image and pin");
}

#[test]
fn mapped_writeback_survives_each_pressure_freeze_cut() {
    use crate::memory::file_map::{bind_host, EVICT_BUDGET};
    let mut failures = Vec::new();
    for cut in [FreezeCut::AfterStaging, FreezeCut::BeforeInstall, FreezeCut::AfterInstall] {
        let (fs, disk, nodes) = mapped_batch_fixture();
        *disk.1.lock().unwrap() = vec![0; MAPPED_TOTAL];
        let maps: Vec<_> = nodes.iter().map(|node| node.map.clone()).collect();
        let weak: Vec<_> = nodes.iter().map(Arc::downgrade).collect();
        let inos: Vec<_> = nodes.iter().map(|node| node.ino).collect();
        let counts = || maps.iter().fold((0, 0, 0), |(resident, dirty, bindings), map| {
            let n = map.cache_counts();
            (resident + n.0, dirty + n.1, bindings + n.2)
        });
        let baseline = counts();
        let header = |page: usize| {
            let [lo, hi] = (page as u16).to_le_bytes();
            [MAPPED_TAG[0], MAPPED_TAG[1], MAPPED_TAG[2], MAPPED_TAG[3], lo, hi]
        };
        let mut page_id = 0;
        for (node, pages) in nodes.iter().zip(MAPPED_SHAPE) {
            let binding = bind_host(node.clone(), (pages * BLOCK_SIZE) as u64, true, true, true);
            for page in 0..pages {
                for (i, byte) in header(page_id).into_iter().enumerate() {
                    binding.store((page * BLOCK_SIZE + i) as u64, byte).unwrap();
                }
                page_id += 1;
            }
        }
        drop(nodes);
        assert_eq!(counts(), (MAPPED_TOTAL, MAPPED_TOTAL, 0));
        let (after_cut, retry) = interleave_pressure(&fs, cut, || {
            (counts(), maps[..4].iter().filter(|map| map.writeback_pending()).count())
        });
        let installing_mapped_pages = matches!(cut, FreezeCut::AfterInstall);
        let expected_dirty = if installing_mapped_pages { MAPPED_TOTAL - EVICT_BUDGET } else { MAPPED_TOTAL };
        let expected_retry = if installing_mapped_pages { 0 } else { 4 };
        let mut passes = 1;
        // Two budgets per page (snapshot, then eviction), plus one pass whose
        // completion the in-flight pressure publication can put off.
        let limit = (2 * MAPPED_TOTAL).div_ceil(EVICT_BUDGET) + 1;
        while counts() != baseline && passes < limit {
            fs.commit_due(0).unwrap();
            passes += 1;
        }
        let settled = counts();
        let pinned = weak.iter().filter(|node| node.upgrade().is_some()).count();
        let writes = disk.1.lock().unwrap();
        let missing = writes.iter().filter(|&&count| count == 0).count();
        let repeated = writes.iter().filter(|&&count| count > 1).count();
        drop(writes);
        let observer = BreenFs::mount(disk, Arc::new(BufferCache::new(NoWait)), 85, 1).unwrap();
        let mut mismatches = 0;
        let mut page_id = 0;
        for (ino, pages) in inos.into_iter().zip(MAPPED_SHAPE) {
            let node = observer.open(ino).unwrap();
            let mut bytes = vec![0xff; pages * BLOCK_SIZE];
            assert_eq!(observer.read(&node, 0, &mut bytes).unwrap(), bytes.len());
            for page in bytes.chunks_exact(BLOCK_SIZE) {
                if page[..6] != header(page_id) || page[6..].iter().any(|&byte| byte != 0) {
                    mismatches += 1;
                }
                page_id += 1;
            }
        }
        eprintln!("cut={cut:?} after_cut={after_cut:?} retry={retry} settled={settled:?} passes={passes}/{limit} pinned={pinned} missing={missing} repeated={repeated} mismatches={mismatches}");
        if after_cut != (MAPPED_TOTAL, expected_dirty, 0)
            || retry != expected_retry
            || settled != baseline
            || pinned != 0
            || missing != 0
            || repeated != 0
            || mismatches != 0
        {
            failures.push(cut);
        }
        assert_eq!(fs.read_only(), Ok(false));
        fs.shutdown("mapped-freeze-cleanup", u64::MAX).unwrap();
    }
    assert!(failures.is_empty(), "writeback failures at pressure cuts: {failures:?}");
}

// E24 coordinator ledger 630(a1). The session arena has three 4 KiB
// headers, two optional 2560x1600 BGRX slots and a 1 MiB tree. The
// supervisor and case share its inode until checkpoint replacement or RESET.
#[test]
fn reset_releases_surface_arenas_during_worker_publication() {
    use crate::memory::file_map::{bind_host, EVICT_BUDGET};
    use super::super::preempt::context::Event;
    use std::{sync::mpsc::channel, thread, time::Duration};

    const SURFACE: usize = 2560 * 1600 * 4 / BLOCK_SIZE;
    const TREE: usize = 1024 * 1024 / BLOCK_SIZE;
    const ARENA: usize = 4 + 2 * SURFACE + TREE;
    let (fs, _, marker) = fixture();
    let make = |name: &[u8]| {
        let ino = fs.create(2, name, NewNode::Regular {
            attrs: InodeAttrs::new(0o100600, 0, 0),
        }).unwrap();
        let node = fs.open(ino).unwrap();
        fs.resize(&node, (ARENA * BLOCK_SIZE) as u64, false, &|_| Ok(())).unwrap();
        node
    };
    let unlink = |name: &[u8], ino| fs.remove(2, name, Remove {
        directory: false, expected: ino, owner: 0, parent: fs.getattr(2).unwrap().attrs,
    }).unwrap();

    // Linked cache must obey the existing 64-page budget and keep its
    // request until the last page goes, even without another binding drop.
    let linked = make(b"linked");
    let reader = bind_host(linked.clone(), (ARENA * BLOCK_SIZE) as u64, true, false, false);
    for page in 1..=SURFACE {
        assert_eq!(reader.load((page * BLOCK_SIZE) as u64), Ok(0));
    }
    drop(reader);
    for pass in 1..=SURFACE.div_ceil(EVICT_BUDGET) {
        assert!(linked.map.eviction_pending());
        assert!(fs.inner.signal.mappings.load(Ordering::Acquire));
        fs.commit_due(0).unwrap();
        assert_eq!(linked.map.cache_counts(), (SURFACE.saturating_sub(pass * EVICT_BUDGET), 0, 0));
    }
    assert!(!linked.map.eviction_pending());
    drop(linked);

    let mut at_reset = Vec::new();
    for (session, ranges) in [
        vec![1..ARENA],
        vec![1..ARENA - 9],
        vec![1..SURFACE + 3, 2 * SURFACE + 3..ARENA],
        vec![1..SURFACE + 3, 2 * SURFACE + 3..2 * SURFACE + 4],
    ].into_iter().enumerate() {
        let name = format!("session-{session}.arena");
        let arena = make(name.as_bytes());
        fs.fsync(&arena, false).unwrap();
        let mapping = bind_host(arena.clone(), (ARENA * BLOCK_SIZE) as u64, true, true, false);
        for page in ranges.into_iter().flatten() {
            assert_eq!(mapping.load((page * BLOCK_SIZE) as u64), Ok(0));
        }
        mapping.store(BLOCK_SIZE as u64, 0x59).unwrap();
        let (pages, dirty, bindings) = arena.map.cache_counts();
        assert_eq!((pages, dirty, bindings), ([8259, 8250, 4259, 4003][session], 1, 1));
        let weak = Arc::downgrade(&arena);
        if session == 0 {
            let next = make(b"checkpoint.next");
            fs.rename(2, b"checkpoint.next", 2, name.as_bytes(), RenameMode::Replace,
                &|_, _, _, _| Ok(())).unwrap();
            assert_eq!(fs.getattr(arena.ino).unwrap().nlink, 0);
            assert_eq!(mapping.load(BLOCK_SIZE as u64), Ok(0x59));
            drop(arena); // Arena closes the file before unmapping the payload.
            drop(mapping);
            let next_ino = next.ino;
            drop(next);
            unlink(name.as_bytes(), next_ino);
            at_reset.push((None, weak));
        } else {
            drop(mapping);
            let ino = arena.ino;
            drop(arena);
            at_reset.push((Some((name, ino)), weak));
        }
    }

    let pending = bind_host(marker.clone(), BLOCK_SIZE as u64, true, true, false);
    pending.store(0, 0x91).unwrap();
    drop(pending);
    let (entered, edge) = channel();
    let (resume, released) = channel();
    let during_reset = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            pause_at(|event| matches!(event, Event::Phase {
                op: "commit", phase: "apply-touches", begin: true,
            }), entered, released, |_| {});
            fs.commit_due(0)
        });
        edge.recv_timeout(Duration::from_secs(5)).unwrap();
        for (name, ino) in at_reset.iter().filter_map(|(path, _)| path.as_ref()) {
            unlink(name.as_bytes(), *ino);
        }
        let counts: Vec<_> = at_reset.iter().map(|(_, node)| {
            node.upgrade().map_or((0, 0, 0, 0), |held| {
                let (pages, dirty, bindings) = held.map.cache_counts();
                (pages, dirty, bindings, Arc::strong_count(&held) - 1)
            })
        }).collect();
        resume.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), Ok(true));
        counts
    });
    eprintln!("surface RESET custody (pages, dirty, bindings, pins): {during_reset:?}");
    assert_eq!(during_reset, [(0, 0, 0, 0); 4]);
}
