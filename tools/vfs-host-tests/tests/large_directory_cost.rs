//! E24 ledger 485(k): libc's 300-name directory through the production VFS.
use crate::{
    attrs::InodeAttrs,
    block::{cache::BufferCache, BlockDevice, BlockError},
    breenfs::{btree::measurement, vfs::BreenFs},
    model::image,
    read_conformance::{Disk, NoWait},
    vfs::{file::OpenFile, *},
};
use std::{sync::{atomic::{AtomicU64, Ordering}, Arc, Mutex}, time::Instant};

struct CountDisk { disk: Disk, calls: AtomicU64 }
impl BlockDevice for CountDisk {
    fn block_size(&self) -> usize { 512 }
    fn num_blocks(&self) -> u64 { self.disk.bytes.lock().unwrap().len() as u64 / 512 }
    fn read_block(&self, start: u64, out: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(start, 1, out)
    }
    fn read_blocks(&self, start: u64, count: usize, out: &mut [u8]) -> Result<(), BlockError> {
        assert_eq!(out.len(), count * 512);
        let bytes = self.disk.bytes.lock().unwrap();
        let start = start as usize * 512;
        out.copy_from_slice(bytes.get(start..start + out.len()).ok_or(BlockError::OutOfBounds)?);
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn write_block(&self, start: u64, input: &[u8]) -> Result<(), BlockError> {
        self.write_blocks(start, 1, input)
    }
    fn write_blocks(&self, start: u64, count: usize, input: &[u8]) -> Result<(), BlockError> {
        assert_eq!(input.len(), count * 512);
        let start = start as usize * 512;
        self.disk.bytes.lock().unwrap().get_mut(start..start + input.len())
            .ok_or(BlockError::OutOfBounds)?.copy_from_slice(input);
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn walk(fs: Arc<BreenFs<NoWait>>, disk: &CountDisk, dir: u64, names: &[Vec<u8>], buffer: usize) -> bool {
    let open = OpenFile { node: fs.open(dir).unwrap(), fs };
    measurement::take_decodes();
    measurement::take_geometry();
    disk.calls.store(0, Ordering::Relaxed);
    let mut cookie = 0;
    let mut calls = 0;
    let mut actual = Vec::new();
    loop {
        calls += 1;
        let mut used = 0;
        let next = open.dirents(cookie, &mut |entry, next| {
            let bytes = (19 + entry.name.len() + 1 + 7) & !7;
            if used + bytes > buffer { return Ok(false); }
            assert!(next > cookie);
            used += bytes;
            cookie = next;
            actual.push(entry.name);
            Ok(true)
        }).unwrap();
        assert_eq!(next, cookie);
        if used == 0 { break; }
    }
    assert_eq!(&actual[..2], &[b".".to_vec(), b"..".to_vec()]);
    assert_eq!(&actual[2..], names);
    let decodes = measurement::take_decodes();
    let (unique, height) = measurement::take_geometry();
    let io = disk.calls.swap(0, Ordering::Relaxed);
    // One visit per intersecting node, plus at most two root descents per
    // getdents call (directory validation and resume). This is linear in nodes
    // and calls, independent of how many entries fit a leaf or an internal batch.
    let bound = unique as u64 + 2 * u64::from(height) * calls;
    println!("READDIR_COST names={} buffer={buffer} calls={calls} nodes={unique} height={height} decodes={decodes} device_calls={io} bound={bound}", names.len());
    assert!(io <= bound, "device calls {io} exceed linear node/call bound {bound}");
    decodes <= bound
}

#[test]
fn large_directory_readdir_has_linear_node_and_device_cost() {
    let start = Instant::now();
    let disk = Arc::new(CountDisk { disk: Disk { bytes: Mutex::new(image()), writable: true }, calls: AtomicU64::new(0) });
    let cache = Arc::new(BufferCache::new(NoWait));
    let fs = BreenFs::mount_writable(disk.clone(), cache.clone(), 2415, 1).unwrap();
    let attrs = InodeAttrs::new(0o755, 0, 0);
    let dir = fs.create(2, b"many", NewNode::Directory { attrs }).unwrap();
    let mut names = Vec::new();
    let mut linear = true;
    for n in 0..300 {
        let name = format!("f{n:03}xxxxxxxxx").into_bytes();
        fs.create(dir, &name, NewNode::Regular { attrs }).unwrap();
        names.push(name);
        // Model periodic publication during setup; do not exhaust outstanding
        // metadata grants in the sequential host (which has no worker thread).
        if names.len() % 32 == 0 { fs.sync().unwrap(); }
        if [75, 150, 300].contains(&names.len()) {
            // Full traversal isolates repeated internal batching. 8 KiB is
            // libc dirent.rs's actual buffer; 128 B exercises #3045's resumes.
            for buffer in [usize::MAX, 8192, 128] {
                linear &= walk(fs.clone(), &disk, dir, &names, buffer);
            }
        }
    }
    fs.sync().unwrap();
    drop(fs);
    let fs = BreenFs::mount(disk.clone(), Arc::new(BufferCache::new(NoWait)), 2416, 1).unwrap();
    for buffer in [usize::MAX, 8192, 128] {
        linear &= walk(fs.clone(), &disk, dir, &names, buffer);
    }
    assert!(start.elapsed().as_secs_f64() < 60.0, "U32: large directory count test exceeded 60 s");
    assert!(linear, "readdir repeatedly decodes already visited nodes instead of a linear traversal");
}
