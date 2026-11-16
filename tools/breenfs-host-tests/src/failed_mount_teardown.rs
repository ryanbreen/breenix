//! E24 ledger 309 M1: a latched storage error must not pin mount teardown.
use super::*;
use alloc::{string::String, vec, vec::Vec};
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
struct LossLog(HostMutex<Vec<String>>);
static LOG: LossLog = LossLog(HostMutex::new(Vec::new()));
impl log::Log for LossLog {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        self.0
            .lock()
            .unwrap()
            .push(alloc::format!("{}", record.args()));
    }
    fn flush(&self) {}
}
fn fixture() -> Arc<BreenFs<NoWait>> {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&LOG).unwrap();
        log::set_max_level(log::LevelFilter::Error);
    });
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

fn failed_mount(error: ops::Error) {
    let fs = fixture();
    fs.init_service().unwrap();
    let node = fs.open(2).unwrap();
    // Inject at the real multi-item edit completion boundary, before any
    // shutdown: both errors latch failure and fail the commit receipts.
    assert_eq!(
        fs.inner.state.lock().edited::<()>(Err(error)),
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
    assert!(LOG
        .0
        .lock()
        .unwrap()
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
