//! E24 ledger 309 M1: a latched storage error must not pin mount teardown.
use super::*;
use alloc::{string::String, vec, vec::Vec};
use std::cell::RefCell;
use std::sync::{Mutex as HostMutex, Once};

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("sequential teardown must not wait on cache I/O")
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
// The process-wide sink writes to a buffer owned by the thread that
// requested capture. Assertions read a snapshot after releasing the borrow.
std::thread_local! {
    static MESSAGES: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

struct LossLog;

impl log::Log for LossLog {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        let _ = MESSAGES.try_with(|messages| {
            if let Some(messages) = messages.borrow_mut().as_mut() {
                messages.push(alloc::format!("{}", record.args()));
            }
        });
    }
    fn flush(&self) {}
}
pub(super) fn init_log() {
    static LOG: LossLog = LossLog;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&LOG).unwrap();
        log::set_max_level(log::LevelFilter::Error);
    });
    MESSAGES.with(|messages| *messages.borrow_mut() = Some(Vec::new()));
}
pub(super) fn messages() -> Vec<String> {
    MESSAGES.with(|messages| messages.borrow().as_ref().unwrap().clone())
}
fn fixture() -> Arc<BreenFs<NoWait>> {
    let disk = Arc::new(Disk(HostMutex::new(vec![0; 64 * 1024 * 1024])));
    let sb = Superblock {
        fs_uuid: [0x11; 16],
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
    BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap()
}

#[test]
fn opens_after_abandon_do_not_grow_the_replay_log() {
    let fs = fixture();
    let ino = fs.create(2, b"frozen", NewNode::Regular {
        attrs: InodeAttrs::new(0o644, 0, 0),
    }).unwrap();
    let file = fs.open(ino).unwrap();
    assert_eq!(fs.write(&file, WriteReq {
        at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false,
    }, b"before the failure").unwrap().bytes, 18);
    let frozen = fs.freeze(&mut fs.inner.state.lock(), 0).unwrap();
    drop(fs.open(2).unwrap());
    assert_eq!(fs.abandon(frozen, "prepare-reap", Errno(EIO)), Err(Errno(EIO)));
    assert_eq!(fs.read_only(), Ok(true));

    // Two lifetime changes per open after the first: 32,769 records here.
    // No next view will replay them, and growing one queue past 32,768 272-byte
    // records asks the heap for the 17,825,792 bytes of ledger 763.
    let ((), largest) = super::dirty_heap_bound::largest_allocation(|| {
        for _ in 0..16_385 {
            drop(fs.open(2).unwrap());
        }
    });
    let records = fs.inner.state.lock().files.log.as_ref().map(|log| log.len());
    assert_eq!(records, None, "an abandoned publication still records replay changes");
    assert!(largest < 64 * 1024, "opens on the latched mount allocated a {largest}-byte block");
    let mut readback = [0; 18];
    assert_eq!(fs.read(&file, 0, &mut readback), Ok(18));
    assert_eq!(&readback, b"before the failure");
    assert_eq!(fs.fsync(&file, false), Err(Errno(EIO)));
}

#[test]
fn live_replay_records_use_bounded_pages_and_refuse_with_enomem() {
    // E24 764 (k2): N+1's record while N is frozen. Each record is 272 bytes
    // and a replay page holds 15 of them, one 4,080-byte heap block.
    assert_eq!(core::mem::size_of::<Change>(), 272);
    let fs = fixture();
    let frozen = fs.freeze(&mut fs.inner.state.lock(), 0).unwrap();
    let refused = super::dirty_heap_bound::fail_allocation(15 * 272, 1, || fs.open(2));
    assert_eq!(refused.err(), Some(Errno(ENOMEM)), "a refused replay page must fail the open");
    assert_eq!(fs.read_only(), Ok(false), "a refused replay page latched the mount");
    // 16,383 records, 4,456,448 bytes in one queue before the fix. The page
    // table grows by one 32-byte entry per 4,080-byte page.
    let ((), largest) = super::dirty_heap_bound::largest_allocation(|| {
        for _ in 0..8192 {
            drop(fs.open(2).unwrap());
        }
    });
    let records = fs.inner.state.lock().files.log.as_ref().map(|log| log.len());
    assert_eq!(records, Some(16_383));
    assert!(largest <= 128 * 1024, "replay records allocated a {largest}-byte block");
    assert_eq!(fs.abandon(frozen, "prepare-reap", Errno(EIO)), Err(Errno(EIO)));
}

#[test]
fn clean_inode_lookup_does_not_allocate_a_replay_page() {
    let fs = fixture();
    let node = fs.open(2).unwrap();
    let before = fs.getattr(2).unwrap().times.atime;
    let frozen = fs.freeze(&mut fs.inner.state.lock(), 0).unwrap();

    let (attrs, largest) = super::dirty_heap_bound::largest_allocation(|| fs.getattr(2));
    assert_eq!(attrs.unwrap().times.atime, before);
    assert_eq!(largest, 0, "a clean cached inode needed replay storage");

    // Publication can have sampled the frozen stamp before the lookup, so
    // the atomic timestamp slots can be empty even when a touch needs replay.
    let touched = &node.pin::<NodePin<NoWait>>().unwrap().touched;
    let generation = fs.inner.generation.load(Ordering::Acquire) - 1;
    touched.record(generation, true, 90_001);
    assert_eq!(touched.for_publication(generation).access, 90_001);
    assert!(!touched.pending());
    let refused = super::dirty_heap_bound::fail_allocation(15 * 272, 1, || fs.getattr(2));
    assert_eq!(refused.err(), Some(Errno(ENOMEM)));
    assert_eq!(fs.read_only(), Ok(false));
    assert_eq!(fs.getattr(2).unwrap().times.atime.sec, 90_000);
    assert_eq!(fs.abandon(frozen, "prepare-reap", Errno(EIO)), Err(Errno(EIO)));
}

fn failed_mount(error: ops::Error) {
    init_log();
    let fs = fixture();
    fs.init_service().unwrap();
    let node = fs.open(2).unwrap();
    // Inject at the real multi-item edit completion boundary, before any
    // shutdown: both errors latch failure and fail the commit receipts.
    assert_eq!(
        fs.inner.state.lock().edited::<()>(&[], Err(error)),
        Err(errno(error))
    );
    assert_eq!(fs.fsync(&node, false), Err(Errno(EIO)));
    assert_eq!(fs.read_only(), Ok(true));
    let name = alloc::format!("m1-{error:?}");
    assert_eq!(
        fs.shutdown(&name, u64::MAX),
        Ok(()),
        "failed mount blocks teardown"
    );
    assert!(!fs.inner.signal.started.load(Ordering::Acquire));
    assert!(fs.inner.signal.stop.load(Ordering::Acquire));
    assert!(messages()
        .iter()
        .any(|line| line.contains(&name) && line.contains("uncommitted changes may be lost")));
    // A custody refusal can leave a stopped mount registered. Retrying
    // shutdown must complete, but restarting it cannot revive writes.
    fs.init_service().unwrap();
    assert_eq!(fs.shutdown(&name, u64::MAX), Ok(()));
    assert_eq!(fs.fsync(&node, true), Err(Errno(EIO)));
    assert_eq!(fs.inner.state.lock().writable(), Err(Errno(EROFS)));
}

#[test]
fn m1_latched_eio_teardown_reports_loss_and_keeps_fsync_honest() {
    failed_mount(ops::Error::EIO);
}

#[test]
fn m1_latched_enospc_teardown_reports_loss_and_keeps_fsync_honest() {
    failed_mount(ops::Error::ENOSPC);
}

#[test]
fn m1_shutdown_deadline_still_refuses_completion() {
    let fs = fixture();
    fs.init_service().unwrap();
    assert_eq!(fs.shutdown("m1-deadline", 0), Err(Errno(ETIMEDOUT)));
}

#[test]
fn refused_reap_reservation_preserves_acknowledged_writes() {
    use super::super::{abi::*, reserve};
    let fs = fixture();
    let ino = fs.create(2, b"saved", NewNode::Regular {
        attrs: crate::fs::attrs::InodeAttrs::new(0o644, 0, 0),
    }).unwrap();
    let file = fs.open(ino).unwrap();
    let dir = fs.open(2).unwrap();
    let mut request = SnapshotRequest { flags: SNAPSHOT_READ_ONLY,
        name_len: 5, name: [0; 256] };
    request.name[..5].copy_from_slice(b"trash");
    fs.ioctl(&dir, SNAPSHOT_CREATE, &request as *const _ as u64).unwrap();
    let write = |bytes: &[u8]| fs.write(&file, WriteReq {
        at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false,
    }, bytes).unwrap();
    write(b"before");
    fs.fsync(&file, false).unwrap();
    request.flags = 0;
    fs.ioctl(&dir, SNAPSHOT_DELETE, &request as *const _ as u64).unwrap();
    let dead = fs.inner.state.lock().dead_remaining;
    assert!(dead > 0);
    assert_eq!(write(b"saved!").bytes, 6);
    // Hold the remaining admission budget through this publication, forcing
    // the real optional reaper reservation to refuse after dirty-page drain.
    {
        let mut state = fs.inner.state.lock();
        let free = state.durable.free_blocks;
        let remaining = free - state.reservations.outstanding();
        let grant = state.reservations.reserve(free, remaining,
            reserve::Operation::DeleteSnapshot).unwrap();
        state.grants.push(grant);
    }
    reserve::observation::take();
    assert_eq!(fs.fsync(&file, false), Ok(()));
    assert!(reserve::observation::take().is_some(), "reap grant must refuse");
    assert_eq!(fs.read_only(), Ok(false));
    assert_eq!(fs.inner.state.lock().dead_remaining, dead);
    // The next commit can resume reaping after the held grants are released.
    fs.fsync(&file, false).unwrap();
    assert_eq!(fs.inner.state.lock().dead_remaining, 0);
    let disk = fs.inner.device.clone();
    fs.shutdown("refused-reap", u64::MAX).unwrap();
    drop((file, dir, fs));
    let cold = BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 72, 1).unwrap();
    let mut bytes = [0; 6];
    assert_eq!(cold.read(&cold.open(ino).unwrap(), 0, &mut bytes), Ok(6));
    assert_eq!(&bytes, b"saved!");
}

#[test]
fn log_capture_excludes_peer_receipts_and_survives_peer_panic() {
    init_log();
    let before = messages().len();
    let panic = std::thread::spawn(|| {
        init_log();
        log::error!("unrelated fixture receipt");
        panic!("peer fixture stopped");
    })
    .join()
    .unwrap_err();
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"peer fixture stopped"));

    log::error!("expected fixture receipt");
    let captured = messages();
    assert_eq!(captured.len(), before + 1);
    assert_eq!(captured.last().map(String::as_str), Some("expected fixture receipt"));
}

// Coordinator 485(j): the first failure remains attributable after later
// failures and shutdown. The source census below ties these boundary labels
// to the production error arms; the table exercises their shared recorder.
#[test]
fn failure_latch_records_each_branch_once() {
    init_log();
    let branches = [
        ("observed-metadata", EIO), ("edited", EIO), ("edited", ENOSPC),
        ("write-read-metadata", EIO), ("resize-read-metadata", EIO),
        ("writeback-read-metadata", EIO), ("service-mappings", EIO),
        ("drain-dirty", ENOSPC), ("prepare-reap", EIO),
        ("prepare-files", EIO), ("open-next-generation", ENOSPC),
        ("publish", EIO), ("shutdown-sync", ETIMEDOUT),
    ];
    for (branch, code) in branches {
        let fs = fixture();
        let before = messages().len();
        {
            let mut state = fs.inner.state.lock();
            state.fail(branch, Errno(code));
            state.fail("later", Errno(EIO));
            assert_eq!(state.writable(), Err(Errno(EROFS)));
        }
        assert_eq!(fs.failure(), Some((branch, code)));
        assert_eq!(fs.fsync(&fs.open(2).unwrap(), false), Err(Errno(EIO)));
        let captured = messages();
        assert_eq!(captured.len(), before + 1);
        let log = captured.last().unwrap();
        assert!(log.contains(branch) && log.contains(&alloc::format!("errno={code}")), "{log}");
        let mut options = String::new();
        crate::vfs::mount_table::append_failure_options(&mut options, fs.failure());
        assert_eq!(options, alloc::format!(",failure={branch},failure_errno={code}"));
        fs.shutdown("latched", u64::MAX).unwrap();
        assert_eq!(fs.failure(), Some((branch, code)));
    }
}

#[test]
fn failure_latch_error_arms_are_named() {
    let source = include_str!("../../../kernel/src/fs/breenfs/vfs.rs");
    assert_eq!(source.matches("self.failed = true;").count(), 1);
    assert!(!source.contains("state.failed = true;"));
    for branch in ["observed-metadata", "edited", "service-mappings", "publish", "shutdown-sync"] {
        assert!(source.contains(&alloc::format!(".fail(\"{branch}\",")), "unnamed {branch}");
    }
    let edit_pages = source.split_once("fn edit_pages(").unwrap().1
        .split_once("fn snapshot_request(").unwrap().0;
    assert!(edit_pages.contains("state.fail(branch, Errno(EIO))"));
    for branch in ["write-read-metadata", "resize-read-metadata", "writeback-read-metadata"] {
        assert!(source.contains(&alloc::format!(".edit_pages(&mut state, node.ino, \"{branch}\",")),
            "unnamed {branch}");
    }
    let abandon = source.split_once("fn abandon(").unwrap().1;
    assert!(abandon.split("\n    fn ").next().unwrap().contains("state.fail(branch, error)"));
    for branch in ["drain-dirty", "prepare-reap", "prepare-files", "open-next-generation"] {
        assert!(source.contains(&alloc::format!(".abandon(frozen, \"{branch}\",")), "unnamed {branch}");
    }
    let mounts = include_str!("../../../kernel/src/fs/vfs/mount.rs");
    assert!(mounts.contains("append_failure_options(&mut options, fs.failure())"));
    assert!(mounts.contains("drop(table)"), "backend failure read must release table exclusion");
}

// A device read failure is an I/O error at the filesystem boundary. It must
// stop later mutation; guessing that a timed-out DMA completed is not recovery.
#[test]
fn metadata_io_failure_latches_and_prevents_later_writes() {
    struct FaultDisk {
        bytes: HostMutex<Vec<u8>>,
        fail_reads: core::sync::atomic::AtomicBool,
        failed_reads: AtomicU64,
        writes: AtomicU64,
    }
    impl BlockDevice for FaultDisk {
        fn block_size(&self) -> usize { 512 }
        fn num_blocks(&self) -> u64 { self.bytes.lock().unwrap().len() as u64 / 512 }
        fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
            let start = block as usize * 512;
            bytes.copy_from_slice(&self.bytes.lock().unwrap()[start..start + bytes.len()]);
            Ok(())
        }
        fn read_blocks(&self, block: u64, count: usize, bytes: &mut [u8]) -> Result<(), BlockError> {
            if self.fail_reads.load(Ordering::Acquire) {
                assert_eq!(count, 32, "expected a metadata-node miss");
                self.failed_reads.fetch_add(1, Ordering::Relaxed);
                return Err(BlockError::IoError);
            }
            let start = block as usize * 512;
            bytes.copy_from_slice(&self.bytes.lock().unwrap()[start..start + bytes.len()]);
            Ok(())
        }
        fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
            let start = block as usize * 512;
            self.bytes.lock().unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
            self.writes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn flush(&self) -> Result<(), BlockError> { Ok(()) }
    }
    let disk = Arc::new(FaultDisk { bytes: HostMutex::new(vec![0; 64 * 1024 * 1024]),
        fail_reads: core::sync::atomic::AtomicBool::new(false),
        failed_reads: AtomicU64::new(0), writes: AtomicU64::new(0) });
    let sb = Superblock { fs_uuid: [0x39; 16], generation: 1, slot: 1,
        total_blocks: 16384, global_reserve: 4096, next_root_id: 2,
        ..Superblock::default() };
    let mut files = Files::format(super::super::commit::DeviceStore(disk.as_ref()), &sb).unwrap();
    let mut cold = 0;
    for i in 0..256 {
        let ino = files.create(2, alloc::format!("populated-{i:04}").as_bytes(), 0o644, Time::default()).unwrap();
        // The root inode and mount's ORPHAN scan warm the edge leaves.
        if i == 128 { cold = ino; }
    }
    files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    drop(files);
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 75, 1).unwrap();
    let root = fs.open(2).unwrap();
    disk.fail_reads.store(true, Ordering::Release);
    let writes = disk.writes.load(Ordering::Relaxed);
    assert_eq!(fs.getattr(cold).map(|_| ()), Err(Errno(EIO)));
    assert_eq!(disk.failed_reads.load(Ordering::Relaxed), 1);
    assert_eq!(fs.failure(), Some(("observed-metadata", EIO)));
    assert_eq!(fs.create(2, b"after-failure", NewNode::Regular {
        attrs: InodeAttrs::new(0o644, 0, 0),
    }), Err(Errno(EROFS)));
    assert_eq!(fs.fsync(&root, false), Err(Errno(EIO)));
    assert_eq!(disk.writes.load(Ordering::Relaxed), writes);
}
