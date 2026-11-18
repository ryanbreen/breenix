//! E24 coordinator ledger 574(x5): production publication work at populated roots.
use super::*;
use super::super::{btree::{self, NodeStore}, format::{self, Key, Root}};
use std::{collections::{BTreeMap, BTreeSet}, sync::{Mutex as HostMutex, OnceLock}, time::Instant};

// A chmod changes one INODE and its COW path. Each copied node queues an
// allocation and release; SPACE COW adds its own allocation/release fixpoint.
// Sixteen returned records and 64 decodes per edit/level cover these paths,
// ROOT refresh, FREE/PENDING neighbours and orphan-range endpoints. The latter
// includes both prepare_commit calls. Each decode visits at most MAX_ITEMS
// slots (the format's 48-byte header and 21-byte minimum slot width).
const RECORDS_PER_EDIT_LEVEL: u64 = 16;
const DECODES_PER_EDIT_LEVEL: u64 = 64;
const MAX_ITEMS: u64 = (format::NODE_SIZE as u64 - 48) / 21;
const ITEM_VISITS_PER_EDIT_LEVEL: u64 = DECODES_PER_EDIT_LEVEL * MAX_ITEMS;
const FIXED_ROOTS: u64 = 16;

struct Disk(HostMutex<Vec<u8>>);
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { self.0.lock().unwrap().len() as u64 / BLOCK_SIZE as u64 }
    fn read_block(&self, n: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let at = n as usize * BLOCK_SIZE;
        bytes.copy_from_slice(&self.0.lock().unwrap()[at..at + bytes.len()]); Ok(())
    }
    fn write_block(&self, n: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let at = n as usize * BLOCK_SIZE;
        self.0.lock().unwrap()[at..at + bytes.len()].copy_from_slice(bytes); Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}

// Build immutable format-1 fixtures directly with the production codecs/B-tree,
// avoiding thousands of setup transactions. Only the measured generation uses
// VFS admission and the production commit path. The loop gives each inode one data block.
struct Seed {
    nodes: BTreeMap<u64, Vec<u8>>,
    next: u64,
}
impl NodeStore for Seed {
    fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
        self.nodes.get(&addr).cloned().ok_or(btree::Error::Store)
    }
    fn put(&mut self, addr: u64, bytes: Vec<u8>) -> Result<(), btree::Error> {
        self.nodes.insert(addr, bytes); Ok(())
    }
    fn alloc(&mut self) -> Result<u64, btree::Error> {
        let addr = self.next; self.next += 4; Ok(addr)
    }
}
fn key(object: u64, item_type: u8, offset: u64) -> Key { Key { object, item_type, offset } }
fn image(files: usize) -> &'static Vec<u8> {
    static SMALL: OnceLock<Vec<u8>> = OnceLock::new();
    static LARGE: OnceLock<Vec<u8>> = OnceLock::new();
    (if files == 1100 { &SMALL } else { &LARGE }).get_or_init(|| {
        let mut seed = Seed { nodes: BTreeMap::new(), next: 20 + files as u64 };
        let uuid = [0x75; 16]; let hash = [0x31; 16];
        let mut fs = btree::create(&mut seed, 1, uuid, 1).unwrap();
        let root_inode = format::Inode { mode: 0o40755, nlink: 2, parent: 2,
            size: files as u64, next_index: files as u64 + 2, created_generation: 1,
            ..format::Inode::default() };
        btree::insert(&mut seed, &mut fs, key(2, format::INODE, 0), &root_inode.encode()).unwrap();
        for i in 0..files {
            let ino = 16 + i as u64; let name = alloc::format!("file-{i:04}").into_bytes();
            let entry = format::DirRecord { ino, kind: 1, name: name.clone() };
            btree::insert(&mut seed, &mut fs, key(2, format::DIR_ENTRY,
                super::super::siphash::hash(&hash, &name)), &entry.encode()).unwrap();
            btree::insert(&mut seed, &mut fs, key(2, format::DIR_INDEX, i as u64 + 2), &entry.encode()).unwrap();
            let inode = format::Inode { mode: 0o100644, nlink: 1, size: 1, nblocks: 1,
                created_generation: 1, ..format::Inode::default() };
            btree::insert(&mut seed, &mut fs, key(ino, format::INODE, 0), &inode.encode()).unwrap();
            btree::insert(&mut seed, &mut fs, key(ino, format::EXTENT, 0),
                &format::Extent { phys_start: 20 + i as u64, phys_len: 1,
                    extent_offset: 0, num_blocks: 1, flags: 0 }.encode()).unwrap();
        }
        let mut space = btree::create(&mut seed, 2, uuid, 1).unwrap();
        let root = Root { root: fs, next_inode: files as u64 + 16, source_root: 0,
            created_generation: 1, ctime: Time { seconds: 0, nanos: 0 }, flags: 0,
            name: b"default".to_vec() };
        btree::insert(&mut seed, &mut space, key(0xffff_ffff_ffff_ff10, format::ROOT, 1), &root.encode()).unwrap();
        let crc = super::super::crc32c::checksum(&vec![0; BLOCK_SIZE]);
        for i in 0..files {
            btree::insert(&mut seed, &mut space, key(20 + i as u64, format::REF, 1),
                &format::Ref { refcount: 1, kind: 3, alloc_generation: 1, crcs: vec![crc] }.encode()).unwrap();
        }
        let mut accounted = BTreeSet::new();
        let mut free_start = seed.next;
        loop {
            let missing: Vec<_> = seed.nodes.iter().filter(|(a, _)| !accounted.contains(*a))
                .map(|(&a, b)| (a, b[12])).collect();
            for (addr, tree) in missing {
                btree::insert(&mut seed, &mut space, key(addr, format::REF, 4),
                    &format::Ref { refcount: 1, kind: tree, alloc_generation: 1, crcs: vec![] }.encode()).unwrap();
                accounted.insert(addr);
            }
            if free_start != seed.next {
                btree::delete(&mut seed, &mut space, key(0xffff_ffff_ffff_ff20, format::FREE, free_start)).unwrap();
                free_start = seed.next;
            }
            btree::insert(&mut seed, &mut space, key(0xffff_ffff_ffff_ff20, format::FREE, free_start),
                &format::Free { length: 16384 - 4 - free_start }.encode()).unwrap();
            if accounted.len() == seed.nodes.len() && free_start == seed.next { break; }
        }
        let sb = Superblock { fs_uuid: uuid, generation: 1, slot: 1, total_blocks: 16384,
            global_reserve: 4096, next_root_id: 2, dirhash_key: hash, space_root: space,
            free_blocks: 16384 - 4 - seed.next, ..Superblock::default() };
        let mut bytes = vec![0; 16384 * BLOCK_SIZE];
        for (addr, node) in seed.nodes { let at = addr as usize * BLOCK_SIZE;
            bytes[at..at + node.len()].copy_from_slice(&node); }
        for ring in 0..2 { let mut copy = sb.clone(); copy.ring = ring;
            let at = super::super::commit::slot_block(sb.total_blocks, ring, 1) as usize * BLOCK_SIZE;
            bytes[at..at + BLOCK_SIZE].copy_from_slice(&copy.encode()); }
        bytes
    })
}

fn publication(files: usize) {
    let started = Instant::now();
    let baseline = image(files);
    let mut failures = Vec::new();
    for dirty in [10u64, 100] {
        let disk = Arc::new(Disk(HostMutex::new(baseline.clone())));
        let fs = BreenFs::mount_writable(disk, Arc::new(BufferCache::new(outstanding_grants::NoWait)),
            241515574 + files + dirty as usize, 1).unwrap();
        let before = fs.inner.state.lock().durable.generation;
        for i in 0..dirty { fs.setattr(16 + i, SetAttr { mode: Some(0o600), ..SetAttr::default() }).unwrap(); }
        let heights = { let s = fs.inner.state.lock(); let paths = s.files.reservation_paths(16).unwrap();
            [paths.fs_height, paths.space_height] };
        btree::measurement::take_publication();
        fs.fsync(&fs.open(2).unwrap(), false).unwrap();
        let counts = btree::measurement::take_publication();
        assert_eq!(fs.inner.state.lock().durable.generation, before + 1);
        for (tree, (height, counts)) in heights.into_iter().zip(counts).enumerate() {
            let bounds = [RECORDS_PER_EDIT_LEVEL, DECODES_PER_EDIT_LEVEL, ITEM_VISITS_PER_EDIT_LEVEL]
                .map(|k| dirty * (height + 1) * k + FIXED_ROOTS);
            println!("INCREMENTAL files={files} dirty={dirty} tree={tree} height={height} records={} decodes={} visits={} bounds={bounds:?}", counts[0], counts[1], counts[2]);
            for (column, (count, bound)) in counts.into_iter().zip(bounds).enumerate() {
                if count > bound { failures.push((dirty, tree, column, count, bound)); }
            }
        }
    }
    println!("INCREMENTAL_SECONDS files={files} seconds={:.3}", started.elapsed().as_secs_f64());
    assert!(failures.is_empty(), "574(x5) publication bounds: {failures:?}");
}
#[test]
fn populated_1100_incremental_publication() { publication(1100); }
#[test]
fn populated_4400_incremental_publication() { publication(4400); }
