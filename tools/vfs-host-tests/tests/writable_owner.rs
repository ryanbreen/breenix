//! E24 ledger 309 C4 / 314: one writer, claimed before recovery touches storage.
use crate::{
    block::{cache::BufferCache, BlockDevice, BlockError},
    breenfs::{
        commit::{Committer, DeviceStore},
        format::Superblock,
        ops::Files,
        vfs::BreenFs,
    },
    errno::Errno,
    read_conformance::{Disk, NoWait},
    syscall_errno::*,
    vfs::FileSystem,
};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc, Mutex,
};
use std::time::Duration;

type Pause = (Sender<()>, Receiver<()>);
struct Device {
    disk: Arc<Disk>,
    calls: AtomicUsize,
    fail: AtomicBool,
    pause: Mutex<Option<Pause>>,
}
impl Device {
    fn new(disk: Arc<Disk>) -> Arc<Self> {
        Arc::new(Self {
            disk,
            calls: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
            pause: Mutex::new(None),
        })
    }
}
impl BlockDevice for Device {
    fn identity(&self) -> usize { self.disk.identity() }
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let pause = self.pause.lock().unwrap().take();
        if let Some((entered, resume)) = pause {
            entered.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        if self.fail.load(Ordering::SeqCst) { return Err(BlockError::IoError); }
        self.disk.read_block(block, bytes)
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.disk.write_block(block, bytes)
    }
    fn block_size(&self) -> usize {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.disk.block_size()
    }
    fn num_blocks(&self) -> u64 {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.disk.num_blocks()
    }
    fn flush(&self) -> Result<(), BlockError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.disk.flush()
    }
}
fn fixture() -> Arc<Disk> {
    let disk = Arc::new(Disk {
        bytes: Mutex::new(vec![0; 64 * 1024 * 1024]),
        writable: true,
    });
    let sb = Superblock {
        fs_uuid: [0xc4; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        global_reserve: 4096,
        next_root_id: 2,
        ..Superblock::default()
    };
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    disk
}
fn writer(device: Arc<Device>) -> Result<Arc<BreenFs<NoWait>>, Errno> {
    BreenFs::mount_writable(device, Arc::new(BufferCache::new(NoWait)), 424, 1)
}

#[test]
fn boot_writer_excludes_explicit_reopen_before_device_access() {
    let disk = fixture();
    // Boot uses a borrowed system cache; /dev/disk creates a fresh wrapper.
    let cache = Box::leak(Box::new(BufferCache::new(NoWait)));
    let boot = BreenFs::mount_writable_cached(Device::new(disk.clone()), cache, 423, 1).unwrap();
    let explicit = Device::new(disk.clone());
    let result = writer(explicit.clone());
    assert_eq!(result.err(), Some(Errno(EBUSY)));
    assert_eq!(explicit.calls.load(Ordering::SeqCst), 0, "loser entered recovery");
    // A read-only observer has no writer and may coexist (guest durability cases).
    let reader = BreenFs::mount(Device::new(disk.clone()), Arc::new(BufferCache::new(NoWait)), 425, 1).unwrap();
    assert_eq!(reader.root(), Ok(2));
    boot.shutdown("unmount", u64::MAX).unwrap();
    // Custody may refuse unmount after shutdown. Keep ownership until removal
    // drops the mount; restarting that same mount must remain safe.
    assert_eq!(writer(explicit.clone()).err(), Some(Errno(EBUSY)));
    boot.init_service().unwrap();
    drop(boot);
    assert!(writer(explicit).is_ok(), "unmount must release ownership");
}

#[test]
fn failed_mount_releases_its_claim() {
    let device = Device::new(fixture());
    device.fail.store(true, Ordering::SeqCst);
    assert_eq!(writer(device.clone()).err(), Some(Errno(EIO)));
    device.fail.store(false, Ordering::SeqCst);
    // Also fail after recovery, while selecting a nonexistent root.
    let missing = BreenFs::mount_writable(device.clone(), Arc::new(BufferCache::new(NoWait)), 424, 999);
    assert!(missing.is_err());
    assert!(writer(device).is_ok());
}

#[test]
fn racing_mount_loses_while_winner_is_still_in_recovery() {
    let disk = fixture();
    let first = Device::new(disk.clone());
    let second = Device::new(disk);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    *first.pause.lock().unwrap() = Some((entered_tx, resume_rx));
    let thread = std::thread::spawn(move || writer(first));
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let losing = writer(second.clone());
    // Resume/join before asserting so a red check leaves no paused thread.
    resume_tx.send(()).unwrap();
    let winning = thread.join().unwrap();
    assert!(winning.is_ok());
    assert_eq!(losing.err(), Some(Errno(EBUSY)));
    assert_eq!(second.calls.load(Ordering::SeqCst), 0, "racing loser touched storage");
}
