//! E3-4 (docs/plans/e3-page-cache.md 6.1-6.4 and section 18): the fill
//! interface. Population fills the frames it claimed through
//! `FileSystem::fill`, which reports each page Data, Hole or Error. BreenFS's
//! fill reads each run of blocks that are adjacent in the file and on the
//! device as one request, at most the block layer's `RUN_BYTES`, into a
//! bounce buffer the fill owns, checks each block against its REF CRC and
//! copies the blocks into their frames, without the block cache.
use super::*;
use super::super::{commit::DeviceStore, format::{Extent, Key}};
use super::super::preempt::context::{self, Event};
use crate::memory::file_map::{bind_populated, fill_window, populate, FillBatch, FillPage};
use alloc::vec;
use std::{cell::RefCell, rc::Rc, sync::Mutex as HostMutex};

const RUN_BLOCKS: usize = crate::block::cache::RUN_BYTES / BLOCK_SIZE;
const DISK_BLOCKS: u64 = 16384;
const ROOT_DIR: u64 = 2;

struct YieldWait;
impl Wait for YieldWait {
    fn generation(&self) -> u64 { 0 }
    fn wait(&self, _: u64) { std::thread::yield_now(); }
    fn wake_all(&self) {}
}
type Fs = BreenFs<YieldWait>;

/// A RAM disk of filesystem-block sectors that records each read request as
/// (first block, block count). A request that reaches block `short` returns
/// success having copied only the blocks before it.
struct Disk {
    bytes: HostMutex<Vec<u8>>,
    reads: HostMutex<Vec<(u64, usize)>>,
    short: HostMutex<Option<u64>>,
}
impl Disk {
    fn new() -> Self {
        Self {
            bytes: HostMutex::new(vec![0; DISK_BLOCKS as usize * BLOCK_SIZE]),
            reads: HostMutex::new(Vec::new()),
            short: HostMutex::new(None),
        }
    }
    fn take_reads(&self) -> Vec<(u64, usize)> {
        core::mem::take(&mut *self.reads.lock().unwrap())
    }
}
impl BlockDevice for Disk {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(block, 1, bytes)
    }
    fn read_blocks(&self, block: u64, count: usize, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.reads.lock().unwrap().push((block, count));
        let copied = match *self.short.lock().unwrap() {
            Some(short) if block <= short && short < block + count as u64 => (short - block) as usize,
            _ => count,
        };
        let at = block as usize * BLOCK_SIZE;
        bytes[..copied * BLOCK_SIZE].copy_from_slice(&self.bytes.lock().unwrap()[at..at + copied * BLOCK_SIZE]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let at = block as usize * BLOCK_SIZE;
        self.bytes.lock().unwrap()[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { DISK_BLOCKS }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}

fn at(tick: i64) -> Time { Time { seconds: tick, nanos: 3 } }
fn superblock(tag: u8) -> Superblock {
    Superblock { fs_uuid: [tag; 16], generation: 1, slot: 1, total_blocks: DISK_BLOCKS,
        global_reserve: 1024, next_root_id: 2, ..Superblock::default() }
}
/// Bytes of chunk `chunk` of a file tagged `tag`, distinct per page.
fn pattern(tag: u8, chunk: usize, len: usize) -> Vec<u8> {
    (0..len).map(|i| (i / 7 + (i / BLOCK_SIZE) * 13 + chunk * 101 + usize::from(tag) * 31) as u8).collect()
}
fn commit<'d>(files: &mut Files<'d, DeviceStore<'d>>, disk: &'d Disk, committer: &mut Committer) -> Superblock {
    let sb = files.fsync(disk, committer).unwrap();
    *files = Files::new(DeviceStore(disk), &sb, 1, true).unwrap();
    sb
}

/// A committed image whose file `f` is written as `chunks` of (page offset,
/// pages), each its own extent: one block of file `g` follows each chunk on
/// the device, so no two of `f`'s extents are adjacent there. Returns `f`'s
/// inode, its extents and its bytes.
fn layout(disk: &Disk, tag: u8, chunks: &[(usize, usize)]) -> (u64, Vec<(Key, Extent)>, Vec<u8>) {
    let sb = superblock(tag);
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(disk), &sb).unwrap();
    let f = files.create(ROOT_DIR, b"f", 0o100644, at(1)).unwrap();
    let g = files.create(ROOT_DIR, b"g", 0o100644, at(1)).unwrap();
    let mut shadow = Vec::new();
    let mut durable = sb;
    for (chunk, &(page, pages)) in chunks.iter().enumerate() {
        let bytes = pattern(tag, chunk, pages * BLOCK_SIZE);
        let offset = page * BLOCK_SIZE;
        files.write(disk, f, offset as u64, &bytes, at(2)).unwrap();
        files.write(disk, g, (chunk * BLOCK_SIZE) as u64, &bytes[..BLOCK_SIZE], at(2)).unwrap();
        if shadow.len() < offset + bytes.len() {
            shadow.resize(offset + bytes.len(), 0);
        }
        shadow[offset..offset + bytes.len()].copy_from_slice(&bytes);
        durable = commit(&mut files, disk, &mut committer);
    }
    drop(files);
    let extents = Files::new(DeviceStore(disk), &durable, 1, false).unwrap().extents(f).unwrap();
    (f, extents, shadow)
}
fn observer(disk: &Arc<Disk>, id: usize) -> Arc<Fs> {
    BreenFs::mount(disk.clone(), Arc::new(BufferCache::new(YieldWait)), id, 1).unwrap()
}
/// The device blocks of `extents`.
fn data_blocks(extents: &[(Key, Extent)]) -> BTreeSet<u64> {
    extents.iter()
        .flat_map(|(_, e)| (0..u64::from(e.num_blocks)).map(|i| e.phys_start + u64::from(e.extent_offset) + i))
        .collect()
}
/// The requests a clean read of the whole file needs: one per maximal run of
/// blocks adjacent both in the file and on the device, split at RUN_BLOCKS.
fn expected_requests(extents: &[(Key, Extent)]) -> usize {
    let bs = BLOCK_SIZE as u64;
    let mut requests = 0;
    let mut run: Option<(u64, u64, usize)> = None;
    for (k, e) in extents {
        for i in 0..u64::from(e.num_blocks) {
            let (logical, phys) = (k.offset / bs + i, e.phys_start + u64::from(e.extent_offset) + i);
            run = match run {
                Some((l, p, len)) if logical == l + 1 && phys == p + 1 && len < RUN_BLOCKS => Some((logical, phys, len + 1)),
                _ => { requests += 1; Some((logical, phys, 1)) }
            };
        }
    }
    requests
}
/// Populate `node`'s first `length` bytes, bind them and read them back
/// through the binding.
fn map_bytes(node: &Arc<VNode>, length: u64) -> Result<Vec<u8>, i32> {
    let population = populate(node, 0, length.div_ceil(BLOCK_SIZE as u64)).map_err(|error| error.errno())?;
    let binding = bind_populated(node.clone(), &*population, length);
    Ok((0..length).map(|offset| binding.load(offset).unwrap()).collect())
}

/// Population of a 1,000-page file of four extents issues one device
/// request per run of blocks adjacent in the file and on the device, each at
/// most RUN_BLOCKS, and leaves no data entry in the block cache. Red on the
/// old tree on both counts: population read 16-page windows from the file's
/// start through the block cache, splitting the runs that cross a window.
#[test]
fn population_reads_each_run_once_and_leaves_no_data_in_the_block_cache() {
    let disk = Arc::new(Disk::new());
    let (ino, extents, shadow) = layout(&disk, 0x34, &[(0, 250), (250, 250), (500, 250), (750, 250)]);
    assert_eq!(extents.len(), 4, "the fixture needs four extents");
    let blocks = data_blocks(&extents);
    let fs = observer(&disk, 3_404_001);
    let node = fs.open(ino).unwrap();
    disk.take_reads();
    let mapped = map_bytes(&node, shadow.len() as u64).unwrap();
    let data: Vec<_> = disk.take_reads().into_iter().filter(|(block, _)| blocks.contains(block)).collect();
    assert!(mapped == shadow, "population bytes differ from the writes");
    assert!(data.iter().all(|&(_, count)| count <= RUN_BLOCKS), "a request exceeded RUN_BLOCKS: {data:?}");
    assert_eq!(data.len(), expected_requests(&extents), "1000 pages in 4 extents: requests {data:?}");
    let buffers = fs.inner.cache.buffers();
    let cached = blocks.iter().filter(|&&block| buffers.status(&*fs.inner.data, block).is_some()).count();
    assert_eq!(cached, 0, "population left {cached} file blocks in the block cache");
}

/// A sparse file's fill reports its hole pages as Hole and reads no device
/// block for them: pages 0-1 and 6 are data and 2-5 are a hole. Red on the
/// old tree, whose fill reported the hole pages as Data.
#[test]
fn a_sparse_files_hole_pages_fill_as_holes_without_a_device_read() {
    let disk = Arc::new(Disk::new());
    let (ino, extents, shadow) = layout(&disk, 0x35, &[(0, 2), (6, 1)]);
    let fs = observer(&disk, 3_404_002);
    let node = fs.open(ino).unwrap();
    let size = shadow.len() as u64;
    assert_eq!(size, 7 * BLOCK_SIZE as u64);
    let io = fs.mapping_io(&node).unwrap();
    let mut batch = FillBatch::new(0, size, 7).unwrap();
    disk.take_reads();
    node.filesystem.fill(&node, &*io, &mut batch).unwrap();
    let blocks = data_blocks(&extents);
    let reads = disk.take_reads();
    let outcomes: Vec<_> = (0..7).map(|n| batch.outcome(n)).collect();
    use FillPage::{Data, Hole};
    assert_eq!(outcomes, [Data, Data, Hole, Hole, Hole, Hole, Data]);
    assert_eq!(reads.iter().filter(|(block, _)| blocks.contains(block)).count(), 2, "requests {reads:?}");
    for n in [0, 1, 6] {
        assert!(batch.page_mut(n)[..] == shadow[n * BLOCK_SIZE..(n + 1) * BLOCK_SIZE], "page {n}");
    }
}

/// A run that fails, by its CRC or by a device that stops short of its end,
/// makes exactly that run's pages Error(EIO); the runs around it are Data.
/// Population of the file then fails with EIO and leaves no page cached, so
/// no mapping sees the unread bytes as 00. Red on the old tree, where the
/// failed run failed the whole fill.
#[test]
fn a_failed_run_fails_only_its_own_pages() {
    for (case, id) in [("crc", 3_404_003), ("short", 3_404_004)] {
        let disk = Arc::new(Disk::new());
        let (ino, extents, shadow) = layout(&disk, 0x36, &[(0, 8), (8, 8), (16, 8)]);
        assert_eq!(extents.len(), 3, "{case}: the fixture needs three extents");
        let bad = extents[1].1.phys_start + u64::from(extents[1].1.extent_offset) + 3;
        match case {
            "crc" => disk.bytes.lock().unwrap()[bad as usize * BLOCK_SIZE + 17] ^= 0x40,
            _ => *disk.short.lock().unwrap() = Some(bad),
        }
        let fs = observer(&disk, id);
        let node = fs.open(ino).unwrap();
        let io = fs.mapping_io(&node).unwrap();
        let mut batch = FillBatch::new(0, shadow.len() as u64, 24).unwrap();
        node.filesystem.fill(&node, &*io, &mut batch).unwrap_or_else(|error| panic!("{case}: the fill failed whole: {error:?}"));
        for n in 0..24 {
            let want = if (8..16).contains(&n) { FillPage::Error(Errno(EIO)) } else { FillPage::Data };
            assert_eq!(batch.outcome(n), want, "{case}: page {n}");
            if want == FillPage::Data {
                assert!(batch.page_mut(n)[..] == shadow[n * BLOCK_SIZE..(n + 1) * BLOCK_SIZE], "{case}: page {n}");
            }
        }
        drop(io);
        assert_eq!(map_bytes(&node, shadow.len() as u64).err(), Some(EIO), "{case}: population");
        assert_eq!(node.map.cache_counts(), (0, 0, 0), "{case}: a failed population cached pages");
    }
}

/// Two fills of different files on one mount, as a fixed interleaving: fill
/// A checks its first run and pauses before its first copy into a frame,
/// then fill B starts. Before E2.5 B waits for A's mount exclusion; where the
/// exclusion is free at that point, B runs there. Either way each mapping
/// holds its own file's bytes: B did not reuse the bytes A was copying.
/// The old tree staged each call's reads in a buffer of its own, so this
/// guards the ownership the bounce pool keeps and is not red there.
#[test]
fn two_fills_on_one_mount_never_share_a_bounce_buffer() {
    let disk = Arc::new(Disk::new());
    let sb = superblock(0x37);
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let (a_bytes, b_bytes) = (pattern(0x37, 0, 32 * BLOCK_SIZE), pattern(0x38, 1, 32 * BLOCK_SIZE));
    let a = files.create(ROOT_DIR, b"a", 0o100644, at(1)).unwrap();
    files.write(disk.as_ref(), a, 0, &a_bytes, at(2)).unwrap();
    commit(&mut files, disk.as_ref(), &mut committer);
    let b = files.create(ROOT_DIR, b"b", 0o100644, at(1)).unwrap();
    files.write(disk.as_ref(), b, 0, &b_bytes, at(2)).unwrap();
    commit(&mut files, disk.as_ref(), &mut committer);
    drop(files);
    let fs = observer(&disk, 3_404_005);
    let (node_a, node_b) = (fs.open(a).unwrap(), fs.open(b).unwrap());
    let early: Rc<RefCell<Option<Vec<u8>>>> = Rc::new(RefCell::new(None));
    let (paused, waited) = (Rc::new(RefCell::new(false)), Rc::new(RefCell::new(false)));
    let (live, other, seen, pause, wait) = (fs.clone(), node_b.clone(), early.clone(), paused.clone(), waited.clone());
    let length = b_bytes.len() as u64;
    fill_window::on_copy(Some(Box::new(move |_| {
        if pause.replace(true) {
            return;
        }
        if live.inner.state.is_held() {
            *wait.borrow_mut() = true;
        } else {
            *seen.borrow_mut() = Some(map_bytes(&other, length).unwrap());
        }
    })));
    let mapped_a = map_bytes(&node_a, a_bytes.len() as u64);
    fill_window::on_copy(None);
    assert!(*paused.borrow(), "fill A made no copy into a frame");
    let mapped_b = match early.borrow_mut().take() {
        Some(bytes) => bytes,
        None => {
            assert!(*waited.borrow(), "fill B neither ran nor waited");
            map_bytes(&node_b, length).unwrap()
        }
    };
    assert!(mapped_a.unwrap() == a_bytes, "file A's mapping holds other bytes");
    assert!(mapped_b == b_bytes, "file B's mapping holds other bytes");
}

/// An image whose root holds `f`: `pages` pages of `byte`, mounted writable.
fn writable(tag: u8, id: usize, pages: usize, byte: u8) -> (Arc<Fs>, Arc<VNode>) {
    let disk = Arc::new(Disk::new());
    let sb = superblock(tag);
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let f = files.create(ROOT_DIR, b"f", 0o100644, at(1)).unwrap();
    files.write(disk.as_ref(), f, 0, &vec![byte; pages * BLOCK_SIZE], at(2)).unwrap();
    files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    drop(files);
    let fs = BreenFs::mount_writable(disk, Arc::new(BufferCache::new(YieldWait)), id, 1).unwrap();
    let node = fs.open(f).unwrap();
    (fs, node)
}
fn write_at(fs: &Fs, node: &VNode, offset: u64, bytes: &[u8]) {
    let req = WriteReq { at: WriteAt::Offset(offset), limit: u64::MAX, clear_setid: false };
    assert_eq!(fs.write(node, req, bytes).unwrap().bytes, bytes.len());
}
fn read_all(fs: &Fs, node: &VNode, length: usize) -> Vec<u8> {
    let mut bytes = vec![0; length];
    assert_eq!(fs.read(node, 0, &mut bytes).unwrap(), length);
    bytes
}

/// DEV-29(3) 0.1's second interleaving, through the production fill: the
/// fill of pages 0-3 returns, then a BreenFS write changes page 0 before the
/// population merges. The fault on page 0 and a `read` both return the
/// write's bytes, and exactly one filled page is discarded. Today's mark
/// already holds this on the old tree (the merge drops the marked page and
/// fills it again), so this is not red there: it guards that the fill keeps
/// the mark.
#[test]
fn a_write_between_a_fill_and_its_merge_reaches_the_mapping() {
    let (fs, node) = writable(0x39, 3_404_006, 4, b'A');
    let (writer, file, mut pending) = (fs.clone(), node.clone(), true);
    fill_window::on_filled(Some(Box::new(move |_| {
        if core::mem::take(&mut pending) {
            write_at(&writer, &file, 0, &[b'N'; BLOCK_SIZE]);
        }
    })));
    let before = fill_window::discarded();
    let mapped = map_bytes(&node, 4 * BLOCK_SIZE as u64);
    fill_window::on_filled(None);
    let mut expected = vec![b'A'; 4 * BLOCK_SIZE];
    expected[..BLOCK_SIZE].fill(b'N');
    assert!(mapped.unwrap() == expected, "the mapping kept the fill's stale page 0");
    assert!(read_all(&fs, &node, 4 * BLOCK_SIZE) == expected, "read returned a stale page 0");
    assert_eq!(fill_window::discarded() - before, 1, "discarded filled pages");
}

/// Publish the open generation and run `during` once on this thread at its
/// first preemption point after the freeze with the mount exclusion free:
/// the frozen generation's pages are not drained yet (frozen_review.rs).
fn during_publication<T: 'static>(fs: &Fs, during: impl FnOnce() -> T + 'static) -> T {
    let generation = fs.inner.state.lock().durable.generation + 1;
    let lock_id = &fs.inner.state as *const _ as usize;
    let result = Rc::new(RefCell::new(None));
    let (output, mut during, mut armed, mut held) = (result.clone(), Some(during), false, false);
    context::observer(Some(Box::new(move |event| match event {
        Event::Acquire(id) if id == lock_id => held = true,
        Event::Release(id) if id == lock_id => held = false,
        Event::Phase { op: "commit", phase: "publication", begin: true } => armed = true,
        Event::Point if armed && !held => {
            if let Some(run) = during.take() { *output.borrow_mut() = Some(run()); }
        }
        _ => (),
    })));
    let published = fs.commit_generation_up_to(0, generation, true);
    context::observer(None);
    assert_eq!(published, Ok(true), "the commit did not publish");
    let value = result.borrow_mut().take();
    value.expect("the commit reached no free preemption point after its freeze")
}

/// Before E3-8 a fill reads BreenFS's dirty layers first (6.2): page 0 is
/// written and frozen in G, page 1 is written in G+1, and both are mapped
/// and read before G's drain. The extents still hold the older bytes, so
/// this is red only against an extents-only fill; the old tree read the
/// layers through `read`, so it is not red there.
#[test]
fn population_maps_written_and_frozen_pages_before_the_drain() {
    let (fs, node) = writable(0x3a, 3_404_007, 2, b'A');
    write_at(&fs, &node, 0, &[b'N'; BLOCK_SIZE]);
    let (live, file) = (fs.clone(), node.clone());
    let (mapped, read) = during_publication(&fs, move || {
        write_at(&live, &file, BLOCK_SIZE as u64, &[b'P'; BLOCK_SIZE]);
        let mapped = map_bytes(&file, 2 * BLOCK_SIZE as u64).unwrap();
        (mapped, read_all(&live, &file, 2 * BLOCK_SIZE))
    });
    let mut expected = vec![b'N'; 2 * BLOCK_SIZE];
    expected[BLOCK_SIZE..].fill(b'P');
    assert!(mapped == expected, "the mapping missed the frozen or the open generation's page");
    assert!(read == expected, "read missed the frozen or the open generation's page");
}
