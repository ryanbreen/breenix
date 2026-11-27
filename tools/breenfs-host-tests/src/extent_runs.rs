//! Phase 1.1 (audit R7, and the R6 interim): a clean read issues one device
//! request per physically contiguous run of file blocks, up to the block
//! cache's RUN_BYTES device limit; mmap population reads runs of pages, not
//! single pages; the in-core extent map is reused until the inode's extents
//! change; a partial overwrite keeps the old extent's untouched head and tail
//! as two bookend items rather than one item per 4 KiB block.
//!
//! Layouts are built with the production Files writer, interleaving files so
//! that their blocks alternate on the device. The byte oracle is a shadow copy
//! of the writes; the request oracle is computed from the stored EXTENT items.
use super::*;
use super::super::{commit::DeviceStore, format::{Extent, Key}};
use std::sync::Mutex as HostMutex;

const RUN_BLOCKS: usize = crate::block::cache::RUN_BYTES / BLOCK_SIZE;
const DISK_BLOCKS: u64 = 16384;

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 { 0 }
    fn wait(&self, _: u64) { panic!("sequential extent-run check waited for I/O") }
    fn wake_all(&self) {}
}

/// A RAM disk that logs each read request as (first block, block count):
/// requests made on the disk itself in `reads`, and requests made through a
/// `Data` handle in `data_reads`.
struct Disk {
    bytes: HostMutex<Vec<u8>>,
    reads: HostMutex<Vec<(u64, usize)>>,
    data_reads: HostMutex<Vec<(u64, usize)>>,
}
impl Disk {
    fn new() -> Self {
        Self {
            bytes: HostMutex::new(vec![0; DISK_BLOCKS as usize * BLOCK_SIZE]),
            reads: HostMutex::new(Vec::new()),
            data_reads: HostMutex::new(Vec::new()),
        }
    }
    fn take_reads(&self) -> Vec<(u64, usize)> { core::mem::take(&mut *self.reads.lock().unwrap()) }
    fn take_data_reads(&self) -> Vec<(u64, usize)> {
        core::mem::take(&mut *self.data_reads.lock().unwrap())
    }
    fn copy_out(&self, block: u64, count: usize, bytes: &mut [u8]) {
        let at = block as usize * BLOCK_SIZE;
        let len = count * BLOCK_SIZE;
        bytes[..len].copy_from_slice(&self.bytes.lock().unwrap()[at..at + len]);
    }
}
impl BlockDevice for Disk {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(block, 1, bytes)
    }
    fn read_blocks(&self, block: u64, count: usize, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.reads.lock().unwrap().push((block, count));
        self.copy_out(block, count, bytes);
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

/// The data device a Files read is handed. Metadata reads go through the
/// Files store, on the disk itself, so only data requests land here.
struct Data<'a>(&'a Disk);
impl BlockDevice for Data<'_> {
    fn identity(&self) -> usize { self.0 as *const Disk as usize }
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(block, 1, bytes)
    }
    fn read_blocks(&self, block: u64, count: usize, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.0.data_reads.lock().unwrap().push((block, count));
        self.0.copy_out(block, count, bytes);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> { self.0.write_block(block, bytes) }
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { DISK_BLOCKS }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}

/// xorshift64*, reproducible without host entropy.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 { self.next() % n }
}

/// Publish the transaction and continue in a fresh writable view, as the
/// writer adapter does: publication fills child checksums the old overlay lacks.
fn commit<'d>(files: &mut Files<'d, DeviceStore<'d>>, disk: &'d Disk, committer: &mut Committer) -> Superblock {
    let sb = files.fsync(disk, committer).unwrap();
    *files = Files::new(DeviceStore(disk), &sb, 1, true).unwrap();
    sb
}

fn at(tick: i64) -> Time { Time { seconds: tick, nanos: 7 } }
fn superblock(tag: u8) -> Superblock {
    Superblock { fs_uuid: [tag; 16], generation: 1, slot: 1, total_blocks: DISK_BLOCKS,
        global_reserve: 1024, next_root_id: 2, ..Superblock::default() }
}

/// The device requests a clean read of `[off, end)` needs: one per maximal
/// run of mapped blocks that are adjacent both in the file and on the device,
/// split at RUN_BLOCKS. Holes need no request.
fn expected_requests(extents: &[(Key, Extent)], off: u64, end: u64) -> usize {
    let bs = BLOCK_SIZE as u64;
    let mut blocks = Vec::new();
    for (k, e) in extents {
        for i in 0..u64::from(e.num_blocks) {
            let logical = k.offset + i * bs;
            if logical < end && logical + bs > off {
                blocks.push((logical / bs, e.phys_start + u64::from(e.extent_offset) + i));
            }
        }
    }
    let mut requests = 0;
    let mut run: Option<(u64, u64, usize)> = None;
    for (l, p) in blocks {
        run = match run {
            Some((pl, pp, len)) if l == pl + 1 && p == pp + 1 && len < RUN_BLOCKS => Some((l, p, len + 1)),
            _ => { requests += 1; Some((l, p, 1)) }
        };
    }
    requests
}

fn write_shadow(shadow: &mut Vec<u8>, off: usize, bytes: &[u8]) {
    if shadow.len() < off + bytes.len() { shadow.resize(off + bytes.len(), 0); }
    shadow[off..off + bytes.len()].copy_from_slice(bytes);
}

fn pattern(rng: &mut Rng, len: usize) -> Vec<u8> {
    let seed = rng.next();
    (0..len).map(|i| (seed.wrapping_add(i as u64 * 0x9e37) >> 13) as u8).collect()
}

/// Random layouts: interleaved appends and overwrites of three files, sparse
/// gaps and truncation, committed at random points. Reads of the three files
/// at random offsets return the shadow's bytes, and each clean read issues
/// exactly the request count its stored extents call for.
#[test]
fn clean_reads_issue_one_request_per_contiguous_run_over_random_layouts() {
    for seed in 1..=12u64 {
        let disk = Disk::new();
        let sb = superblock(0x40 + seed as u8);
        let mut committer = Committer::format(sb.clone());
        let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
        let mut rng = Rng(0x5eed_0000 + seed);
        let inos: Vec<u64> = (0..3).map(|n| files.create(2, format!("f{n}").as_bytes(), 0o100644, at(1)).unwrap()).collect();
        let mut shadows = vec![Vec::new(); 3];
        for tick in 0..160 {
            let f = rng.below(3) as usize;
            let span = if rng.below(4) == 0 { 40 * BLOCK_SIZE as u64 } else { 3 * BLOCK_SIZE as u64 };
            let len = 1 + rng.below(span) as usize;
            let off = match rng.below(5) {
                0 | 1 => shadows[f].len(),                                            // append
                2 => shadows[f].len() + rng.below(6 * BLOCK_SIZE as u64) as usize,    // sparse gap
                _ => rng.below(shadows[f].len().max(1) as u64) as usize,              // overwrite
            };
            if off + len > 600 * BLOCK_SIZE { continue; }
            if rng.below(23) == 0 && !shadows[f].is_empty() {
                let size = rng.below(shadows[f].len() as u64) as usize;
                files.truncate(&Data(&disk), inos[f], size as u64, at(tick + 2)).unwrap();
                shadows[f].truncate(size);
                continue;
            }
            let bytes = pattern(&mut rng, len);
            assert_eq!(files.write(&Data(&disk), inos[f], off as u64, &bytes, at(tick + 2)).unwrap(), len);
            write_shadow(&mut shadows[f], off, &bytes);
            if rng.below(9) == 0 {
                commit(&mut files, &disk, &mut committer);
            }
        }
        // Private (uncommitted) extents are served from memory: bytes only.
        for (f, &ino) in inos.iter().enumerate() {
            let mut buf = vec![0xee; shadows[f].len() + 5000];
            assert_eq!(files.read(&Data(&disk), ino, 0, &mut buf).unwrap(), shadows[f].len());
            assert!(buf[..shadows[f].len()] == shadows[f][..], "seed {seed}: private read of file {f}");
        }
        let durable = commit(&mut files, &disk, &mut committer);
        drop(files);
        // A fresh view reads committed extents from the device.
        let files = Files::new(DeviceStore(&disk), &durable, 1, false).unwrap();
        for (f, &ino) in inos.iter().enumerate() {
            let extents = files.extents(ino).unwrap();
            let size = shadows[f].len();
            for probe in 0..40 {
                let (off, len) = if probe == 0 { (0, size + 4096) } else {
                    let off = rng.below(size as u64 + 2 * BLOCK_SIZE as u64) as usize;
                    (off, 1 + rng.below(70 * BLOCK_SIZE as u64) as usize)
                };
                let mut buf = vec![0xa5; len];
                disk.take_data_reads();
                let n = files.read(&Data(&disk), ino, off as u64, &mut buf).unwrap();
                let requests = disk.take_data_reads();
                let want = size.saturating_sub(off).min(len);
                assert_eq!(n, want, "seed {seed} file {f} read {off}+{len}");
                assert!(buf[..n] == shadows[f][off.min(size)..off.min(size) + n],
                    "seed {seed} file {f} read {off}+{len}: bytes differ from the writes");
                assert_eq!(requests.len(), expected_requests(&extents, off as u64, (off + n) as u64),
                    "seed {seed} file {f} read {off}+{len}: {} extents, requests {requests:?}", extents.len());
            }
        }
    }
}

/// A partial overwrite of one extent leaves three items: the untouched head
/// and tail referencing the old extent (bookends) and the new data.
#[test]
fn partial_overwrite_keeps_bookend_items() {
    let disk = Disk::new();
    let sb = superblock(0x61);
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
    let ino = files.create(2, b"bookend", 0o100644, at(1)).unwrap();
    let mut rng = Rng(0xb00c);
    let mut shadow = pattern(&mut rng, 64 * BLOCK_SIZE);
    files.write(&Data(&disk), ino, 0, &shadow, at(2)).unwrap();
    commit(&mut files, &disk, &mut committer);
    let patch = pattern(&mut rng, 3 * BLOCK_SIZE - 100);
    files.write(&Data(&disk), ino, 10 * BLOCK_SIZE as u64 + 50, &patch, at(3)).unwrap();
    write_shadow(&mut shadow, 10 * BLOCK_SIZE + 50, &patch);
    commit(&mut files, &disk, &mut committer);
    let extents = files.extents(ino).unwrap();
    let shape: Vec<_> = extents.iter().map(|(k, e)| (k.offset / BLOCK_SIZE as u64, e.num_blocks)).collect();
    assert_eq!(shape, [(0, 10), (10, 3), (13, 51)], "overwrite left {} items", extents.len());
    assert_eq!(extents[0].1.phys_start, extents[2].1.phys_start, "bookends name the old extent");
    let mut buf = vec![0; shadow.len()];
    assert_eq!(files.read(&Data(&disk), ino, 0, &mut buf).unwrap(), shadow.len());
    assert!(buf == shadow);
}

/// The second read of a many-extent file walks no EXTENT items: the in-core
/// map answers it, so it reads at most the INODE's root-to-leaf path. After
/// each change to the file's extents (overwrite, truncate, extension over a
/// hole) the map is rebuilt and reads return the new bytes, both before and
/// after the change commits.
#[test]
fn extent_map_is_reused_until_the_inode_changes() {
    let disk = Disk::new();
    let sb = superblock(0x62);
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
    let a = files.create(2, b"a", 0o100644, at(1)).unwrap();
    let b = files.create(2, b"b", 0o100644, at(1)).unwrap();
    let mut rng = Rng(0x3a9);
    let mut shadow = Vec::new();
    // Alternate single blocks so each of the 3000 blocks of `a` is its own extent.
    for i in 0..3000u64 {
        let block = pattern(&mut rng, BLOCK_SIZE);
        files.write(&Data(&disk), a, i * BLOCK_SIZE as u64, &block, at(2)).unwrap();
        write_shadow(&mut shadow, i as usize * BLOCK_SIZE, &block);
        files.write(&Data(&disk), b, i * BLOCK_SIZE as u64, &block, at(2)).unwrap();
        if i % 500 == 499 { commit(&mut files, &disk, &mut committer); }
    }
    let durable = commit(&mut files, &disk, &mut committer);
    drop(files);
    let mut files = Files::new(DeviceStore(&disk), &durable, 1, true).unwrap();
    assert_eq!(files.extents(a).unwrap().len(), 3000);
    let check = |files: &Files<'_, DeviceStore<'_>>, shadow: &[u8], what: &str| {
        let mut buf = vec![0; shadow.len() + BLOCK_SIZE];
        assert_eq!(files.read(&Data(&disk), a, 0, &mut buf).unwrap(), shadow.len(), "{what}");
        assert!(buf[..shadow.len()] == shadow[..], "{what}: bytes differ from the writes");
    };
    check(&files, &shadow, "first read");
    disk.take_reads();
    let mut one = vec![0; BLOCK_SIZE];
    files.read(&Data(&disk), a, 1717 * BLOCK_SIZE as u64, &mut one).unwrap();
    assert!(one[..] == shadow[1717 * BLOCK_SIZE..1718 * BLOCK_SIZE]);
    // One data block; the INODE lookup reads at most a few tree nodes, each
    // one request of NODE_SIZE blocks. The old full extent walk reads each
    // leaf holding the file's 3000 EXTENT items (22 requests at the check commit).
    let metadata = disk.take_reads().len();
    assert!(metadata <= 4, "second read made {metadata} metadata requests");
    // Overwrite a span crossing many single-block extents.
    let patch = pattern(&mut rng, 37 * BLOCK_SIZE + 11);
    files.write(&Data(&disk), a, 1000 * BLOCK_SIZE as u64 - 5, &patch, at(3)).unwrap();
    write_shadow(&mut shadow, 1000 * BLOCK_SIZE - 5, &patch);
    check(&files, &shadow, "after overwrite");
    // Shrink, then extend past a hole: the old extents beyond EOF must not
    // reappear in the hole.
    files.truncate(&Data(&disk), a, 2000 * BLOCK_SIZE as u64 + 3, at(4)).unwrap();
    shadow.truncate(2000 * BLOCK_SIZE + 3);
    check(&files, &shadow, "after truncate");
    let tail = pattern(&mut rng, 2 * BLOCK_SIZE);
    files.write(&Data(&disk), a, 2600 * BLOCK_SIZE as u64, &tail, at(5)).unwrap();
    write_shadow(&mut shadow, 2600 * BLOCK_SIZE, &tail);
    check(&files, &shadow, "after extension");
    commit(&mut files, &disk, &mut committer);
    check(&files, &shadow, "after commit");
}

/// mmap population of a fragmented file through a cold mount: one device
/// request per contiguous run within each population run of RUN_BLOCKS
/// pages, where the old path read one page per request.
#[test]
fn population_reads_runs_not_pages() {
    use crate::memory::file_map::{bind_populated, populate};
    let disk = Arc::new(Disk::new());
    let sb = superblock(0x63);
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let a = files.create(2, b"mapped", 0o100644, at(1)).unwrap();
    let b = files.create(2, b"other", 0o100644, at(1)).unwrap();
    let mut rng = Rng(0x7a9e);
    let mut shadow = Vec::new();
    // Chunks of 1 to 24 blocks alternate with another file's, so runs both
    // break inside and continue across population runs.
    while shadow.len() < 300 * BLOCK_SIZE {
        let len = (1 + rng.below(24) as usize) * BLOCK_SIZE;
        let bytes = pattern(&mut rng, len);
        let off = shadow.len();
        files.write(&Data(&disk), a, off as u64, &bytes, at(2)).unwrap();
        write_shadow(&mut shadow, off, &bytes);
        files.write(&Data(&disk), b, off as u64, &bytes[..BLOCK_SIZE], at(2)).unwrap();
        commit(&mut files, disk.as_ref(), &mut committer);
    }
    let tail = pattern(&mut rng, 777);
    let off = shadow.len();
    files.write(&Data(&disk), a, off as u64, &tail, at(3)).unwrap();
    write_shadow(&mut shadow, off, &tail);
    let durable = commit(&mut files, disk.as_ref(), &mut committer);
    drop(files);
    let extents = Files::new(DeviceStore(disk.as_ref()), &durable, 1, false).unwrap().extents(a).unwrap();
    let data: std::collections::BTreeSet<u64> = extents.iter()
        .flat_map(|(_, e)| (0..u64::from(e.num_blocks)).map(|i| e.phys_start + u64::from(e.extent_offset) + i))
        .collect();
    let size = shadow.len() as u64;
    let pages = size.div_ceil(BLOCK_SIZE as u64);
    let mut want = 0;
    for first in (0..pages).step_by(RUN_BLOCKS) {
        let lo = first * BLOCK_SIZE as u64;
        want += expected_requests(&extents, lo, (lo + (RUN_BLOCKS * BLOCK_SIZE) as u64).min(size));
    }
    let observer = BreenFs::mount(disk.clone(), Arc::new(BufferCache::new(NoWait)), 91, 1).unwrap();
    let node = observer.open(a).unwrap();
    disk.take_reads();
    let Ok(population) = populate(&node, 0, pages) else { panic!("population failed") };
    let reads = disk.take_reads();
    let binding = bind_populated(node.clone(), &*population, size);
    let seen: Vec<u8> = (0..size).map(|offset| binding.load(offset).unwrap()).collect();
    assert!(seen == shadow, "population bytes differ from the writes");
    let data_reads = reads.iter().filter(|(block, _)| data.contains(block)).count();
    assert_eq!(data_reads, want,
        "{pages} pages in {} extents: population made {data_reads} data requests", extents.len());
    drop(binding);
    drop(population);
}
