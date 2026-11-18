//! E24 coordinator ledgers 597 and 601: checks for the x11c review findings
//! (ledger 604) on the frozen-generation commit. Each test drives production
//! VFS calls on this thread at one chosen commit preemption point, outside the
//! mount exclusion, and then reads the cold image the commit left.
use super::*;
use alloc::vec;
use super::super::preempt::context::{self, Event};
use std::{cell::RefCell, rc::Rc, sync::Mutex as HostMutex};

const ROOT_DIR: u64 = 2;

struct YieldWait;
impl Wait for YieldWait {
    fn generation(&self) -> u64 { 0 }
    fn wait(&self, _: u64) { std::thread::yield_now(); }
    fn wake_all(&self) {}
}
// 512-byte sectors, as the production-format writer fixtures use.
const SECTOR: usize = 512;
struct Disk(HostMutex<Vec<u8>>);
impl Disk {
    fn image(&self) -> Vec<u8> { self.0.lock().unwrap().clone() }
}
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { SECTOR }
    fn num_blocks(&self) -> u64 { self.0.lock().unwrap().len() as u64 / SECTOR as u64 }
    fn read_block(&self, n: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let at = n as usize * SECTOR;
        bytes.copy_from_slice(&self.0.lock().unwrap()[at..at + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, n: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let at = n as usize * SECTOR;
        self.0.lock().unwrap()[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
type Fs = BreenFs<YieldWait>;

/// A formatted image whose root holds `files`: (name, pages, fill byte).
fn image(total_blocks: u64, global_reserve: u64, files: &[(&[u8], usize, u8)]) -> Vec<u8> {
    let disk = Disk(HostMutex::new(vec![0; total_blocks as usize * BLOCK_SIZE]));
    let sb = Superblock { fs_uuid: [0x5c; 16], generation: 1, slot: 1, total_blocks, global_reserve,
        next_root_id: 2, dirhash_key: [0x33; 16], ..Superblock::default() };
    let at = Time { seconds: 0, nanos: 0 };
    {
        let mut view = Files::format(super::super::commit::DeviceStore(&disk), &sb).unwrap();
        for &(name, pages, byte) in files {
            let ino = view.create(ROOT_DIR, name, 0o644, at).unwrap();
            if pages != 0 {
                view.write(&disk, ino, 0, &vec![byte; pages * BLOCK_SIZE], at).unwrap();
            }
        }
        view.fsync(&disk, &mut Committer::format(sb)).unwrap();
    }
    disk.0.into_inner().unwrap()
}
fn mount(bytes: Vec<u8>, writable: bool, id: usize) -> (Arc<Disk>, Arc<Fs>) {
    let disk = Arc::new(Disk(HostMutex::new(bytes)));
    let cache = Arc::new(BufferCache::new(YieldWait));
    let fs = if writable { BreenFs::mount_writable(disk.clone(), cache, id, 1) }
        else { BreenFs::mount(disk.clone(), cache, id, 1) };
    (disk, fs.unwrap())
}
fn open(fs: &Fs, name: &[u8]) -> Arc<VNode> {
    fs.open(fs.lookup(ROOT_DIR, name).unwrap()).unwrap()
}
fn write_at(fs: &Fs, node: &VNode, offset: u64, bytes: &[u8]) -> Result<usize, WriteError> {
    fs.write(node, WriteReq { at: WriteAt::Offset(offset), limit: u64::MAX, clear_setid: false }, bytes)
        .map(|written| written.bytes)
}
fn read_at(fs: &Fs, node: &VNode, offset: u64, length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    let read = fs.read(node, offset, &mut bytes).unwrap();
    bytes.truncate(read);
    bytes
}
/// Publish the open generation, running `during` once on this thread at the
/// first commit preemption point after `phase` begins (or ends) at which the
/// mount exclusion is free. The observer is out while `during` runs, so its
/// own calls are not observed.
fn commit_with<T: 'static>(fs: &Fs, phase: &'static str, begin: bool, during: impl FnOnce() -> T + 'static) -> T {
    let generation = fs.inner.state.lock().durable.generation + 1;
    let lock_id = &fs.inner.state as *const _ as usize;
    let result = Rc::new(RefCell::new(None));
    let (output, mut during, mut armed, mut held) = (result.clone(), Some(during), false, false);
    context::observer(Some(Box::new(move |event| match event {
        Event::Acquire(id) if id == lock_id => held = true,
        Event::Release(id) if id == lock_id => held = false,
        Event::Phase { op: "commit", phase: name, begin: edge } if name == phase && edge == begin => armed = true,
        Event::Point if armed && !held => {
            if let Some(run) = during.take() { *output.borrow_mut() = Some(run()); }
        }
        _ => (),
    })));
    let published = fs.commit_generation_up_to(0, generation, true);
    context::observer(None);
    assert_eq!(published, Ok(true), "the commit did not publish");
    let value = result.borrow_mut().take();
    value.expect("the commit reached no free preemption point after its phase")
}

// x11c C1: a truncation that fails with ENOMEM while N is frozen must leave
// the size and the visible pages as they were. N's bytes stay visible, and a
// partial write over them keeps N's other bytes, in memory and in the cold
// image after fsync.
#[test]
fn failed_truncate_of_frozen_pages_changes_nothing() {
    // The shrink's tail-page allocation: the first page-sized allocation
    // after the admission and reservation reads, which the cache already holds.
    const TAIL_PAGE_ALLOCATION: usize = 1;
    let (disk, fs) = mount(image(16384, 64, &[(b"file", 2, b'A')]), true, 2_415_156_041);
    let node = open(&fs, b"file");
    let ino = node.ino;
    assert_eq!(write_at(&fs, &node, 0, &[b'N'; 2 * BLOCK_SIZE]), Ok(2 * BLOCK_SIZE));
    let (live, handle) = (fs.clone(), node.clone());
    let (failed, edited, size, page, written) = commit_with(&fs, "publication", true, move || {
        let failed = super::dirty_heap_bound::fail_allocation(BLOCK_SIZE, TAIL_PAGE_ALLOCATION,
            || live.resize(&handle, 1, false, &|_| Ok(())));
        // Only the page edit creates the open generation's entry for this file.
        let edited = live.inner.state.lock().dirty.contains_key(&ino);
        let size = live.getattr(ino).unwrap().size;
        let page = read_at(&live, &handle, BLOCK_SIZE as u64, BLOCK_SIZE);
        let written = write_at(&live, &handle, BLOCK_SIZE as u64, b"P");
        (failed, edited, size, page, written)
    });
    assert_eq!(failed, Err(Errno(ENOMEM)), "the forced allocation failure was not the resize's");
    assert!(edited, "the allocation failure came before the page edit");
    assert_eq!(size, 2 * BLOCK_SIZE as u64, "a failed truncation changed the size");
    assert!(page == [b'N'; BLOCK_SIZE], "a failed truncation hid frozen page 1");
    assert_eq!(written, Ok(1));
    let mut expected = vec![b'N'; 2 * BLOCK_SIZE];
    expected[BLOCK_SIZE] = b'P';
    assert!(read_at(&fs, &node, 0, 2 * BLOCK_SIZE) == expected, "live bytes after the partial write");
    fs.fsync(&node, false).unwrap();
    let (_, cold) = mount(disk.image(), false, 2_415_156_042);
    let cold_node = open(&cold, b"file");
    assert!(read_at(&cold, &cold_node, 0, 3 * BLOCK_SIZE) == expected, "cold bytes after fsync");
}

// x11c M4: a touch that read generation N before N froze, and recorded its
// stamp only after N's preparation took N's slot, has still returned before
// the fsync that follows it. That fsync must make the touch's time durable.
#[test]
fn touch_racing_a_freeze_is_durable_at_the_next_fsync() {
    const TOUCHED_AT: u64 = 77;
    let (disk, fs) = mount(image(16384, 64, &[(b"file", 1, b'A')]), true, 2_415_156_043);
    let node = open(&fs, b"file");
    assert_ne!(fs.getattr(node.ino).unwrap().times.mtime.sec, TOUCHED_AT as i64);
    let generation = fs.inner.state.lock().durable.generation + 1;
    let (live, published) = (fs.clone(), Rc::new(RefCell::new(None)));
    let result = published.clone();
    // N freezes, prepares and installs between touch's load and its record.
    context::observer(Some(Box::new(move |event| if let Event::Touch = event {
        if result.borrow().is_none() {
            *result.borrow_mut() = Some(live.commit_generation_up_to(0, generation, true));
        }
    })));
    crate::time::with_unix_time(TOUCHED_AT, || fs.touch(&node, false));
    context::observer(None);
    assert_eq!(published.borrow_mut().take(), Some(Ok(true)), "N was not published inside the touch");
    fs.fsync(&node, false).unwrap();
    let (_, cold) = mount(disk.image(), false, 2_415_156_044);
    let mtime = cold.getattr(node.ino).unwrap().times.mtime.sec;
    assert_eq!(mtime, TOUCHED_AT as i64, "the fsync after the touch lost its modification time");
}

// x11c M3: once N is installed its allocations have left free_blocks, while
// its grants are released afterwards, one per preemption point (601(4)). A
// write admitted between install and those releases must not be refused for
// space that only N's spent grants still claim.
#[test]
fn installed_generation_grants_refuse_no_usable_space() {
    const PAGES: usize = 512;
    // Admission room left beyond N's grant before N's write. N allocates at
    // least PAGES blocks, so after install the old accounting has no room.
    const SLACK: u64 = 256;
    let files: &[(&[u8], usize, u8)] = &[(b"big", 0, 0), (b"small", 0, 0)];
    let data = vec![b'N'; PAGES * BLOCK_SIZE];
    // A probe mount measures N's grant and the free count it starts from.
    let (grant, free) = {
        let (_, probe) = mount(image(16384, 64, files), true, 2_415_156_045);
        assert_eq!(write_at(&probe, &open(&probe, b"big"), 0, &data), Ok(data.len()));
        let state = probe.inner.state.lock();
        (state.reservations.outstanding(), state.durable.free_blocks)
    };
    let (disk, fs) = mount(image(16384, free - grant - SLACK, files), true, 2_415_156_046);
    let (big, small) = (open(&fs, b"big"), open(&fs, b"small"));
    assert_eq!(write_at(&fs, &big, 0, &data), Ok(data.len()));
    assert_eq!(fs.inner.state.lock().reservations.outstanding(), grant, "N's grant differs from the probe's");
    let (live, handle) = (fs.clone(), small.clone());
    let (accounting, written) = commit_with(&fs, "completion", false, move || {
        let state = live.inner.state.lock();
        let accounting = (state.durable.free_blocks, state.durable.global_reserve, state.reservations.outstanding());
        drop(state);
        (accounting, write_at(&live, &handle, 0, &[b'W'; BLOCK_SIZE]))
    });
    let (installed_free, reserve, outstanding) = accounting;
    assert!(outstanding == grant && installed_free - reserve <= outstanding,
        "the window does not exercise N's spent grants: {accounting:?}, grant {grant}");
    assert_eq!(written, Ok(BLOCK_SIZE), "a write after N's install was refused for N's spent grants");
    fs.fsync(&small, false).unwrap();
    let (_, cold) = mount(disk.image(), false, 2_415_156_047);
    assert!(read_at(&cold, &open(&cold, b"small"), 0, BLOCK_SIZE) == [b'W'; BLOCK_SIZE]);
    assert!(read_at(&cold, &open(&cold, b"big"), 0, data.len()) == data);
}
