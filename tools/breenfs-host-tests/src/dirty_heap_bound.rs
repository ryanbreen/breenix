//! E24 ledger 309 C3: sustained writes keep BreenFS's dirty pages and transient
//! commit memory well inside the smallest kernel heap, and a durable
//! generation's private buffers are released.
//!
//! The commit path runs on the test thread here (the worker pass and the
//! cfg(test) sync path are driven inline), so a per-thread counting allocator
//! measures the heap it holds: dirty pages, transaction copies, node buffers,
//! the published generation and the buffer cache's own resident charge.
use super::*;
use alloc::vec;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Mutex as HostMutex;

const MIB: usize = 1024 * 1024;
// The smallest kernel heap is 64 MiB (x86-64 HEAP_SIZE and the ARM64 QEMU
// heap, memory/heap.rs). Three eighths of it: one eighth each for dirty data,
// the buffer cache's own budget (design 1.2) and transaction copies and nodes.
const PEAK_BOUND: usize = 3 * 64 * MIB / 8;
// After a durable fsync no generation's private data may stay resident. The
// bound is a third of the 3 MiB the release check commits.
const RETAINED_BOUND: usize = MIB;

std::thread_local! {
    static FAIL: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}
fn reject(size: usize) -> bool {
    FAIL.try_with(|fail| match fail.get() {
        Some((target, 1)) if target == size => { fail.set(None); true }
        Some((target, nth)) if target == size => { fail.set(Some((target, nth - 1))); false }
        _ => false,
    }).unwrap_or(false)
}
pub(super) fn fail_allocation<T>(size: usize, nth: usize, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset { fn drop(&mut self) { FAIL.with(|fail| fail.set(None)); } }
    FAIL.with(|fail| { assert!(fail.get().is_none()); fail.set(Some((size, nth))); });
    let _reset = Reset;
    let result = run();
    FAIL.with(|fail| assert!(fail.get().is_none(), "allocation failure was not reached"));
    result
}
fn charge(delta: isize) {
    let _ = LIVE.try_with(|live| {
        let now = live.get() + delta;
        live.set(now);
        let _ = PEAK.try_with(|peak| {
            if now > peak.get() {
                peak.set(now);
            }
        });
    });
}
fn live() -> isize {
    LIVE.with(Cell::get)
}
fn reset_peak() -> isize {
    let now = live();
    PEAK.with(|peak| peak.set(now));
    now
}
fn peak() -> isize {
    PEAK.with(Cell::get)
}
struct Counting;
// SAFETY: each method forwards to the system allocator with the caller's
// layout; the accounting touches only const-initialized thread-locals.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if reject(layout.size()) { return core::ptr::null_mut(); }
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            charge(layout.size() as isize);
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if reject(layout.size()) { return core::ptr::null_mut(); }
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            charge(layout.size() as isize);
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        charge(-(layout.size() as isize));
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if reject(size) { return core::ptr::null_mut(); }
        let new = unsafe { System.realloc(ptr, layout, size) };
        if !new.is_null() {
            charge(size as isize - layout.size() as isize);
        }
        new
    }
}
#[global_allocator]
static HEAP: Counting = Counting;

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        panic!("a synchronous host disk never waits on cache I/O")
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
        (self.0.lock().unwrap().len() / BLOCK_SIZE) as u64
    }
    fn flush(&self) -> Result<(), BlockError> {
        Ok(())
    }
}
const ATTRS: InodeAttrs = InodeAttrs::new(0o100644, 0, 0);

// A 128 MiB disk holds the 48 MiB stream with room for copy-on-write.
fn fixture(name: &[u8]) -> (Arc<BreenFs<NoWait>>, Arc<BufferCache<NoWait>>, Arc<VNode>) {
    let blocks = 32768u64;
    let disk = Arc::new(Disk(HostMutex::new(vec![0; blocks as usize * BLOCK_SIZE])));
    let sb = Superblock {
        fs_uuid: [0x33; 16],
        generation: 1,
        slot: 1,
        total_blocks: blocks,
        next_root_id: 2,
        ..Superblock::default()
    };
    let mut files = Files::format(super::super::commit::DeviceStore(disk.as_ref()), &sb).unwrap();
    files
        .fsync(disk.as_ref(), &mut Committer::format(sb))
        .unwrap();
    drop(files);
    let cache = Arc::new(BufferCache::new(NoWait));
    let fs = BreenFs::mount_writable(disk, cache.clone(), 73, 1).unwrap();
    fs.init_service().unwrap();
    let ino = fs
        .create(2, name, NewNode::Regular { attrs: ATTRS })
        .unwrap();
    let node = fs.open(ino).unwrap();
    (fs, cache, node)
}
// Each page carries its own index, so a lost or misplaced page is visible.
fn fill(chunk: &mut [u8], first_page: u64) {
    for (i, page) in chunk.chunks_exact_mut(BLOCK_SIZE).enumerate() {
        let index = first_page + i as u64;
        page.fill((index % 251) as u8);
        page[..8].copy_from_slice(&index.to_le_bytes());
    }
}
fn write(fs: &BreenFs<NoWait>, node: &VNode, offset: u64, bytes: &[u8]) {
    let req = WriteReq {
        at: WriteAt::Offset(offset),
        limit: u64::MAX,
        clear_setid: false,
    };
    let written = fs.write(node, req, bytes).ok().expect("write");
    assert_eq!(written.bytes, bytes.len());
}

#[test]
fn c3_sustained_writes_stay_inside_a_heap_share() {
    let (fs, _cache, node) = fixture(b"stream");
    let mut chunk = vec![0u8; MIB];
    let mut page = vec![0u8; BLOCK_SIZE];
    let mut expect = vec![0u8; BLOCK_SIZE];
    let pages = (MIB / BLOCK_SIZE) as u64;
    let base = reset_peak();
    // Phase A: the commit worker runs after each write, as a woken kthread
    // would. Phase B: writers outrun a worker that is not scheduled, so
    // admission itself must hold the bound.
    for n in 0..48u64 {
        fill(&mut chunk, n * pages);
        write(&fs, &node, n * MIB as u64, &chunk);
        if n < 24 {
            fs.commit_due(monotonic_ns()).unwrap();
        }
    }
    fs.fsync(&node, false).unwrap();
    let transient = (peak() - base) as usize;
    for index in [0, 255, 256, 3000, 6143, 6144, 9000, 12287] {
        fs.read(&node, index * BLOCK_SIZE as u64, &mut page).unwrap();
        fill(&mut expect, index);
        assert_eq!(page, expect, "page {index} after the stream");
    }
    assert!(
        transient <= PEAK_BOUND,
        "48 MiB of sustained writes held {} KiB of heap at peak; bound {} KiB",
        transient / 1024,
        PEAK_BOUND / 1024
    );
}

#[test]
fn c3_durable_generation_releases_private_buffers() {
    let (fs, cache, node) = fixture(b"released");
    let mut chunk = vec![0u8; 3 * MIB];
    fill(&mut chunk, 0);
    let base = live();
    write(&fs, &node, 0, &chunk);
    fs.fsync(&node, false).unwrap();
    assert_eq!(fs.dirty_bytes(), 0);
    // Clean buffers stay in the cache under its own budget; everything else
    // the durable generation allocated must be gone.
    let retained = live() - base - cache.resident_bytes() as isize;
    assert!(
        retained <= RETAINED_BOUND as isize,
        "a durable 3 MiB generation left {} KiB resident outside the buffer cache; bound {} KiB",
        retained / 1024,
        RETAINED_BOUND / 1024
    );
}
