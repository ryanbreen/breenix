//! Phase 1.2 (audit R13): BreenFS free space.
//!
//! - Unlinking 10,000 files costs about one SPACE edit per file (its data
//!   REF), not four, and no commit allocates in proportion to the quarantine.
//! - One generation's adjacent releases are one PENDING item, and reuse at
//!   G+2 returns them to FREE coalesced (design 2.4 and invariant A2).
//! - Data written later to a file lands at or after its existing extent
//!   rather than in an unrelated earlier hole.
//!
//! The production Files writer runs on a RAM disk. Each commit continues the
//! mount with next_generation, as the kernel's publication does, so the
//! quarantine of one generation is carried to G+2 and not reread from disk.
use super::*;
use super::super::{
    btree,
    commit::DeviceStore,
    format::{Extent, Free, Key, Pending},
};
use super::dirty_heap_bound::{largest_allocation, live, peak, reset_peak};
use std::sync::Mutex as HostMutex;

const FREE_OBJECT: u64 = 0xffff_ffff_ffff_ff20;
const PENDING_OBJECT: u64 = 0xffff_ffff_ffff_ff30;

struct Disk {
    blocks: u64,
    bytes: HostMutex<Vec<u8>>,
}
impl Disk {
    fn new(blocks: u64) -> Self {
        Self { blocks, bytes: HostMutex::new(vec![0; blocks as usize * BLOCK_SIZE]) }
    }
}
impl BlockDevice for Disk {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let at = block as usize * BLOCK_SIZE;
        bytes.copy_from_slice(&self.bytes.lock().unwrap()[at..at + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let at = block as usize * BLOCK_SIZE;
        self.bytes.lock().unwrap()[at..at + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { self.blocks }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}

fn at(tick: i64) -> Time { Time { seconds: tick, nanos: 11 } }

/// A metadata change, so the generation has something to publish.
fn touch(tick: i64) -> ops::SetAttr {
    ops::SetAttr { mtime: Some(at(tick)), ..ops::SetAttr::default() }
}

fn superblock(tag: u8, total_blocks: u64) -> Superblock {
    Superblock { fs_uuid: [tag; 16], generation: 1, slot: 1, total_blocks,
        global_reserve: 1024, next_root_id: 2, ..Superblock::default() }
}

/// Publish and continue the mount from the published generation.
fn commit<'d>(files: &mut Files<'d, DeviceStore<'d>>, disk: &'d Disk, committer: &mut Committer) -> Superblock {
    let sb = files.fsync(disk, committer).unwrap();
    files.next_generation(DeviceStore(disk), &sb).unwrap();
    sb
}

fn items(disk: &Disk, sb: &Superblock, object: u64, ty: u8) -> Vec<(Key, Vec<u8>)> {
    btree::range(&DeviceStore(disk), sb.space_root,
        Key { object, item_type: ty, offset: 0 },
        Key { object, item_type: ty, offset: u64::MAX }).unwrap()
}

/// (start, length, released generation) of each PENDING item.
fn pending(disk: &Disk, sb: &Superblock) -> Vec<(u64, u64, u64)> {
    items(disk, sb, PENDING_OBJECT, format::PENDING).into_iter().map(|(key, bytes)| {
        let p = Pending::decode(&bytes).unwrap();
        (key.offset, p.length, p.released_generation)
    }).collect()
}

/// (start, length) of each FREE item.
fn free(disk: &Disk, sb: &Superblock) -> Vec<(u64, u64)> {
    items(disk, sb, FREE_OBJECT, format::FREE).into_iter()
        .map(|(key, bytes)| (key.offset, Free::decode(&bytes).unwrap().length))
        .collect()
}

fn data_extents(files: &Files<'_, DeviceStore<'_>>, ino: u64) -> Vec<Extent> {
    files.extents(ino).unwrap().into_iter().map(|(_, extent)| extent).collect()
}

fn block_pattern(n: usize) -> Vec<u8> {
    (0..BLOCK_SIZE).map(|i| (n.wrapping_mul(31) ^ i) as u8).collect()
}

/// Audit R13: "Unlinking 10,000 files makes about 40,000 space-tree edits",
/// and #3228's 384 KiB abort was promotion reading 8,192 PENDING records
/// onto the heap at once. Each file here owns one data REF, which its unlink
/// must delete. Everything else (PENDING, promotion to FREE, node copies) is
/// bounded by coalesced runs and touched nodes, not by files, so the window
/// from the first unlink through reuse at G+2 makes at most 1.25 SPACE edits
/// per file. The two commits after the unlink commit hold no per-record
/// state, and no allocation in the window grows with the quarantine.
#[test]
fn unlinking_ten_thousand_files_bounds_space_edits_and_heap() {
    const FILES: usize = 10_000;
    let disk = Disk::new(32_768);
    let sb = superblock(0x31, disk.blocks);
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
    for n in 0..FILES {
        let ino = files.create(2, format!("f{n:05}").as_bytes(), 0o100644, at(1)).unwrap();
        assert_eq!(files.write(&disk, ino, 0, &block_pattern(n), at(2)).unwrap(), BLOCK_SIZE);
    }
    let created = files.fsync(&disk, &mut committer).unwrap();
    // A clean mount of the populated volume drops the writer's data buffers.
    let mut files = Files::new(DeviceStore(&disk), &created, 1, true).unwrap();

    let _ = btree::measurement::take_tree_edits();
    let start = live();
    reset_peak();
    let (measured, largest) = largest_allocation(|| {
        for n in 0..FILES {
            files.unlink(2, format!("f{n:05}").as_bytes(), at(3)).unwrap();
        }
        let unlinked = commit(&mut files, &disk, &mut committer);
        let unlink_peak = peak() - start;
        let promotion_start = live();
        reset_peak();
        // Two more generations make the unlink's releases reusable (G+2).
        let mut sb = unlinked.clone();
        for tick in 4..6 {
            files.setattr(2, &touch(tick), at(tick)).unwrap();
            sb = commit(&mut files, &disk, &mut committer);
        }
        let promotion_peak = peak() - promotion_start;
        (unlinked, sb, unlink_peak, promotion_peak)
    });
    let (unlinked, sb, unlink_peak, promotion_peak) = measured;
    let [fs_edits, space_edits] = btree::measurement::take_tree_edits();
    let quarantined: u64 = pending(&disk, &unlinked).iter()
        .filter(|&&(_, _, generation)| generation == unlinked.generation)
        .map(|&(_, length, _)| length).sum();
    println!("R13_UNLINK files={FILES} space_edits={space_edits} fs_edits={fs_edits} unlink_peak={unlink_peak} \
promotion_peak={promotion_peak} largest={largest} quarantined={quarantined} pending_items={} free_items={}",
        pending(&disk, &unlinked).len(), free(&disk, &sb).len());
    assert!(quarantined >= FILES as u64, "every file's data block entered PENDING");
    assert!(pending(&disk, &sb).iter().all(|&(_, _, g)| g > unlinked.generation),
        "G+2 returned the unlink's releases to FREE");
    assert!(space_edits <= (FILES + FILES / 4) as u64,
        "SPACE edits {space_edits} for {FILES} unlinks and their reuse at G+2");
    // The unlink commit itself tracks each released extent; the promotion
    // commits must not hold anything per released record.
    assert!(unlink_peak <= (FILES * 256) as isize, "unlink commit heap peak {unlink_peak}");
    assert!(promotion_peak <= 256 * 1024, "promotion heap peak {promotion_peak}");
    // A node buffer is the largest allocation BreenFS needs for this work.
    assert!(largest <= 64 * 1024, "largest allocation {largest}");
}

/// Design 2.4/2.5: a generation's releases that are adjacent on the device
/// enter PENDING as one item, the item is not reusable before G+2, and at G+2
/// it becomes FREE coalesced with its neighbors (A2).
#[test]
fn adjacent_releases_are_one_pending_run_returned_at_g_plus_2() {
    const FILES: usize = 64;
    let disk = Disk::new(16_384);
    let sb = superblock(0x32, disk.blocks);
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
    let mut inos = Vec::new();
    for n in 0..FILES {
        let ino = files.create(2, format!("p{n:02}").as_bytes(), 0o100644, at(1)).unwrap();
        files.write(&disk, ino, 0, &block_pattern(n), at(2)).unwrap();
        inos.push(ino);
    }
    commit(&mut files, &disk, &mut committer);
    let released: Vec<(u64, u64)> = inos.iter()
        .flat_map(|&ino| data_extents(&files, ino))
        .map(|e| (e.phys_start, u64::from(e.phys_len)))
        .collect();
    for n in 0..FILES {
        files.unlink(2, format!("p{n:02}").as_bytes(), at(3)).unwrap();
    }
    let g = commit(&mut files, &disk, &mut committer);
    let runs: Vec<_> = pending(&disk, &g).into_iter().filter(|&(_, _, r)| r == g.generation).collect();
    println!("R13_PENDING releases={} runs={runs:?}", released.len());
    for &(start, length) in &released {
        assert!(runs.iter().any(|&(s, l, _)| s <= start && start + length <= s + l),
            "{start}+{length} quarantined at G={}", g.generation);
    }
    for pair in runs.windows(2) {
        assert!(pair[0].0 + pair[0].1 < pair[1].0, "adjacent PENDING items of one generation: {pair:?}");
    }
    // The data was written in one pass, so its releases are a few runs.
    assert!(runs.len() <= 4, "{} releases became {} PENDING items", released.len(), runs.len());

    // G+1 may not reuse them; G+2 returns them to FREE.
    files.setattr(2, &touch(4), at(4)).unwrap();
    let g1 = commit(&mut files, &disk, &mut committer);
    assert!(pending(&disk, &g1).iter().any(|&(_, _, r)| r == g.generation));
    files.setattr(2, &touch(5), at(5)).unwrap();
    let g2 = commit(&mut files, &disk, &mut committer);
    assert!(pending(&disk, &g2).iter().all(|&(_, _, r)| r != g.generation));
    let free = free(&disk, &g2);
    for &(start, length) in &released {
        assert!(free.iter().any(|&(s, l)| s <= start && start + length <= s + l), "{start}+{length} FREE at G+2");
    }
    for pair in free.windows(2) {
        assert!(pair[0].0 + pair[0].1 < pair[1].0, "A2: adjacent FREE items {pair:?}");
    }
    assert_eq!(g2.free_blocks, free.iter().map(|&(_, l)| l).sum::<u64>(), "S4");
    crate::test_image::check(&disk.bytes.lock().unwrap(), "r13-pending-runs");
}

/// A file's later data is allocated with a goal at its existing extent: it
/// lands at or after that extent, not in an unrelated hole freed below it.
#[test]
fn later_data_lands_after_the_files_existing_extent() {
    let disk = Disk::new(16_384);
    let sb = superblock(0x33, disk.blocks);
    let mut committer = Committer::format(sb.clone());
    let mut files = Files::format(DeviceStore(&disk), &sb).unwrap();
    let hole = files.create(2, b"hole", 0o100644, at(1)).unwrap();
    files.write(&disk, hole, 0, &vec![7; 64 * BLOCK_SIZE], at(1)).unwrap();
    let file = files.create(2, b"file", 0o100644, at(1)).unwrap();
    files.write(&disk, file, 0, &vec![9; 8 * BLOCK_SIZE], at(1)).unwrap();
    commit(&mut files, &disk, &mut committer);
    let hole_extents = data_extents(&files, hole);
    let first = data_extents(&files, file);
    assert_eq!(first.len(), 1);
    let file_end = first[0].phys_start + u64::from(first[0].phys_len);
    files.unlink(2, b"hole", at(2)).unwrap();
    commit(&mut files, &disk, &mut committer);
    let mut sb = sb;
    for tick in 3..5 {
        files.setattr(2, &touch(tick), at(tick)).unwrap();
        sb = commit(&mut files, &disk, &mut committer);
    }
    let free = free(&disk, &sb);
    for e in &hole_extents {
        assert!(free.iter().any(|&(s, l)| s <= e.phys_start && e.phys_start + 8 <= s + l),
            "the freed hole at {} can hold the next run", e.phys_start);
    }
    files.write(&disk, file, 8 * BLOCK_SIZE as u64, &vec![10; 8 * BLOCK_SIZE], at(5)).unwrap();
    commit(&mut files, &disk, &mut committer);
    let extents = data_extents(&files, file);
    println!("R13_GOAL hole={:?} file={:?}", hole_extents.iter().map(|e| e.phys_start).collect::<Vec<_>>(),
        extents.iter().map(|e| (e.phys_start, e.phys_len)).collect::<Vec<_>>());
    assert_eq!(extents.len(), 2);
    assert!(extents[1].phys_start >= file_end,
        "append at {} placed below the file's extent ending at {file_end}", extents[1].phys_start);
    // No free range of 8 or more blocks starts between the goal and the choice.
    assert!(free.iter().all(|&(s, l)| !(s >= file_end && s < extents[1].phys_start && l >= 8)),
        "a closer free range was skipped: {free:?}");
}
