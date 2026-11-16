//! Committed-image corruption corpus (breenfs design section 5).
use crate::{
    block::{BlockDevice, BlockError},
    btree::NodeStore,
    commit::DeviceStore,
    crash::Image,
    format::{
        self, Extent, InternalEntry, LeafItemHeader, NodeHeader, Ref, Root, Superblock, BLOCK_SIZE,
    },
    minimize::{Case, Fault, Outcome},
    recording::{RecordingDevice, SECTOR_SIZE},
    recover,
    stream::Stream,
    Action, Contents, Errno, Fs, Result, WriterAdapter,
};
use alloc::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec,
    vec::Vec,
};
use spin::Mutex;

/// Plain recovery device without the healthy-image recording oracle.
pub struct Device(Mutex<Image>);
pub fn device(image: Image) -> Device {
    Device(Mutex::new(image))
}
impl Device {
    pub fn image(&self) -> Image {
        self.0.lock().clone()
    }
}
impl BlockDevice for Device {
    fn read_block(&self, sector: u64, bytes: &mut [u8]) -> core::result::Result<(), BlockError> {
        let image = self.0.lock();
        if sector >= image.sector_count || bytes.len() != SECTOR_SIZE {
            return Err(BlockError::OutOfBounds);
        }
        bytes.copy_from_slice(image.sectors.get(&sector).unwrap_or(&[0; SECTOR_SIZE]));
        Ok(())
    }
    fn write_block(&self, sector: u64, bytes: &[u8]) -> core::result::Result<(), BlockError> {
        let mut image = self.0.lock();
        if sector >= image.sector_count || bytes.len() != SECTOR_SIZE {
            return Err(BlockError::OutOfBounds);
        }
        let bytes: [u8; SECTOR_SIZE] = bytes.try_into().map_err(|_| BlockError::IoError)?;
        if bytes == [0; SECTOR_SIZE] {
            image.sectors.remove(&sector);
        } else {
            image.sectors.insert(sector, bytes);
        }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Slot,
    Node { addr: u64 },
    Data,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Flip {
    pub seed: u64,
    pub operation: Option<usize>,
    pub block: u64,
    /// Bit offset within the flipped 4 KiB block.
    pub bit: usize,
    pub kind: Kind,
}

#[derive(Debug, Default)]
pub struct Report {
    pub committed_images: usize,
    pub slots: usize,
    pub nodes: usize,
    pub data: usize,
}

/// Visit one seeded bit flip per reachable block of the populated formatter
/// image and the committed image after a successful fsync in the fixed streams.
/// File identities and lengths come from the pristine committed image so that
/// an error in a corrupted state walk cannot skip the separate file reads.
pub fn bit_flips(
    populated: Vec<u8>,
    empty: Vec<u8>,
    seeds: &[u64],
    operations: usize,
    mut observe: impl FnMut(&Image, Flip, &[(u64, u64)]) -> Result<()>,
) -> Result<Report> {
    let mut report = Report::default();
    visit(&populated, 0x24_09, None, &mut observe, &mut report)?;
    for &seed in seeds {
        let source = Arc::new(RecordingDevice::new(empty.clone()).map_err(|_| Errno::EIO)?);
        let mut writer = WriterAdapter::new(source.clone())?;
        for (operation, op) in Stream::new(seed).take(operations).enumerate() {
            let result = writer.execute(&op);
            if matches!(op.action, Action::Fsync { .. }) && result.is_ok() {
                visit(
                    &source.durable_image(),
                    seed,
                    Some(operation),
                    &mut observe,
                    &mut report,
                )?;
            }
        }
    }
    Ok(report)
}

fn sparse(bytes: &[u8]) -> Result<Image> {
    if bytes.len() % SECTOR_SIZE != 0 {
        return Err(Errno::EIO.into());
    }
    let sectors = bytes
        .chunks_exact(SECTOR_SIZE)
        .enumerate()
        .filter(|(_, bytes)| bytes.iter().any(|b| *b != 0))
        .map(|(sector, bytes)| (sector as u64, bytes.try_into().unwrap()))
        .collect();
    Ok(Image {
        sectors,
        sector_count: (bytes.len() / SECTOR_SIZE) as u64,
    })
}

fn visit(
    bytes: &[u8],
    seed: u64,
    operation: Option<usize>,
    observe: &mut impl FnMut(&Image, Flip, &[(u64, u64)]) -> Result<()>,
    report: &mut Report,
) -> Result<()> {
    let Committed {
        image,
        files,
        targets,
    } = committed(bytes)?;
    report.committed_images += 1;
    for (block, kind) in targets {
        let mut random = seed
            ^ block.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (operation.map_or(0, |n| n as u64 + 1)).wrapping_mul(0xd1b5_4a32_d192_ed03);
        random ^= random >> 12;
        random ^= random << 25;
        random ^= random >> 27;
        let bit = (random.wrapping_mul(0x2545_f491_4f6c_dd1d) % (BLOCK_SIZE * 8) as u64) as usize;
        let flip = Flip {
            seed,
            operation,
            block,
            bit,
            kind,
        };
        observe(&flipped(&image, block, bit), flip, &files)?;
        match kind {
            Kind::Slot => report.slots += 1,
            Kind::Node { .. } => report.nodes += 1,
            Kind::Data => report.data += 1,
        }
    }
    Ok(())
}

/// A committed image, its regular files (inode, size) from the pristine state
/// walk, and the selected generation's reachable blocks.
struct Committed {
    image: Image,
    files: Vec<(u64, u64)>,
    targets: BTreeMap<u64, Kind>,
}

fn committed(bytes: &[u8]) -> Result<Committed> {
    let image = sparse(bytes)?;
    let pristine = Arc::new(device(image.clone()));
    let writer = WriterAdapter::new(pristine.clone())?;
    let files: Vec<_> = writer
        .state()?
        .inodes
        .into_iter()
        .filter_map(|(ino, inode)| match inode.contents {
            Contents::File { size, .. } => Some((ino, size)),
            _ => None,
        })
        .collect();
    let sb = recover::recover(pristine.as_ref(), false).map_err(|_| Errno::EIO)?;
    let targets = reachable(&pristine, &sb)?;
    Ok(Committed {
        image,
        files,
        targets,
    })
}

fn flipped(image: &Image, block: u64, bit: usize) -> Image {
    let mut corrupt = image.clone();
    let sector = block * 8 + (bit / (SECTOR_SIZE * 8)) as u64;
    let bytes = corrupt.sectors.entry(sector).or_insert([0; SECTOR_SIZE]);
    bytes[bit / 8 % SECTOR_SIZE] ^= 1 << (bit % 8);
    corrupt
}

/// Replay one bit-flip regression row through the corpus observer. `base` is
/// the image flipped directly when the flip names no operation (the populated
/// formatter image), and otherwise the empty image the case's stream runs on.
/// The flip applies only when its operation is a kept, successful fsync and its
/// block is still reachable with the same kind.
pub fn replay_flip(
    base: Vec<u8>,
    case: &Case,
    mut observe: impl FnMut(&Image, Flip, &[(u64, u64)]) -> Result<()>,
) -> Result<Outcome> {
    let Fault::Flip(flip) = case.fault else {
        return Ok(Outcome::Inapplicable);
    };
    if !case.well_formed() || flip.bit >= BLOCK_SIZE * 8 {
        return Ok(Outcome::Inapplicable);
    }
    let bytes = match flip.operation {
        None => base,
        Some(at) => {
            let source = Arc::new(RecordingDevice::new(base).map_err(|_| Errno::EIO)?);
            let mut writer = WriterAdapter::new(source.clone())?;
            let mut durable = None;
            for (index, op) in case.indexed_operations() {
                let result = writer.execute(&op);
                if index == at {
                    if matches!(op.action, Action::Fsync { .. }) && result.is_ok() {
                        durable = Some(source.durable_image());
                    }
                    break;
                }
            }
            match durable {
                Some(bytes) => bytes,
                None => return Ok(Outcome::Inapplicable),
            }
        }
    };
    let Committed {
        image,
        files,
        targets,
    } = committed(&bytes)?;
    if targets.get(&flip.block) != Some(&flip.kind) {
        return Ok(Outcome::Inapplicable);
    }
    observe(&flipped(&image, flip.block, flip.bit), flip, &files)?;
    Ok(Outcome::Accepted)
}

fn reachable(device: &Device, sb: &Superblock) -> Result<BTreeMap<u64, Kind>> {
    let mut blocks = BTreeMap::new();
    // Slots containing earlier committed generations remain part of the rings.
    // Their trees are historical; L is the selected generation's reachable set
    // (design section 3), including DEAD nodes and their outgoing references.
    for ring in 0..2 {
        for slot in 0..4 {
            let block = if ring == 0 {
                16 + u64::from(slot)
            } else {
                sb.total_blocks - 4 + u64::from(slot)
            };
            let image = device.image();
            let mut bytes = [0; BLOCK_SIZE];
            for (n, chunk) in bytes.chunks_exact_mut(SECTOR_SIZE).enumerate() {
                chunk.copy_from_slice(
                    image
                        .sectors
                        .get(&(block * 8 + n as u64))
                        .unwrap_or(&[0; SECTOR_SIZE]),
                );
            }
            if format::valid_superblock_slot(&bytes, sb.total_blocks, ring, slot) {
                blocks.insert(block, Kind::Slot);
            }
        }
    }
    let store = DeviceStore(device);
    let mut todo = vec![sb.space_root.addr];
    let mut seen = BTreeSet::new();
    while let Some(addr) = todo.pop() {
        if !seen.insert(addr) {
            continue;
        }
        let bytes = store.get(addr).map_err(|_| Errno::EIO)?;
        let h = NodeHeader::decode(&bytes).map_err(|_| Errno::EIO)?;
        for block in addr..addr + 4 {
            blocks.insert(block, Kind::Node { addr });
        }
        for n in 0..usize::from(h.nitems) {
            if h.level > 0 {
                let at = 48 + n * 37;
                let entry = InternalEntry::decode(&bytes[at..at + 37]).map_err(|_| Errno::EIO)?;
                todo.push(entry.child_addr);
            } else {
                let at = 48 + n * 21;
                let item = LeafItemHeader::decode(&bytes[at..at + 21]).map_err(|_| Errno::EIO)?;
                let value = &bytes[usize::from(item.data_off)
                    ..usize::from(item.data_off) + usize::from(item.data_len)];
                match item.key.item_type {
                    format::ROOT => {
                        todo.push(Root::decode(value).map_err(|_| Errno::EIO)?.root.addr)
                    }
                    format::DEAD => todo.push(item.key.object),
                    format::REF => {
                        let r = Ref::decode(value).map_err(|_| Errno::EIO)?;
                        if r.kind == 3 {
                            for block in item.key.object..item.key.object + item.key.offset {
                                blocks.insert(block, Kind::Data);
                            }
                        }
                    }
                    format::EXTENT => {
                        let e = Extent::decode(value).map_err(|_| Errno::EIO)?;
                        for block in e.phys_start..e.phys_start + u64::from(e.phys_len) {
                            blocks.insert(block, Kind::Data);
                        }
                    }
                    _ => (),
                }
            }
        }
    }
    Ok(blocks)
}

/// Selected generation and greatest reachable tree height (a leaf has height 1).
pub fn coverage(image: &Image) -> Result<(u64, u8)> {
    let device = device(image.clone());
    let sb = recover::recover(&device, false).map_err(|_| Errno::EIO)?;
    let targets = reachable(&device, &sb)?;
    let mut height = 0;
    for (block, kind) in targets {
        if let Kind::Node { addr } = kind {
            if block == addr {
                let bytes = DeviceStore(&device).get(addr).map_err(|_| Errno::EIO)?;
                height = height.max(NodeHeader::decode(&bytes).map_err(|_| Errno::EIO)?.level + 1);
            }
        }
    }
    Ok((sb.generation, height))
}
