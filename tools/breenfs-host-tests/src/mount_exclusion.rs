//! E24 coordinator ledger 309 C1: while one caller holds a mount inside a cold
//! block read, a second caller of the same mount parks on the mount exclusion
//! instead of spinning, and both complete with the right results.
use super::*;
use alloc::{vec, vec::Vec};
use core::sync::atomic::AtomicBool;
use std::{
    sync::Mutex as HostMutex,
    thread,
    time::{Duration, Instant},
};

const LEN: usize = 3 * BLOCK_SIZE;

// Callers may share the cache from several threads, so a wait yields and the
// caller rechecks (Wait permits an early return).
struct YieldWait;
impl Wait for YieldWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        thread::yield_now();
    }
    fn wake_all(&self) {}
}

// Once armed, the next device read blocks until the check opens the gate:
// a holder asleep on block I/O.
struct Gate {
    disk: HostMutex<Vec<u8>>,
    armed: AtomicBool,
    inside: AtomicBool,
    open: AtomicBool,
}
impl BlockDevice for Gate {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.inside.store(true, Ordering::SeqCst);
            while !self.open.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
        }
        let start = block as usize * BLOCK_SIZE;
        bytes.copy_from_slice(&self.disk.lock().unwrap()[start..start + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let start = block as usize * BLOCK_SIZE;
        self.disk.lock().unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
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

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let limit = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < limit, "{what}");
        thread::sleep(Duration::from_millis(1));
    }
}

// A committed file on the device, then unmounted, so a new mount reads cold.
fn image() -> (Arc<Gate>, u64, Vec<u8>) {
    let disk = Arc::new(Gate {
        disk: HostMutex::new(vec![0; 64 * 1024 * 1024]),
        armed: AtomicBool::new(false),
        inside: AtomicBool::new(false),
        open: AtomicBool::new(false),
    });
    let sb = Superblock {
        fs_uuid: [0x33; 16],
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
    let bytes: Vec<u8> = (0..LEN).map(|i| (i % 251) as u8).collect();
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(YieldWait)), 74, 1)
        .unwrap();
    fs.init_service().unwrap();
    let ino = fs
        .create(
            2,
            b"cold",
            NewNode::Regular {
                attrs: InodeAttrs::new(0o100600, 0, 0),
            },
        )
        .unwrap();
    let node = fs.open(ino).unwrap();
    let req = WriteReq {
        at: WriteAt::Offset(0),
        limit: u64::MAX,
        clear_setid: false,
    };
    assert_eq!(fs.write(&node, req, &bytes).map(|w| w.bytes), Ok(LEN));
    fs.fsync(&node, false).unwrap();
    fs.shutdown("c1-image", u64::MAX).unwrap();
    (disk, ino, bytes)
}

#[test]
fn c1_contender_parks_while_holder_sleeps_in_a_cold_read() {
    let (disk, ino, bytes) = image();
    let fs = BreenFs::mount(disk.clone(), Arc::new(BufferCache::new(YieldWait)), 75, 1).unwrap();
    let node = fs.open(ino).unwrap();
    disk.armed.store(true, Ordering::SeqCst);
    let holder = {
        let (fs, node) = (fs.clone(), node.clone());
        thread::spawn(move || {
            let mut buf = vec![0; LEN];
            (fs.read(&node, 0, &mut buf), buf)
        })
    };
    wait_until("the read never reached the device", || {
        disk.inside.load(Ordering::SeqCst)
    });
    assert!(
        fs.inner.state.is_held(),
        "the cold read is not under the mount exclusion"
    );
    let contender = {
        let fs = fs.clone();
        thread::spawn(move || fs.getattr(ino).map(|attrs| attrs.size))
    };
    wait_until("the contender did not park on the mount exclusion", || {
        fs.inner.state.parks() >= 1
    });
    assert!(
        !contender.is_finished(),
        "the contender entered the mount while the holder was inside its read"
    );
    disk.open.store(true, Ordering::SeqCst);
    let (read, buf) = holder.join().unwrap();
    assert_eq!(read, Ok(LEN));
    assert!(buf == bytes, "the holder read wrong bytes");
    assert_eq!(contender.join().unwrap(), Ok(LEN as u64));
    assert!(!fs.inner.state.is_held(), "the exclusion was not released");
}
