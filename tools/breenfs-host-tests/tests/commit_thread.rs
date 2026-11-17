use breenfs_host_tests::{
    block::{BlockDevice, BlockError},
    buffer_cache::{BufferCache, Wait, WriteGroup, BUFFER_OVERHEAD},
    commit::{Committer, DeviceStore, Error},
    format::{Superblock, Time, BLOCK_SIZE},
    ops::Files,
    recording::{Event, RecordingDevice},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[path = "../src/test_image.rs"]
mod test_image;

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("unexpected cache contention")
    }
    fn wake_all(&self) {}
}
fn superblock(bytes: &[u8]) -> Superblock {
    Superblock::decode(&bytes[17 * BLOCK_SIZE..18 * BLOCK_SIZE]).unwrap()
}

mod commit_policy {
    #[test]
    fn triggers_at_5s_64mib_fsync_and_pressure() {
        use breenfs_host_tests::commit_policy::CommitPolicy;
        // Logical monotonic nanoseconds: no wall clock or timing tolerance.
        let clock = std::cell::Cell::new(37_000_000_000);
        let policy = CommitPolicy::new(clock.get());
        assert!(!policy.due(clock.get(), 0, false, false));
        clock.set(clock.get() + 4_999_999_999);
        assert!(!policy.due(clock.get(), 64 * 1024 * 1024 - 1, false, false));
        clock.set(clock.get() + 1);
        assert!(policy.due(clock.get(), 0, false, false));
        let policy = CommitPolicy::new(clock.get());
        assert!(policy.due(clock.get(), 64 * 1024 * 1024, false, false));
        assert!(policy.due(clock.get(), 0, true, false));
        assert!(policy.due(clock.get(), 0, false, true));
        assert!(!policy.due(clock.get(), 0, false, false));
    }
}

mod hold {
    use super::*;
    #[test]
    fn commit_through_hold_group_keeps_w1_w3() {
        let bytes = test_image::image();
        let mut sb = superblock(&bytes);
        let dev = Arc::new(RecordingDevice::new(bytes).unwrap());
        let mut committer = Committer::new(sb.clone());
        // claim-lint:ok: tools/breenfs-host-tests/src/recording.rs checks W1-W3.
        // Force bounded Hold submissions; each write passes the recorder's
        // last-two-generations and data-before-superblock-flush oracle.
        let cache = BufferCache::with_capacity(NoWait, 4 * (512 + BUFFER_OVERHEAD));
        for generation in 2..=3 {
            let mut files = Files::new(DeviceStore(dev.as_ref()), &sb, 1, true).unwrap();
            let ino = files
                .create(2, &[b'a' + generation as u8], 0o600, Time::default())
                .unwrap();
            files
                .write(
                    dev.as_ref(),
                    ino,
                    0,
                    &vec![generation as u8; BLOCK_SIZE],
                    Time::default(),
                )
                .unwrap();
            let receipt = committer.waiter();
            let before = dev.events().len();
            let prepared = committer.prepare_files(dev.as_ref(), &mut files).unwrap();
            assert_eq!(dev.events().len(), before, "preparation issued I/O");
            let asked = Arc::new(AtomicBool::new(false));
            let callback = {
                let asked = asked.clone();
                let dev = dev.clone();
                Arc::new(move || {
                    // Cache pressure happens while staging, before submit.
                    if !asked.swap(true, Ordering::SeqCst) {
                        assert_eq!(dev.events().len(), before, "Hold wrote before submit");
                    }
                })
            };
            let group = WriteGroup::hold(dev.as_ref(), callback);
            let result = prepared.write_cached(&cache, dev.as_ref(), &group);
            assert_eq!(result, Ok(()));
            assert!(asked.load(Ordering::SeqCst), "writes bypassed Hold staging");
            assert_eq!(
                receipt.result(),
                None,
                "receipt completed before publication result"
            );
            committer.complete(&prepared, result);
            assert_eq!(receipt.result(), Some(Ok(())));
            let events = dev.events();
            let events = &events[before..];
            let flushes: Vec<_> = events
                .iter()
                .enumerate()
                .filter_map(|(n, e)| matches!(e, Event::Flush).then_some(n))
                .collect();
            assert_eq!(flushes.len(), 2);
            assert_eq!(flushes[1], events.len() - 1);
            for (n, event) in events.iter().enumerate() {
                if let Event::Write { sector, .. } = event {
                    let superblock =
                        (16 * 8..20 * 8).contains(sector) || *sector >= dev.num_blocks() - 32;
                    assert_eq!(superblock, n > flushes[0], "publication crossed data flush");
                }
            }
            sb = prepared.superblock().clone();
            assert_eq!(sb.generation, generation);
            assert_eq!(dev.durable_generations(), vec![generation - 1, generation]);
            test_image::check(&dev.durable_image(), "hold");
        }
    }
}

struct FlushFailure {
    disk: RecordingDevice,
    fail: AtomicBool,
}
impl BlockDevice for FlushFailure {
    fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
        self.disk.read_block(n, b)
    }
    fn write_block(&self, n: u64, b: &[u8]) -> Result<(), BlockError> {
        self.disk.write_block(n, b)
    }
    fn block_size(&self) -> usize {
        self.disk.block_size()
    }
    fn num_blocks(&self) -> u64 {
        self.disk.num_blocks()
    }
    fn flush(&self) -> Result<(), BlockError> {
        if self.fail.load(Ordering::SeqCst) {
            Err(BlockError::IoError)
        } else {
            self.disk.flush()
        }
    }
}

#[test]
fn fsync_waits_for_the_generation_it_joined() {
    let bytes = test_image::image();
    let sb = superblock(&bytes);
    let dev = FlushFailure {
        disk: RecordingDevice::new(bytes).unwrap(),
        fail: AtomicBool::new(false),
    };
    let cache = BufferCache::new(NoWait);
    let mut committer = Committer::new(sb.clone());
    let mut files = Files::new(DeviceStore(&dev), &sb, 1, true).unwrap();
    files.create(2, b"first", 0o600, Time::default()).unwrap();
    let first = committer.waiter();
    let prepared = committer.prepare_files(&dev, &mut files).unwrap();
    let second = committer.waiter();
    assert_eq!(first.generation(), 2);
    assert_eq!(second.generation(), 3);
    assert_eq!(first.result(), None);
    assert_eq!(second.result(), None);
    let group = WriteGroup::hold(&dev, Arc::new(|| {}));
    let result = prepared.write_cached(&cache, &dev, &group);
    assert_eq!(result, Ok(()));
    committer.complete(&prepared, result);
    assert_eq!(first.result(), Some(Ok(())));
    assert_eq!(second.result(), None, "G's flush acknowledged G+1");
    let mut files = Files::new(DeviceStore(&dev), prepared.superblock(), 1, true).unwrap();
    files.create(2, b"second", 0o600, Time::default()).unwrap();
    let prepared = committer.prepare_files(&dev, &mut files).unwrap();
    let third = committer.waiter();
    dev.fail.store(true, Ordering::SeqCst);
    let result = prepared.write_cached(&cache, &dev, &group);
    assert_eq!(result, Err(Error::EIO));
    let issued = dev.disk.events().len();
    committer.complete(&prepared, result);
    assert!(committer.read_only());
    assert_eq!(first.result(), Some(Ok(())));
    for receipt in [second, third, committer.waiter()] {
        assert_eq!(receipt.result(), Some(Err(Error::EIO)));
    }
    assert!(committer.prepare_files(&dev, &mut files).is_err());
    assert_eq!(
        dev.disk.events().len(),
        issued,
        "failed mount issued more writes"
    );
}
