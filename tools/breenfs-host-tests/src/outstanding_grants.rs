//! E24 coordinator ledger 558(x1): exact settled debt at populated-root geometry.
use super::*;
use std::{sync::Mutex as HostMutex, time::Instant};

struct NoWait;
impl Wait for NoWait {
    fn generation(&self) -> u64 { 0 }
    fn wait(&self, _: u64) { panic!("unexpected sequential wait") }
    fn wake_all(&self) {}
}
struct Disk(HostMutex<Vec<u8>>);
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { 512 }
    fn num_blocks(&self) -> u64 { self.0.lock().unwrap().len() as u64 / 512 }
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        let start = block as usize * 512;
        bytes.copy_from_slice(&self.0.lock().unwrap()[start..start + bytes.len()]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        let start = block as usize * 512;
        self.0.lock().unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
fn fixture() -> Arc<BreenFs<NoWait>> {
    // Keep populated FS and SPACE trees, rather than only reducing free space
    // on an empty root. This mirrors metadata_admission's production writer.
    let disk = Arc::new(Disk(HostMutex::new(vec![0; 80000 * BLOCK_SIZE])));
    let sb = Superblock { fs_uuid: [0x58;16], generation: 1, slot: 1,
        total_blocks: 80000, global_reserve: 4096, next_root_id: 2,
        dirhash_key: [0x31;16], ..Superblock::default() };
    let at = Time { seconds: 1, nanos: 0 };
    {
        let mut files = Files::format(super::super::commit::DeviceStore(disk.as_ref()), &sb).unwrap();
        files.mkdir(2, b"tmp", 0o1777, at).unwrap();
        let bytes = vec![0x61; 64 * BLOCK_SIZE];
        for i in 0..994 {
            let ino = files.create(2, alloc::format!("populated-{i}").as_bytes(), 0o644, at).unwrap();
            files.write(disk.as_ref(), ino, 0, &bytes, at).unwrap();
        }
        files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    }
    BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 241515581, 1).unwrap()
}

#[derive(Default)]
struct Owed {
    generation: u64,
    metadata: u64,
    // A write has returned, but DirtyPages::write_runs still owes its data,
    // EXTENT edits and SPACE changes. Its admission remains in flight until
    // that generation drains. Subtract metadata work already charged here.
    dirty: u64,
    rows: usize,
    mismatches: usize,
    peak: u64,
}
impl Owed {
    fn observe(&mut self, fs: &BreenFs<NoWait>, label: &str, data: bool) {
        let work = super::super::btree::measurement::take_mutations();
        let s = fs.inner.state.lock();
        assert!(s.pending.is_none()); // Sequential foreground replay, no worker.
        let generation = s.durable.generation;
        if self.generation != generation {
            self.generation = generation;
            self.metadata = 0;
            self.dirty = 0;
        }
        // Read allocation/deferred/orphan debt through Files::metadata_debt,
        // independently of metadata_reserved and the retained-grant list.
        let metadata = s.files.metadata_debt().unwrap();
        if data {
            let spent = metadata.checked_sub(self.metadata).unwrap();
            self.dirty += work[1].checked_sub(spent).unwrap();
        }
        self.metadata = metadata;
        let expected = metadata + self.dirty;
        let outstanding = s.reservations.outstanding();
        let exact = outstanding == expected;
        self.rows += 1;
        self.mismatches += usize::from(!exact);
        self.peak = self.peak.max(outstanding);
        println!("GRANTS {label} generation={generation} free={} reserve={} metadata={metadata} dirty_inflight={} expected={expected} outstanding={outstanding} granted={} retained={} returned={} exact={exact}",
            s.durable.free_blocks, s.durable.global_reserve, self.dirty, work[1], work[2], work[3]);
    }
}

#[test]
fn populated_create_write_unlink_grants_equal_remaining_debt() {
    let start = Instant::now();
    let fs = fixture();
    let tmp = fs.lookup(2, b"tmp").unwrap();
    {
        let s = fs.inner.state.lock();
        let paths = s.files.reservation_paths(tmp).unwrap();
        println!("GEOMETRY total={} free={} reserve={} fs_height={} space_height={} space_nodes={}", s.durable.total_blocks,
            s.durable.free_blocks, s.durable.global_reserve, paths.fs_height, paths.space_height, paths.space_nodes);
        assert!((16000..=16400).contains(&s.durable.free_blocks));
        assert_eq!(s.durable.global_reserve, 4096);
    }
    super::super::btree::measurement::take_mutations();
    let mut owed = Owed::default();
    let mut inodes = Vec::new();
    for i in 0..300 {
        let name = alloc::format!("f{i:03}xxxxxxxxx");
        let ino = fs.create(tmp, name.as_bytes(), NewNode::Regular { attrs: InodeAttrs::new(0o644,0,0) }).unwrap();
        owed.observe(&fs, &alloc::format!("create/{i}"), false);
        let node = fs.open(ino).unwrap();
        let result = fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false }, b"x").unwrap();
        assert_eq!(result.bytes, 1);
        drop(node);
        owed.observe(&fs, &alloc::format!("write/{i}"), true);
        inodes.push((name, ino));
    }
    let parent = fs.getattr(tmp).unwrap().attrs;
    for (i, (name, ino)) in inodes.into_iter().enumerate() {
        fs.remove(tmp, name.as_bytes(), Remove { directory: false, expected: ino, owner: 0, parent }).unwrap();
        owed.observe(&fs, &alloc::format!("unlink/{i}"), false);
    }
    // A grant held between admission and completion belongs in the equality.
    // Also plant a settled excess block: the oracle must reject that leak.
    {
        let mut s = fs.inner.state.lock();
        let expected = owed.metadata + owed.dirty;
        let free = s.durable.free_blocks;
        let flight = s.reservations.reserve(free, 1, Operation::Unlink).unwrap();
        assert_eq!(s.reservations.outstanding(), expected + flight.blocks());
        s.reservations.release(flight);
        let excess = s.reservations.reserve(free, 1, Operation::Unlink).unwrap();
        s.grants.push(excess);
        assert_ne!(s.reservations.outstanding(), expected, "planted settled excess must fail the equality");
        let excess = s.grants.pop().unwrap();
        s.reservations.release(excess);
        assert_eq!(s.reservations.outstanding(), expected);
    }
    fs.fsync(&fs.open(tmp).unwrap(), false).unwrap();
    assert_eq!(fs.inner.state.lock().reservations.outstanding(), 0);
    let control = fs.create(tmp, b"explicit-fsync-control", NewNode::Regular { attrs: InodeAttrs::new(0o644,0,0) });
    assert!(control.is_ok());
    println!("GRANTS_SUMMARY rows={} mismatches={} peak={} seconds={:.3}", owed.rows, owed.mismatches, owed.peak, start.elapsed().as_secs_f64());
    assert_eq!(owed.rows, 900);
    assert_eq!(owed.mismatches, 0, "settled namespace capacity must equal remaining debt plus in-flight data grants");
}
