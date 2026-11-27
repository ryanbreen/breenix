//! Flush-boundary crash replay (breenfs design section 5).
use crate::{
    block::{BlockDevice, BlockError},
    minimize::{Case, Fault, Outcome},
    model::Model,
    recording::{Event, RecordingDevice, SECTOR_SIZE},
    stream::Stream,
    Action, Errno, Fs, Operation, Result, Roots, Time, WriterAdapter,
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec,
    vec::Vec,
};
use spin::Mutex;

/// Sparse bytes of a plain in-memory sector device.
/// Omitted sectors read as specified by Image::read_sector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Image {
    pub sectors: BTreeMap<u64, [u8; SECTOR_SIZE]>,
    pub sector_count: u64,
}
impl Image {
    fn from_bytes(bytes: &[u8]) -> Self {
        assert_eq!(bytes.len() % SECTOR_SIZE, 0);
        let mut image = Self {
            sectors: BTreeMap::new(),
            sector_count: (bytes.len() / SECTOR_SIZE) as u64,
        };
        for (sector, bytes) in bytes.chunks_exact(SECTOR_SIZE).enumerate() {
            image.write_sector(sector as u64, bytes.try_into().unwrap());
        }
        image
    }
    fn write_sector(&mut self, sector: u64, bytes: [u8; SECTOR_SIZE]) {
        if bytes == [0; SECTOR_SIZE] {
            self.sectors.remove(&sector);
        } else {
            self.sectors.insert(sector, bytes);
        }
    }
    fn read_sector(&self, sector: u64) -> [u8; SECTOR_SIZE] {
        self.sectors
            .get(&sector)
            .copied()
            .unwrap_or([0; SECTOR_SIZE])
    }
    fn apply(&mut self, writes: &[(u64, [u8; SECTOR_SIZE])], mask: &[bool]) {
        for (&(sector, bytes), &keep) in writes.iter().zip(mask) {
            if keep {
                self.write_sector(sector, bytes);
            }
        }
    }
    fn tear(&mut self, durable: &Image, blocks: &BTreeSet<u64>, seed: u64, tail: TornTail) {
        for &block in blocks {
            // Prefixes do not depend on the tail kind, or on garbage consumption
            // in earlier blocks. Keep a nonempty proper prefix (1..=7 sectors).
            let mut random = seed ^ block.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let prefix = random_word(&mut random) % 7 + 1;
            for sector in block * 8 + prefix..block * 8 + 8 {
                let bytes = match tail {
                    TornTail::Old => durable.read_sector(sector),
                    TornTail::Garbage => {
                        let mut bytes = [0; SECTOR_SIZE];
                        for word in bytes.chunks_exact_mut(8) {
                            word.copy_from_slice(&random_word(&mut random).to_le_bytes());
                        }
                        bytes
                    }
                };
                self.write_sector(sector, bytes);
            }
        }
    }
}

// Recovery must be permitted to clear abandoned slots or reclaim orphans.
// This device has ordinary memory reads/writes, no recorder and no W1-W3 oracle.
struct MemoryDevice(Mutex<Image>);
impl BlockDevice for MemoryDevice {
    fn read_block(&self, sector: u64, bytes: &mut [u8]) -> core::result::Result<(), BlockError> {
        let image = self.0.lock();
        if sector >= image.sector_count {
            return Err(BlockError::OutOfBounds);
        }
        let out = bytes.get_mut(..SECTOR_SIZE).ok_or(BlockError::IoError)?;
        out.copy_from_slice(&image.read_sector(sector));
        Ok(())
    }
    fn write_block(&self, sector: u64, bytes: &[u8]) -> core::result::Result<(), BlockError> {
        let mut image = self.0.lock();
        if sector >= image.sector_count {
            return Err(BlockError::OutOfBounds);
        }
        let bytes = bytes.get(..SECTOR_SIZE).ok_or(BlockError::IoError)?;
        image.write_sector(sector, bytes.try_into().unwrap());
        Ok(())
    }
    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }
    fn num_blocks(&self) -> u64 {
        self.0.lock().sector_count
    }
    fn flush(&self) -> core::result::Result<(), BlockError> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct Boundary {
    pub writes: usize,
    /// Write selections the boundary enumerates (design section 5).
    pub images: usize,
    /// Selections validated: `images` for the full corpus, fewer for a sample.
    pub validated: usize,
    pub acknowledged: u64,
    pub issued: u64,
}

/// Which of a boundary's write selections a corpus run validates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Masks {
    /// The design section 5 corpus: subsets() yields N selections; N are validated.
    Every,
    /// The default-run slice: the 2^writes exhaustive selections (at most 12 writes),
    /// and above 12 writes the writes+1 prefixes, the writes omissions and one
    /// random subset in k.
    Sample(usize),
    /// Small per-window slice: empty/partial/full prefixes, endpoint omissions
    /// and the first random subset; small windows remain exhaustive.
    Representative,
}

impl Masks {
    pub fn keeps(self, writes: usize, case: usize) -> bool {
        match self {
            Masks::Every => true,
            Masks::Representative => {
                writes <= 12
                    || [
                        0,
                        1,
                        writes / 2,
                        writes - 1,
                        writes,
                        writes + 1,
                        writes * 2,
                        writes * 2 + 1,
                    ]
                    .contains(&case)
            }
            Masks::Sample(k) => {
                writes <= 12 || case <= writes * 2 || (case - writes * 2 - 1) % k == 0
            }
        }
    }
}
#[derive(Debug)]
pub struct Report {
    pub commits: usize,
    pub boundaries: Vec<Boundary>,
}
struct Window {
    durable: Image,
    writes: Vec<(u64, [u8; SECTOR_SIZE])>,
    acknowledged: u64,
    issued: u64,
    operation: usize,
}
/// The source run's flush windows and its model state at each published commit.
struct Recorded {
    seed: u64,
    windows: Vec<Window>,
    snapshots: BTreeMap<u64, Roots>,
}

/// Replay the bounded seeded stream; validate its crash images per design section 5.
/// The checker receives the image after writable recovery, including cleanup writes.
/// A final clean remount commits any pending operations in a short stream.
pub fn subset_images(
    initial: Vec<u8>,
    seed: u64,
    operations: usize,
    checker: impl FnMut(&Image) -> Result<()>,
) -> Result<Report> {
    replay(initial, seed, operations, None, Masks::Every, checker)
}

/// `subset_images` over the selections `masks` keeps (a deterministic sample).
pub fn subset_images_of(
    initial: Vec<u8>,
    seed: u64,
    operations: usize,
    masks: Masks,
    checker: impl FnMut(&Image) -> Result<()>,
) -> Result<Report> {
    replay(initial, seed, operations, None, masks, checker)
}

fn replay(
    initial: Vec<u8>,
    seed: u64,
    operations: usize,
    tail: Option<TornTail>,
    keep: Masks,
    mut checker: impl FnMut(&Image) -> Result<()>,
) -> Result<Report> {
    let recorded = record(
        initial,
        seed,
        Stream::new(seed).take(operations).enumerate(),
        operations,
    )?;
    let mut report = Report {
        commits: recorded.snapshots.len() - 1,
        boundaries: Vec::new(),
    };
    for (boundary, window) in recorded.windows.iter().enumerate() {
        let masks = subsets(window.writes.len(), seed.wrapping_add(boundary as u64))?;
        let mut validated = 0;
        for (case, mask) in masks.iter().enumerate() {
            if !keep.keeps(window.writes.len(), case) {
                continue;
            }
            let tear = tail.map(|tail| (tail, tear_seed(seed, boundary, case)));
            validate(&recorded, boundary, case, mask, tear, &mut checker)?;
            validated += 1;
        }
        report.boundaries.push(Boundary {
            writes: window.writes.len(),
            images: masks.len(),
            validated,
            acknowledged: window.acknowledged,
            issued: window.issued,
        });
    }
    Ok(report)
}

/// Run the operations on the model and on the writer over the recording device,
/// then a final clean remount numbered `end`. Indices name the seeded stream's
/// positions, so a minimized stream reports the original operation numbers.
fn record(
    initial: Vec<u8>,
    seed: u64,
    operations: impl Iterator<Item = (usize, Operation)>,
    end: usize,
) -> Result<Recorded> {
    record_with(initial, seed, operations, end, false)
}

/// `record`, optionally with a writer that continues each published
/// generation (WriterAdapter::continuing) as the kernel's mount does.
fn record_with(
    initial: Vec<u8>,
    seed: u64,
    operations: impl Iterator<Item = (usize, Operation)>,
    end: usize,
    continuing: bool,
) -> Result<Recorded> {
    let mut durable = Image::from_bytes(&initial);
    let device = Arc::new(RecordingDevice::new(initial).map_err(|_| Errno::EIO)?);
    let mut writer = if continuing {
        WriterAdapter::continuing(device.clone())?
    } else {
        WriterAdapter::new(device.clone())?
    };
    let mut model = Model::new();
    assert_eq!(
        writer.roots()?,
        model.roots()?,
        "seed={seed}: initial model"
    );
    let initial_generation = *device.durable_generations().last().ok_or(Errno::EIO)?;
    let mut snapshots = BTreeMap::from([(initial_generation, model.roots()?)]);
    let mut acknowledged = initial_generation;
    let mut issued = initial_generation;
    let mut generation = initial_generation;
    let mut cursor = 0;
    let mut pending = Vec::new();
    let mut windows = Vec::new();
    let stream = operations.chain(core::iter::once((
        end,
        Operation {
            at: Time::default(),
            action: Action::Remount,
        },
    )));
    for (index, op) in stream {
        let expected = model.execute(&op);
        assert_eq!(
            writer.execute(&op),
            expected,
            "seed={seed} op={index}: {op:?}"
        );
        assert_eq!(
            writer.roots()?,
            model.roots()?,
            "seed={seed} op={index}: {op:?}"
        );
        let committed = *device.durable_generations().last().ok_or(Errno::EIO)?;
        if committed != generation {
            assert_eq!(
                committed,
                generation + 1,
                "seed={seed} op={index}: commit snapshot"
            );
            snapshots.insert(committed, model.roots()?);
            generation = committed;
        }
        let events = device.events();
        for event in &events[cursor..] {
            match event {
                Event::Write { sector, bytes } => {
                    // The first sector carries the generation; publication may
                    // survive even before the other sectors or ring are issued.
                    let block = sector / 8;
                    let total = durable.sector_count / 8;
                    if sector % 8 == 0
                        && ((16..20).contains(&block) || (total - 4..total).contains(&block))
                        && &bytes[..8] == b"BREENFSS"
                    {
                        issued = issued.max(u64::from_le_bytes(bytes[32..40].try_into().unwrap()));
                    }
                    pending.push((*sector, *bytes));
                }
                Event::Flush => {
                    windows.push(Window {
                        durable: durable.clone(),
                        writes: pending.clone(),
                        acknowledged,
                        issued,
                        operation: index,
                    });
                    durable.apply(&pending, &vec![true; pending.len()]);
                    pending.clear();
                }
            }
        }
        cursor = events.len();
        // An operation's flush boundaries precede its successful fsync receipt.
        // A clean remount commits too, but is not a returned fsync call.
        if expected.is_ok() && matches!(op.action, Action::Fsync { .. }) {
            acknowledged = committed;
        }
    }
    assert!(pending.is_empty(), "seed={seed}: unflushed source writes");
    assert_eq!(
        durable,
        Image::from_bytes(&device.durable_image()),
        "seed={seed}: event replay"
    );
    Ok(Recorded {
        seed,
        windows,
        snapshots,
    })
}

/// Build one crash image from a window and apply the corpus acceptance to it:
/// writable recovery, the checker, and a model commit inside the window's bounds.
fn validate(
    recorded: &Recorded,
    boundary: usize,
    case: usize,
    mask: &[bool],
    tear: Option<(TornTail, u64)>,
    checker: &mut impl FnMut(&Image) -> Result<()>,
) -> Result<()> {
    let seed = recorded.seed;
    let window = &recorded.windows[boundary];
    let tail = tear.map(|(tail, _)| tail);
    let mut image = window.durable.clone();
    image.apply(&window.writes, mask);
    if let Some((tail, tear_seed)) = tear {
        let blocks = window.writes.iter().map(|(sector, _)| sector / 8).collect();
        image.tear(&window.durable, &blocks, tear_seed, tail);
    }
    let crash = Arc::new(MemoryDevice(Mutex::new(image)));
    let recovered = WriterAdapter::new(crash.clone()).unwrap_or_else(|error| {
        panic!(
            "seed={seed} op={} boundary={boundary} image={case} tail={tail:?}: recover {error:?}",
            window.operation
        )
    });
    let image = crash.0.lock().clone();
    assert!(
        checker(&image).is_ok(),
        "seed={seed} op={} boundary={boundary} image={case} tail={tail:?}: checker",
        window.operation
    );
    let state = recovered.roots()?;
    assert!(matches_commit(&recorded.snapshots, window.acknowledged, window.issued, &state),
        "seed={seed} op={} boundary={boundary} image={case} tail={tail:?}: state outside commits {}..={}: {state:?}",
        window.operation, window.acknowledged, window.issued);
    Ok(())
}

fn tear_seed(seed: u64, boundary: usize, case: usize) -> u64 {
    seed ^ (boundary as u64).wrapping_mul(0xd1b5_4a32_d192_ed03)
        ^ (case as u64).wrapping_mul(0x94d0_49bb_1331_11eb)
}

/// Name a corpus crash image (the seed, boundary, image index and tail that a
/// subset or torn failure reports) as a minimizable case over the full stream.
pub fn case_at(
    initial: Vec<u8>,
    seed: u64,
    operations: usize,
    boundary: usize,
    image: usize,
    tail: Option<TornTail>,
) -> Result<Case> {
    let recorded = record(
        initial,
        seed,
        Stream::new(seed).take(operations).enumerate(),
        operations,
    )?;
    let window = recorded.windows.get(boundary).ok_or(Errno::EINVAL)?;
    let writes = window.writes.len();
    let masks = subsets(writes, seed.wrapping_add(boundary as u64))?;
    let mask = masks.get(image).ok_or(Errno::EINVAL)?;
    Ok(Case::seeded(
        seed,
        operations,
        Fault::Crash {
            boundary,
            writes,
            selected: (0..writes).filter(|&i| mask[i]).collect(),
            tail,
            tear_seed: tear_seed(seed, boundary, image),
        },
    ))
}

/// Replay one regression row through the corpus acceptance. A case whose
/// boundary or write count does not exist in its stream is inapplicable.
pub fn replay_case(
    initial: Vec<u8>,
    case: &Case,
    mut checker: impl FnMut(&Image) -> Result<()>,
) -> Result<Outcome> {
    let Fault::Crash {
        boundary,
        writes,
        ref selected,
        tail,
        tear_seed,
    } = case.fault
    else {
        return Ok(Outcome::Inapplicable);
    };
    if !case.well_formed() {
        return Ok(Outcome::Inapplicable);
    }
    let recorded = record(
        initial,
        case.seed,
        case.indexed_operations().into_iter(),
        case.operation_count,
    )?;
    if recorded.windows.get(boundary).map(|w| w.writes.len()) != Some(writes) {
        return Ok(Outcome::Inapplicable);
    }
    let mask: Vec<bool> = (0..writes)
        .map(|i| selected.binary_search(&i).is_ok())
        .collect();
    let tear = tail.map(|tail| (tail, tear_seed));
    validate(&recorded, boundary, 0, &mask, tear, &mut checker)?;
    Ok(Outcome::Accepted)
}

/// Bytes left after the new sector prefix of a torn 4 KiB write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TornTail {
    Old,
    Garbage,
}

/// Validate torn variants of the same flush-boundary selections as subset_images.
pub fn torn_images(
    initial: Vec<u8>,
    seed: u64,
    operations: usize,
    tail: TornTail,
    checker: impl FnMut(&Image) -> Result<()>,
) -> Result<Report> {
    replay(initial, seed, operations, Some(tail), Masks::Every, checker)
}

/// `torn_images` over the selections `masks` keeps (a deterministic sample).
pub fn torn_images_of(
    initial: Vec<u8>,
    seed: u64,
    operations: usize,
    tail: TornTail,
    masks: Masks,
    checker: impl FnMut(&Image) -> Result<()>,
) -> Result<Report> {
    replay(initial, seed, operations, Some(tail), masks, checker)
}

fn matches_commit(
    snapshots: &BTreeMap<u64, Roots>,
    acknowledged: u64,
    issued: u64,
    state: &Roots,
) -> bool {
    snapshots
        .range(acknowledged..=issued)
        .any(|(_, expected)| expected == state)
}

/// Ordered write selections from design section 5. Duplicate selections retain
/// their own validation: prefixes and omissions may also occur in random subsets.
pub fn subsets(writes: usize, seed: u64) -> Result<Vec<Vec<bool>>> {
    if writes <= 12 {
        return Ok((0..1usize << writes)
            .map(|bits| (0..writes).map(|bit| bits & (1 << bit) != 0).collect())
            .collect());
    }
    let mut masks = Vec::with_capacity(writes * 2 + 4097);
    for prefix in 0..=writes {
        masks.push((0..writes).map(|i| i < prefix).collect());
    }
    for omission in 0..writes {
        masks.push((0..writes).map(|i| i != omission).collect());
    }
    let mut random = if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    };
    for _ in 0..4096 {
        masks.push(
            (0..writes)
                .map(|_| {
                    random ^= random >> 12;
                    random ^= random << 25;
                    random ^= random >> 27;
                    random.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 63 != 0
                })
                .collect(),
        );
    }
    Ok(masks)
}

fn random_word(state: &mut u64) -> u64 {
    if *state == 0 {
        *state = 0x9e37_79b9_7f4a_7c15;
    }
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_crash_images_sample_matches_acknowledged_roots() {
        use crate::stream::snapshot_seed;
        use std::{
            fs::OpenOptions,
            io::{Seek, SeekFrom, Write},
            process::Command,
            time::Instant,
        };
        let started = Instant::now();
        let seed = snapshot_seed(0x2414);
        let initial = crate::test_image::image();
        let recorded = record(initial, seed, Stream::new(seed).take(24).enumerate(), 24).unwrap();
        // Creation, the last deletion and successive reaper publications have
        // their own data and superblock flush windows.
        for operation in [12, 20, 21, 22] {
            assert_eq!(
                recorded
                    .windows
                    .iter()
                    .filter(|w| w.operation == operation)
                    .count(),
                2
            );
        }
        assert!(recorded
            .snapshots
            .values()
            .any(|roots| roots.contains_key(b"frozen".as_slice())));
        assert!(!recorded
            .snapshots
            .last_key_value()
            .unwrap()
            .1
            .contains_key(b"frozen".as_slice()));
        let path = std::env::temp_dir().join(alloc::format!(
            "breenfs-snapshot-crash-{}.img",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut checks = 0;
        let mut mid_reap = false;
        for (boundary, window) in recorded.windows.iter().enumerate() {
            let selections =
                subsets(window.writes.len(), seed.wrapping_add(boundary as u64)).unwrap();
            for (case, mask) in selections.iter().enumerate() {
                if !Masks::Representative.keeps(window.writes.len(), case) {
                    continue;
                }
                validate(&recorded, boundary, case, mask, None, &mut |image| {
                    file.set_len(0).unwrap();
                    file.set_len(image.sector_count * SECTOR_SIZE as u64)
                        .unwrap();
                    for (&sector, bytes) in &image.sectors {
                        file.seek(SeekFrom::Start(sector * SECTOR_SIZE as u64))
                            .unwrap();
                        file.write_all(bytes).unwrap();
                    }
                    file.flush().unwrap();
                    let output = Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-check"))
                        .arg("--full")
                        .arg(&path)
                        .output()
                        .unwrap();
                    assert_eq!(
                        output.status.code(),
                        Some(0),
                        "boundary={boundary} image={case}: {output:?}"
                    );
                    let dev = MemoryDevice(Mutex::new(image.clone()));
                    let sb = crate::recover::recover(&dev, false).unwrap();
                    let dead = crate::recover::space_items(&crate::commit::DeviceStore(&dev), &sb)
                        .unwrap()
                        .iter()
                        .filter(|(key, _)| key.item_type == crate::format::DEAD)
                        .count();
                    mid_reap |= window.operation == 21 && dead > 1;
                    checks += 1;
                    Ok(())
                })
                .unwrap();
            }
        }
        assert!(mid_reap, "sample must retain a committed reaper frontier");
        assert!(checks > recorded.windows.len());
        std::fs::remove_file(path).unwrap();
        std::eprintln!(
            "snapshot crash sample: {checks} checked images in {:?}",
            started.elapsed()
        );
        assert!(started.elapsed().as_secs_f64() < 60.0);
    }

    /// PENDING ranges (start, length) released in `generation`, read from a
    /// durable image after read-only recovery.
    fn released_in(image: &Image, generation: u64) -> Vec<(u64, u64)> {
        use crate::{btree, commit::DeviceStore, format};
        let dev = MemoryDevice(Mutex::new(image.clone()));
        let sb = crate::recover::recover(&dev, false).unwrap();
        let object = 0xffff_ffff_ffff_ff30;
        btree::range(&DeviceStore(&dev), sb.space_root,
            format::Key { object, item_type: format::PENDING, offset: 0 },
            format::Key { object, item_type: format::PENDING, offset: u64::MAX })
            .unwrap()
            .into_iter()
            .filter_map(|(key, bytes)| {
                let p = format::Pending::decode(&bytes).unwrap();
                (p.released_generation == generation).then_some((key.offset, p.length))
            })
            .collect()
    }

    /// Audit R13: one generation's releases enter PENDING, are promoted two
    /// generations later and are then rewritten, through a mount that
    /// continues each published generation (the kernel's path, which carries
    /// the quarantine in memory). Each representative crash image of each
    /// flush window recovers to an acknowledged commit and passes the full
    /// checker, and the recording device refuses any write into the two
    /// newest durable generations (writer rule W1).
    #[test]
    fn promotion_at_g_plus_2_crash_images_recover() {
        use std::{
            fs::OpenOptions,
            io::{Seek, SeekFrom, Write},
            process::Command,
            time::Instant,
        };
        let started = Instant::now();
        let create = |name: &[u8]| Action::Create { dir: 2, name: name.to_vec(), mode: 0o644 };
        let write = |ino, blocks: usize, byte| Action::Write { ino, offset: 0, bytes: vec![byte; blocks * 4096] };
        // Inodes are numbered from 16 in creation order (model and writer).
        let actions = [
            create(b"a"), write(16, 8, 1), create(b"b"), write(17, 8, 2), Action::Fsync { ino: 16 },
            Action::Unlink { dir: 2, name: b"a".to_vec() }, Action::Unlink { dir: 2, name: b"b".to_vec() },
            Action::Fsync { ino: 2 },                                   // 7: G, the releases
            create(b"c"), Action::Fsync { ino: 18 },                    // 9: G+1
            create(b"d"), write(19, 4, 3), Action::Fsync { ino: 19 },   // 12: G+2, promotion
            create(b"e"), write(20, 16, 4), Action::Fsync { ino: 20 },  // 15: reuse
        ];
        let operations: Vec<_> = actions.into_iter().enumerate()
            .map(|(i, action)| (i, Operation { at: Time { seconds: 100 + i as i64, nanos: 0 }, action }))
            .collect();
        let end = operations.len();
        let seed = 0x13_13;
        let recorded = record_with(crate::test_image::image(), seed, operations.into_iter(), end, true).unwrap();
        let first = |operation| recorded.windows.iter().position(|w| w.operation == operation).unwrap();
        // The durable image when G+1 starts writing holds G's quarantine.
        let g_plus_1 = &recorded.windows[first(9)];
        let released = released_in(&g_plus_1.durable, g_plus_1.acknowledged);
        assert!(!released.is_empty(), "G released the unlinked files' blocks");
        let written_into_release = |operation| recorded.windows.iter()
            .filter(|w| w.operation == operation)
            .flat_map(|w| &w.writes)
            .any(|(sector, _)| released.iter().any(|&(s, l)| (s..s + l).contains(&(sector / 8))));
        assert!(!written_into_release(9), "G+1 wrote a block G released");
        assert!(written_into_release(12) || written_into_release(15), "released blocks were reused after G+2");

        let path = std::env::temp_dir().join(alloc::format!("breenfs-promotion-crash-{}.img", std::process::id()));
        let mut file = OpenOptions::new().create_new(true).write(true).open(&path).unwrap();
        let mut checks = 0;
        for (boundary, window) in recorded.windows.iter().enumerate() {
            let selections = subsets(window.writes.len(), seed.wrapping_add(boundary as u64)).unwrap();
            for (case, mask) in selections.iter().enumerate() {
                if !Masks::Representative.keeps(window.writes.len(), case) {
                    continue;
                }
                validate(&recorded, boundary, case, mask, None, &mut |image| {
                    file.set_len(0).unwrap();
                    file.set_len(image.sector_count * SECTOR_SIZE as u64).unwrap();
                    for (&sector, bytes) in &image.sectors {
                        file.seek(SeekFrom::Start(sector * SECTOR_SIZE as u64)).unwrap();
                        file.write_all(bytes).unwrap();
                    }
                    file.flush().unwrap();
                    let output = Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-check"))
                        .arg("--full").arg(&path).output().unwrap();
                    assert_eq!(output.status.code(), Some(0), "boundary={boundary} image={case}: {output:?}");
                    checks += 1;
                    Ok(())
                })
                .unwrap();
            }
        }
        assert!(checks > recorded.windows.len());
        std::fs::remove_file(path).unwrap();
        std::eprintln!("promotion crash sample: {checks} checked images in {} windows in {:?}",
            recorded.windows.len(), started.elapsed());
    }

    #[test]
    #[ignore = "full snapshot crash corpus: opt-in breenfs battery only, never per change"]
    fn snapshot_crash_images_full_corpus() {
        use std::{
            fs::OpenOptions,
            io::{Seek, SeekFrom, Write},
            process::Command,
        };
        let path = std::env::temp_dir().join(alloc::format!(
            "breenfs-snapshot-full-{}.img",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        let report = subset_images(
            crate::test_image::image(),
            crate::stream::snapshot_seed(0x2414),
            24,
            |image| {
                file.set_len(0).unwrap();
                file.set_len(image.sector_count * SECTOR_SIZE as u64)
                    .unwrap();
                for (&sector, bytes) in &image.sectors {
                    file.seek(SeekFrom::Start(sector * SECTOR_SIZE as u64))
                        .unwrap();
                    file.write_all(bytes).unwrap();
                }
                file.flush().unwrap();
                let output = Command::new(env!("CARGO_BIN_FILE_BREENFS_breenfs-check"))
                    .arg("--full")
                    .arg(&path)
                    .output()
                    .unwrap();
                assert_eq!(output.status.code(), Some(0), "{output:?}");
                Ok(())
            },
        )
        .unwrap();
        assert!(report
            .boundaries
            .iter()
            .all(|boundary| boundary.images == boundary.validated));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn seed_1_garbage_tail_in_unpublished_ring_slots() {
        // Minimized from stream seed 1, boundary 1, mask 0 (no sector
        // selected): A2/B2 have a zero prefix and seeded garbage tails.
        // The preceding commit's unreferenced data does not affect recovery.
        // claim-lint:ok: Minimized seed 1 fixture below; 2 of 2 slots retain old prefix sectors under Image::tear with mask 0.
        fn word(state: &mut u64) -> u64 {
            *state ^= *state >> 12;
            *state ^= *state << 25;
            *state ^= *state >> 27;
            state.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        let mut bytes = crate::test_image::image();
        let total = bytes.len() / 4096;
        let blocks = [18, total as u64 - 2];
        for block in blocks {
            let mut random = 1 ^ 0xd1b5_4a32_d192_ed03 ^ block.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let prefix = word(&mut random) % 7 + 1;
            let start = (block * 8 + prefix) as usize * SECTOR_SIZE;
            let end = (block as usize + 1) * 4096;
            for chunk in bytes[start..end].chunks_exact_mut(8) {
                chunk.copy_from_slice(&word(&mut random).to_le_bytes());
            }
        }
        let image = Image::from_bytes(&bytes);
        let device = Arc::new(MemoryDevice(Mutex::new(image.clone())));
        assert_eq!(
            crate::recover::recover(device.as_ref(), false)
                .unwrap()
                .generation,
            1
        );
        assert_eq!(
            *device.0.lock(),
            image,
            "read-only recovery wrote the image"
        );
        let recovered = WriterAdapter::new(device.clone()).unwrap();
        assert_eq!(recovered.state().unwrap(), Model::new().state().unwrap());
        let image = device.0.lock();
        for block in blocks {
            for sector in block * 8..block * 8 + 8 {
                assert_eq!(
                    image.read_sector(sector),
                    [0; SECTOR_SIZE],
                    "invalid slot at block {block} was retained"
                );
            }
        }
        for (&sector, value) in &image.sectors {
            let at = sector as usize * SECTOR_SIZE;
            bytes[at..at + SECTOR_SIZE].copy_from_slice(value);
        }
        // Explicit zeros are absent from sparse storage.
        for block in blocks {
            bytes[block as usize * 4096..(block as usize + 1) * 4096].fill(0);
        }
        crate::test_image::check(&bytes, "seed-1-garbage-slots");
    }
}
