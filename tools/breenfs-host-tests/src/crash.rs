//! Flush-boundary crash replay (breenfs design section 5).
use crate::{
    block::{BlockDevice, BlockError},
    model::Model,
    recording::{Event, RecordingDevice, SECTOR_SIZE},
    stream::Stream,
    Action, Errno, Fs, Operation, Result, State, Time, WriterAdapter,
};
use alloc::{collections::BTreeMap, sync::Arc, vec, vec::Vec};
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
    pub images: usize,
    pub acknowledged: u64,
    pub issued: u64,
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

/// Replay the bounded seeded stream; validate its crash images per design section 5.
/// The checker receives the image after writable recovery, including cleanup writes.
/// A final clean remount commits any pending operations in a short stream.
pub fn subset_images(
    initial: Vec<u8>,
    seed: u64,
    operations: usize,
    mut checker: impl FnMut(&Image) -> Result<()>,
) -> Result<Report> {
    let mut durable = Image::from_bytes(&initial);
    let device = Arc::new(RecordingDevice::new(initial).map_err(|_| Errno::EIO)?);
    let mut writer = WriterAdapter::new(device.clone())?;
    let mut model = Model::new();
    assert_eq!(
        writer.state()?,
        model.state()?,
        "seed={seed}: initial model"
    );
    let initial_generation = *device.durable_generations().last().ok_or(Errno::EIO)?;
    let mut snapshots = BTreeMap::from([(initial_generation, model.state()?)]);
    let mut acknowledged = initial_generation;
    let mut issued = initial_generation;
    let mut generation = initial_generation;
    let mut cursor = 0;
    let mut pending = Vec::new();
    let mut windows = Vec::new();
    let stream = Stream::new(seed)
        .take(operations)
        .chain(core::iter::once(Operation {
            at: Time::default(),
            action: Action::Remount,
        }));
    for (index, op) in stream.enumerate() {
        let expected = model.execute(&op);
        assert_eq!(
            writer.execute(&op),
            expected,
            "seed={seed} op={index}: {op:?}"
        );
        assert_eq!(
            writer.state()?,
            model.state()?,
            "seed={seed} op={index}: {op:?}"
        );
        let committed = *device.durable_generations().last().ok_or(Errno::EIO)?;
        if committed != generation {
            assert_eq!(
                committed,
                generation + 1,
                "seed={seed} op={index}: commit snapshot"
            );
            snapshots.insert(committed, model.state()?);
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
    let mut report = Report {
        commits: snapshots.len() - 1,
        boundaries: Vec::new(),
    };
    for (boundary, window) in windows.into_iter().enumerate() {
        let masks = subsets(window.writes.len(), seed.wrapping_add(boundary as u64))?;
        for (case, mask) in masks.iter().enumerate() {
            let mut image = window.durable.clone();
            image.apply(&window.writes, mask);
            let crash = Arc::new(MemoryDevice(Mutex::new(image)));
            let recovered = WriterAdapter::new(crash.clone()).unwrap_or_else(|error| {
                panic!(
                    "seed={seed} op={} boundary={boundary} image={case}: recover {error:?}",
                    window.operation
                )
            });
            let image = crash.0.lock().clone();
            assert!(
                checker(&image).is_ok(),
                "seed={seed} op={} boundary={boundary} image={case}: checker",
                window.operation
            );
            let state = recovered.state()?;
            assert!(matches_commit(&snapshots, window.acknowledged, window.issued, &state),
                "seed={seed} op={} boundary={boundary} image={case}: state outside commits {}..={}: {state:?}",
                window.operation, window.acknowledged, window.issued);
        }
        report.boundaries.push(Boundary {
            writes: window.writes.len(),
            images: masks.len(),
            acknowledged: window.acknowledged,
            issued: window.issued,
        });
    }
    Ok(report)
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
    let _ = (initial, seed, operations, tail, checker);
    Err(crate::Error::Unimplemented)
}

fn matches_commit(
    snapshots: &BTreeMap<u64, State>,
    acknowledged: u64,
    issued: u64,
    state: &State,
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
