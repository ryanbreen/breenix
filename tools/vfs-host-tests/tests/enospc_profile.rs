//! Deterministic phase counts for the guest's 4 KiB fill (E24 ledger 392).
use crate::{attrs::InodeAttrs, block::{BlockDevice, BlockError, cache::BufferCache},
    breenfs::{btree, dirty, vfs::{BreenFs, measurement}}, model::image,
    read_conformance::NoWait, syscall_errno::ENOSPC, vfs::*};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Io { reads: u64, writes: u64, flushes: u64, generations: u64, durable: u64, pending: u64 }
struct Disk { bytes: Mutex<Vec<u8>>, io: Mutex<Io> }
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { 512 }
    fn num_blocks(&self) -> u64 { self.bytes.lock().unwrap().len() as u64 / 512 }
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(block, 1, bytes)
    }
    fn read_blocks(&self, block: u64, count: usize, out: &mut [u8]) -> Result<(), BlockError> {
        let bytes = self.bytes.lock().unwrap();
        let start = block as usize * 512;
        out[..count * 512].copy_from_slice(bytes.get(start..start + count * 512).ok_or(BlockError::OutOfBounds)?);
        self.io.lock().unwrap().reads += 1;
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.write_blocks(block, 1, bytes)
    }
    fn write_blocks(&self, block: u64, count: usize, input: &[u8]) -> Result<(), BlockError> {
        let mut bytes = self.bytes.lock().unwrap();
        let start = block as usize * 512;
        let total = bytes.len() as u64 / 512;
        bytes.get_mut(start..start + count * 512).ok_or(BlockError::OutOfBounds)?
            .copy_from_slice(&input[..count * 512]);
        let mut io = self.io.lock().unwrap();
        io.writes += 1;
        for (index, sector) in input[..count * 512].chunks_exact(512).enumerate() {
            let lba = block + index as u64;
            if lba % 8 == 0 && ((128..160).contains(&lba) || lba >= total - 32)
                && &sector[..8] == b"BREENFSS"
            {
                io.pending = io.pending.max(u64::from_le_bytes(sector[32..40].try_into().unwrap()));
            }
        }
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> {
        let mut io = self.io.lock().unwrap();
        io.flushes += 1;
        if io.pending > io.durable { io.generations += 1; io.durable = io.pending; }
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
struct Counts([u64; 14]);
impl Counts {
    fn take(disk: &Disk) -> Self {
        let tree = btree::measurement::take_work();
        let pages = dirty::measurement::take_work();
        let commits = measurement::take_work();
        let mut io = disk.io.lock().unwrap();
        let n = Self([tree[0], tree[1], tree[2], tree[3], pages[0], pages[1], pages[2],
            commits[0], commits[1], commits[2], io.reads, io.writes, io.flushes, io.generations]);
        io.reads = 0; io.writes = 0; io.flushes = 0; io.generations = 0;
        n
    }
    fn add(&mut self, other: Self) {
        for (a, b) in self.0.iter_mut().zip(other.0) { *a += b; }
    }
    fn emit(self, phase: &str, bytes: u64) {
        println!("E24_PROFILE,{phase},{bytes},{}", self.0.map(|n| n.to_string()).join(","));
    }
}
fn mount(disk: &Arc<Disk>) -> Arc<BreenFs<NoWait>> {
    BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 71, 1).unwrap()
}
fn create(fs: &dyn FileSystem, dir: u64, name: &[u8], directory: bool) -> u64 {
    let attrs = InodeAttrs::new(if directory {0o755} else {0o644}, 0, 0);
    fs.create(dir, name, if directory {NewNode::Directory {attrs}} else {NewNode::Regular {attrs}}).unwrap()
}
fn write(fs: &dyn FileSystem, node: &VNode, offset: u64, bytes: &[u8]) -> Result<WriteResult, WriteError> {
    fs.write(node, WriteReq {at: WriteAt::Offset(offset), limit: u64::MAX, clear_setid: false}, bytes)
}
fn remove(fs: &dyn FileSystem, dir: u64, name: &[u8], directory: bool) {
    let ino = fs.lookup(dir, name).unwrap();
    fs.remove(dir, name, Remove {directory, expected: ino, owner: fs.getattr(ino).unwrap().attrs.uid,
        parent: fs.getattr(dir).unwrap().attrs}).unwrap();
}

#[test]
fn guest_delete_lifecycle_phase_counts() {
    println!("E24_PROFILE,phase,bytes,node_decodes,decoded_items,fs_reservation_visits,space_reservation_visits,dirty_projection_visits,dirty_index_visits,drained_pages,reservation_pressure_commits,dirty_pressure_commits,vfs_publications,device_reads,device_writes,flushes,device_generations");
    let disk = Arc::new(Disk {bytes: Mutex::new(image()), io: Mutex::new(Io {durable: 1, ..Io::default()})});
    Counts::take(&disk);
    // The preceding fill case remounts its durable file, then deletes its tree.
    let fs = mount(&disk);
    let dir = create(fs.as_ref(), 2, b"prior", true);
    let node = fs.open(create(fs.as_ref(), dir, b"fill", false)).unwrap();
    let megabyte = vec![0x5a; 1024 * 1024];
    let mut offset = 0;
    loop {
        match write(fs.as_ref(), &node, offset, &megabyte) {
            Ok(w) => { assert_eq!(w.bytes, megabyte.len()); offset += w.bytes as u64; }
            Err(e) => { assert_eq!(e.errno().0, ENOSPC); break; }
        }
        measurement::service(&fs).unwrap();
        assert!(offset < 64 * 1024 * 1024);
    }
    fs.fsync(&node, false).unwrap(); drop(node);
    remove(fs.as_ref(), dir, b"fill", false); remove(fs.as_ref(), 2, b"prior", true);
    fs.shutdown("profile-prior", u64::MAX).unwrap(); drop(fs);
    Counts::take(&disk).emit("precondition", offset);

    let fs = mount(&disk);
    assert_eq!(fs.statfs().unwrap().blocks, 16384);
    let dir = create(fs.as_ref(), 2, b"write-24", true);
    let unlink = fs.open(create(fs.as_ref(), dir, b"unlink", false)).unwrap();
    write(fs.as_ref(), &unlink, 0, b"unlink").unwrap(); drop(unlink);
    create(fs.as_ref(), dir, b"empty", true);
    let truncate = fs.open(create(fs.as_ref(), dir, b"truncate", false)).unwrap();
    write(fs.as_ref(), &truncate, 0, b"truncate").unwrap();
    let fill = fs.open(create(fs.as_ref(), dir, b"fill", false)).unwrap();
    Counts::take(&disk).emit("setup", 0);
    let page = [0x5a; 4096];
    let mut offset = 0;
    let mut counts = Counts::default();
    loop {
        match write(fs.as_ref(), &fill, offset, &page) {
            Ok(w) => { assert_eq!(w.bytes, page.len()); offset += w.bytes as u64; }
            Err(e) => { assert_eq!(e.errno().0, ENOSPC); break; }
        }
        // Deterministically service the real pressure policy after each write.
        // There is no elapsed-time trigger or synthetic fsync in this fill.
        measurement::service(&fs).unwrap();
        if offset % (1024 * 1024) == 0 {
            counts.add(Counts::take(&disk)); counts.emit("fill-prefix", offset);
        }
        assert!(offset < 64 * 1024 * 1024);
    }
    counts.add(Counts::take(&disk)); counts.emit("fill-refusal", offset);
    remove(fs.as_ref(), dir, b"unlink", false); Counts::take(&disk).emit("unlink", offset);
    remove(fs.as_ref(), dir, b"empty", true); Counts::take(&disk).emit("rmdir", offset);
    fs.resize(&truncate, 0, false, &|_| Ok(())).unwrap(); Counts::take(&disk).emit("truncate", offset);
    fs.fsync(&fill, false).unwrap(); Counts::take(&disk).emit("fsync", offset);
    drop(fill); drop(truncate);
    remove(fs.as_ref(), dir, b"fill", false); remove(fs.as_ref(), dir, b"truncate", false);
    remove(fs.as_ref(), 2, b"write-24", true); Counts::take(&disk).emit("cleanup", offset);
    fs.shutdown("profile-delete", u64::MAX).unwrap(); drop(fs);
    Counts::take(&disk).emit("unmount", offset);
    let fs = mount(&disk);
    let mut names = Vec::new();
    fs.readdir(2, 0, &mut |e, _| {names.push(e.name); Ok(true)}).unwrap();
    assert!(names.is_empty());
    Counts::take(&disk).emit("remount", offset);
}

#[test]
fn bavail_reports_ungranted_headroom_after_write_refusal() {
    let disk = Arc::new(Disk {bytes: Mutex::new(image()), io: Mutex::new(Io {durable: 1, ..Io::default()})});
    assert_eq!(disk.bytes.lock().unwrap().len(), 64 * 1024 * 1024);
    let fs = mount(&disk);
    let dir = create(fs.as_ref(), 2, b"write-24", true);
    let fill = fs.open(create(fs.as_ref(), dir, b"fill", false)).unwrap();
    assert_eq!(fs.statfs().unwrap().blocks, 16384);
    let mut bytes = 0;
    loop {
        match write(fs.as_ref(), &fill, bytes, &[0x5a; 4096]) {
            Ok(w) => { assert_eq!(w.bytes, 4096); bytes += 4096; }
            Err(e) => { assert_eq!(e.errno().0, ENOSPC); break; }
        }
        // Service the production pressure policy without a synthetic fsync.
        measurement::service(&fs).unwrap();
        assert!(bytes < 64 * 1024 * 1024);
    }
    let full = fs.statfs().unwrap();
    let [free, reserve, outstanding, rejected, available] =
        crate::breenfs::reserve::observation::take().unwrap();
    assert_eq!(reserve, 4096);
    assert!(free >= reserve + outstanding);
    assert_eq!(available, free - reserve - outstanding);
    assert!(rejected > available);
    assert!(available > 0);
    assert_eq!(outstanding, 0);
    assert_eq!(full.free_blocks, free);
    assert_eq!(full.available_blocks, available);
    println!("BAVAIL_REFUSAL bytes={bytes} free={free} reserve={reserve} outstanding={outstanding} rejected_grant={rejected} available={available}");
}
