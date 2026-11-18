//! E24 coordinator ledger 558(x3): count one production VFS publication.
use super::super::{
    commit::publication_measurement as measure,
    format::{BlockPtr, InternalEntry, Key, LeafItemHeader},
};
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex as HostMutex, OnceLock},
    time::Instant,
};

#[derive(Default)]
struct Io {
    writes: Vec<u64>,
    flushes: Vec<usize>,
    read_sectors: u64,
}
struct Disk {
    bytes: HostMutex<Vec<u8>>,
    io: HostMutex<Io>,
}
impl BlockDevice for Disk {
    fn block_size(&self) -> usize {
        512
    }
    fn num_blocks(&self) -> u64 {
        self.bytes.lock().unwrap().len() as u64 / 512
    }
    fn read_block(&self, sector: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.io.lock().unwrap().read_sectors += 1;
        let at = sector as usize * 512;
        bytes.copy_from_slice(&self.bytes.lock().unwrap()[at..at + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, sector: u64, bytes: &[u8]) -> Result<(), BlockError> {
        assert_eq!(bytes.len(), 512);
        let at = sector as usize * 512;
        self.bytes.lock().unwrap()[at..at + 512].copy_from_slice(bytes);
        self.io.lock().unwrap().writes.push(sector);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> {
        let mut io = self.io.lock().unwrap();
        let writes = io.writes.len();
        io.flushes.push(writes);
        Ok(())
    }
}

fn image() -> &'static Vec<u8> {
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    IMAGE.get_or_init(|| {
        let disk = Disk {
            bytes: HostMutex::new(vec![0; 16384 * BLOCK_SIZE]),
            io: HostMutex::new(Io::default()),
        };
        let sb = Superblock {
            fs_uuid: [0x53; 16],
            generation: 1,
            slot: 1,
            total_blocks: 16384,
            global_reserve: 4096,
            next_root_id: 2,
            dirhash_key: [0x31; 16],
            ..Superblock::default()
        };
        let at = Time {
            seconds: 1,
            nanos: 0,
        };
        let mut files = Files::format(super::super::commit::DeviceStore(&disk), &sb).unwrap();
        // Fixed 1100-file geometry. The 100 trailing inodes and 1100 data
        // blocks stay unchanged; no cross-geometry ratio or timing oracle.
        for i in 0..1100 {
            let ino = files
                .create(2, alloc::format!("file-{i:04}").as_bytes(), 0o644, at)
                .unwrap();
            files.write(&disk, ino, 0, b"unchanged data", at).unwrap();
        }
        files.fsync(&disk, &mut Committer::format(sb)).unwrap();
        disk.bytes.into_inner().unwrap()
    })
}

fn nodes(image: &[u8], root: BlockPtr, out: &mut BTreeMap<u64, Vec<u8>>) {
    if out.contains_key(&root.addr) {
        return;
    }
    let at = root.addr as usize * BLOCK_SIZE;
    let bytes = image[at..at + super::super::format::NODE_SIZE].to_vec();
    let h = super::super::format::NodeHeader::decode(&bytes).unwrap();
    assert_eq!(
        (h.addr, h.generation, h.level),
        (root.addr, root.generation, root.level)
    );
    assert_eq!(
        u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        root.csum
    );
    if h.level != 0 {
        for i in 0..usize::from(h.nitems) {
            let at = 48 + i * 37;
            let child = InternalEntry::decode(&bytes[at..at + 37]).unwrap();
            nodes(
                image,
                BlockPtr {
                    addr: child.child_addr,
                    generation: child.child_generation,
                    csum: child.child_csum,
                    level: h.level - 1,
                },
                out,
            );
        }
    }
    out.insert(root.addr, bytes);
}
fn tree_nodes(image: &[u8], sb: &Superblock) -> BTreeMap<u64, Vec<u8>> {
    let disk = Disk {
        bytes: HostMutex::new(image.to_vec()),
        io: HostMutex::new(Io::default()),
    };
    let mut out = BTreeMap::new();
    nodes(image, sb.space_root, &mut out);
    for (key, bytes) in
        super::super::recover::space_items(&super::super::commit::DeviceStore(&disk), sb).unwrap()
    {
        if key.item_type == format::ROOT {
            nodes(image, format::Root::decode(&bytes).unwrap().root, &mut out);
        }
    }
    out
}

fn records(nodes: &BTreeMap<u64, Vec<u8>>, tree: u8) -> BTreeMap<Key, Vec<u8>> {
    let mut out = BTreeMap::new();
    for bytes in nodes.values().filter(|b| b[12] == tree && b[13] == 0) {
        let count = u16::from_le_bytes(bytes[14..16].try_into().unwrap());
        for i in 0..usize::from(count) {
            let at = 48 + i * 21;
            let item = LeafItemHeader::decode(&bytes[at..at + 21]).unwrap();
            let start = usize::from(item.data_off);
            assert!(out
                .insert(
                    item.key,
                    bytes[start..start + usize::from(item.data_len)].to_vec()
                )
                .is_none());
        }
    }
    out
}

fn changed_records(old: &BTreeMap<Key, Vec<u8>>, new: &BTreeMap<Key, Vec<u8>>) -> BTreeSet<Key> {
    old.keys()
        .chain(new.keys())
        .filter(|key| old.get(key) != new.get(key))
        .copied()
        .collect()
}

fn publication(operations: usize) {
    let start = Instant::now();
    let disk = Arc::new(Disk {
        bytes: HostMutex::new(image().clone()),
        io: HostMutex::new(Io::default()),
    });
    let fs = BreenFs::mount_writable(
        disk.clone(),
        Arc::new(BufferCache::new(outstanding_grants::NoWait)),
        241515583 + operations,
        1,
    )
    .unwrap();
    let before = fs.inner.state.lock().durable.clone();
    let old = tree_nodes(&disk.bytes.lock().unwrap(), &before);
    measurement::take_work();
    // Inode numbers are assigned monotonically by the fixed formatter above.
    for i in 0..operations {
        fs.setattr(
            16 + i as u64,
            SetAttr {
                mode: Some(0o600),
                ..SetAttr::default()
            },
        )
        .unwrap();
    }
    assert_eq!(
        measurement::take_work(),
        [0; 3],
        "generation must not publish during setup mutations"
    );
    *disk.io.lock().unwrap() = Io::default();
    measure::start();
    super::super::btree::measurement::take_work();
    fs.fsync(&fs.open(2).unwrap(), false).unwrap();
    let counted = measure::take();
    let work = super::super::btree::measurement::take_work();
    let io = core::mem::take(&mut *disk.io.lock().unwrap());
    let after = fs.inner.state.lock().durable.clone();
    let image = disk.bytes.lock().unwrap().clone();
    let new = tree_nodes(&image, &after);
    let changed_fs = changed_records(&records(&old, 1), &records(&new, 1));
    let expected_inodes: BTreeSet<_> = (16..16 + operations as u64)
        .map(|object| Key {
            object,
            item_type: format::INODE,
            offset: 0,
        })
        .collect();
    assert_eq!(
        changed_fs, expected_inodes,
        "publication must contain the requested dirty operations"
    );
    let changed_space = changed_records(&records(&old, 2), &records(&new, 2)).len();
    let staged: BTreeMap<_, _> = counted.staged.into_iter().collect();
    let fs_nodes = staged.values().filter(|&&tree| tree == 1).count();
    let space_nodes = staged.values().filter(|&&tree| tree == 2).count();
    // Packed leaf records include unchanged neighbours in a changed node.
    // That is distinct from copying an unchanged node or rewriting file data.
    let written_space_records: usize = staged
        .iter()
        .filter(|&(_, &tree)| tree == 2)
        .map(|(&addr, _)| &image[addr as usize * BLOCK_SIZE..][..super::super::format::NODE_SIZE])
        .filter(|bytes| bytes[13] == 0)
        .map(|bytes| usize::from(u16::from_le_bytes(bytes[14..16].try_into().unwrap())))
        .sum();
    let sectors: BTreeSet<_> = io.writes.iter().copied().collect();
    let expected: BTreeSet<_> = staged
        .keys()
        .flat_map(|addr| addr * 8..(addr + 4) * 8)
        .chain((16 + u64::from(after.slot)) * 8..(17 + u64::from(after.slot)) * 8)
        .chain(
            (after.total_blocks - 4 + u64::from(after.slot)) * 8
                ..(after.total_blocks - 3 + u64::from(after.slot)) * 8,
        )
        .collect();
    // Exact slope four 4-KiB blocks per dirty node, plus two superblocks.
    // The expected set comes from the transaction before header filling,
    // independently of the recording device. Data is unchanged in this replay.
    assert_eq!(sectors, expected);
    assert_eq!(
        io.writes.len(),
        expected.len(),
        "no sector may be written twice"
    );
    assert_eq!(io.writes.len() / 8, 4 * (fs_nodes + space_nodes) + 2);
    assert_eq!(io.flushes, [4 * staged.len() * 8, expected.len()]);
    assert_eq!(after.generation, before.generation + 1);
    assert_eq!(measurement::take_work(), [0, 0, 1]);
    assert_eq!(counted.files_views, 1);
    let space_records = super::super::recover::space_items(
        &super::super::commit::DeviceStore(disk.as_ref()),
        &after,
    )
    .unwrap();
    // The next view refreshes the sole changed ROOT, rather than materializing
    // SPACE. Independently compare its full FS contents with the written tree.
    assert_eq!(counted.view_records, 1);
    let visible: BTreeMap<_, _> = fs.inner.state.lock().files.items().unwrap().into_iter().collect();
    assert_eq!(visible, records(&new, 1));
    let mut unchanged = 0;
    let mut copied_unchanged = Vec::new();
    for (&addr, bytes) in &new {
        if let Some(previous) = old.get(&addr) {
            assert_eq!(bytes, previous, "old node overwritten");
            assert!(!staged.contains_key(&addr));
            unchanged += 1;
        } else if old.values().any(|previous| {
            // CRC, address and generation are identity, not node contents.
            previous[12..32] == bytes[12..32] && previous[48..] == bytes[48..]
        }) {
            copied_unchanged.push(addr);
        }
    }
    println!("PUBLICATION operations={operations} fs_nodes={fs_nodes} space_nodes={space_nodes} blocks={} data_blocks=0 superblocks=2 flushes={} files_views={} view_records={} space_records={} written_space_records={written_space_records} changed_space_records={changed_space} unchanged_nodes={unchanged} copied_unchanged={copied_unchanged:?} reads_sectors={} decodes={} decoded_items={} seconds={:.3}",
        io.writes.len()/8, io.flushes.len(), counted.files_views, counted.view_records, space_records.len(), io.read_sectors, work[0], work[1], start.elapsed().as_secs_f64());
    assert!(unchanged > 0, "must exercise untouched subtrees");
    assert!(
        copied_unchanged.is_empty(),
        "unchanged node contents must keep their addresses"
    );
}

#[test]
fn publication_cost_10() {
    publication(10);
}
#[test]
fn publication_cost_100() {
    publication(100);
}
#[test]
fn publication_cost_1000() {
    publication(1000);
}
