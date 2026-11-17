//! E24 24.15f: count a testdata-populated root walk, sync and mount retirement.
use crate::{
    attrs::InodeAttrs,
    block::{cache::BufferCache, BlockDevice, BlockError},
    breenfs::{btree, commit::{Committer, DeviceStore}, format::{Superblock, Time},
        ops::Files, vfs::{measurement, BreenFs}},
    read_conformance::NoWait,
    vfs::*,
};
use std::{path::{Path, PathBuf}, sync::{Arc, Mutex}, time::Instant};

#[derive(Clone, Copy, Default, Debug)]
struct Io { reads: u64, writes: u64, flushes: u64 }
struct Disk { bytes: Mutex<Vec<u8>>, io: Mutex<Io> }
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { 512 }
    fn num_blocks(&self) -> u64 { self.bytes.lock().unwrap().len() as u64 / 512 }
    fn read_block(&self, start: u64, out: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(start, 1, out)
    }
    fn read_blocks(&self, start: u64, count: usize, out: &mut [u8]) -> Result<(), BlockError> {
        assert_eq!(out.len(), count * 512);
        let bytes = self.bytes.lock().unwrap();
        let start = start as usize * 512;
        out.copy_from_slice(bytes.get(start..start + out.len()).ok_or(BlockError::OutOfBounds)?);
        self.io.lock().unwrap().reads += 1;
        Ok(())
    }
    fn write_block(&self, start: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.write_blocks(start, 1, bytes)
    }
    fn write_blocks(&self, start: u64, count: usize, input: &[u8]) -> Result<(), BlockError> {
        assert_eq!(input.len(), count * 512);
        let start = start as usize * 512;
        self.bytes.lock().unwrap().get_mut(start..start + input.len())
            .ok_or(BlockError::OutOfBounds)?.copy_from_slice(input);
        self.io.lock().unwrap().writes += 1;
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> { self.io.lock().unwrap().flushes += 1; Ok(()) }
}
struct Source { parts: Vec<Vec<u8>>, bytes: Option<Vec<u8>> }
fn sources(root: &Path, relative: &Path, out: &mut Vec<Source>) {
    use std::os::unix::ffi::OsStrExt;
    let path = root.join(relative);
    let directory = path.is_dir();
    out.push(Source { parts: relative.components().map(|p| p.as_os_str().as_bytes().to_vec()).collect(),
        bytes: (!directory).then(|| std::fs::read(&path).unwrap()) });
    if directory {
        let mut children = std::fs::read_dir(path).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>();
        children.sort();
        for name in children { sources(root, &relative.join(name), out); }
    }
}
fn resolve(fs: &dyn FileSystem, path: &[Vec<u8>]) -> Arc<VNode> {
    let mut node = fs.open(fs.root().unwrap()).unwrap();
    for name in path {
        assert_eq!(fs.getattr(node.ino).unwrap().kind, NodeKind::Directory);
        node = fs.lookup_open(&node, name).unwrap();
    }
    node
}
fn report(phase: &str, disk: &Disk, cache: &BufferCache<NoWait>, base: isize, fs: Option<&BreenFs<NoWait>>) -> (u64, Io, usize) {
    let decodes = btree::measurement::take_decodes();
    let io = std::mem::take(&mut *disk.io.lock().unwrap());
    // Measure net retained mount allocations, including decoded nodes and dirty
    // maps, rather than infer their memory from encoded node sizes. The host
    // allocator is thread-local; source/image/output storage precedes `base`.
    let retained = (measurement::heap_live() - base).max(0) as usize;
    let dirty = fs.map_or(0, BreenFs::dirty_bytes);
    println!("ROOT_COST {phase} decodes={decodes} reads={} writes={} flushes={} retained_pages={} cache_pages={} dirty_pages={}",
        io.reads, io.writes, io.flushes, retained.div_ceil(4096), cache.resident_bytes().div_ceil(4096), dirty.div_ceil(4096));
    (decodes, io, retained)
}

#[test]
fn testdata_root_cost_is_linear_and_unmount_releases_pages() {
    let start = Instant::now();
    let mut source = Vec::new();
    sources(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata"), Path::new(""), &mut source);
    let disk = Arc::new(Disk { bytes: Mutex::new(vec![0; 64 * 1024 * 1024]), io: Mutex::new(Io::default()) });
    let sb = Superblock { fs_uuid: [0xf; 16], generation: 1, slot: 1, total_blocks: 16384,
        global_reserve: 4096, next_root_id: 2, dirhash_key: [0x15; 16], ..Superblock::default() };
    let at = Time { seconds: 1, nanos: 0 };
    {
        let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
        let mut directories = std::collections::BTreeMap::from([(Vec::new(), 2)]);
        for row in &source[1..] {
            let parent = directories[&row.parts[..row.parts.len()-1]];
            let name = row.parts.last().unwrap();
            if let Some(bytes) = &row.bytes {
                let ino = files.create(parent, name, 0o644, at).unwrap();
                files.write(disk.as_ref(), ino, 0, bytes, at).unwrap();
            } else {
                let ino = files.mkdir(parent, name, 0o755, at).unwrap();
                directories.insert(row.parts.clone(), ino);
            }
        }
        files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    }
    let cache = Arc::new(BufferCache::new(NoWait));
    let mut buffer = [0; 16384];
    btree::measurement::take_decodes();
    *disk.io.lock().unwrap() = Io::default();
    let base = measurement::heap_live();
    let fs = BreenFs::mount_writable(disk.clone(), cache.clone(), 24150, 1).unwrap();
    let mount = Arc::downgrade(&fs);
    report("mount", &disk, &cache, base, Some(&fs));
    for pass in 0..2 {
        let mut decodes = 0;
        let mut reads = 0;
        let mut blocks = 0;
        for (index, row) in source.iter().enumerate() {
            let node = resolve(fs.as_ref(), &row.parts);
            let attrs = fs.getattr(node.ino).unwrap();
            if let Some(bytes) = &row.bytes {
                assert_eq!(attrs.size, bytes.len() as u64);
                let file = resolve(fs.as_ref(), &row.parts);
                let mut done = 0;
                while done < bytes.len() {
                    let n = fs.read(&file, done as u64, &mut buffer).unwrap();
                    assert!(n > 0);
                    assert_eq!(&buffer[..n], &bytes[done..done+n]);
                    done += n;
                }
                blocks += bytes.len().div_ceil(4096) as u64;
            } else {
                let dir = resolve(fs.as_ref(), &row.parts);
                let mut cookie = 0;
                loop {
                    let mut names = 0;
                    let next = fs.readdir(dir.ino, cookie, &mut |entry, _| {
                        assert!(source.iter().any(|s| s.parts.len() == row.parts.len()+1
                            && s.parts.starts_with(&row.parts) && s.parts.last() == Some(&entry.name)));
                        names += 1;
                        Ok(names <= 31)
                    }).unwrap();
                    if next == cookie { break; }
                    cookie = next;
                }
            }
            drop(node);
            let (n, io, retained) = report(if pass == 0 {"walk-cold-file"} else {"walk-warm-file"}, &disk, &cache, base, Some(&fs));
            decodes += n; reads += io.reads;
            // Linear prefix bounds in paths and data blocks. A whole-tree
            // decode per file exceeds these as the populated root grows.
            let rows = index as u64 + 1;
            assert!(decodes <= 32 * rows + 64, "prefix {rows}: {decodes} decodes");
            assert!(reads <= 32 * rows + blocks + 64, "prefix {rows}: {reads} device reads");
            // Design 1.2's 4 MiB buffer budget plus a linear metadata allowance
            // (32 KiB per visited inode, including the 64-node decoded cache).
            assert!(retained <= 4 * 1024 * 1024 + 32768 * (index + 1));
            assert_eq!(fs.dirty_bytes(), 0);
        }
        println!("ROOT_COST walk pass={pass} rows={} blocks={blocks} decodes={decodes} reads={reads}", source.len());
    }
    fs.sync().unwrap();
    report("sync", &disk, &cache, base, Some(&fs));
    let node = fs.open(fs.create(2, b"mapped", NewNode::Regular { attrs: InodeAttrs::new(0o600, 0, 0) }).unwrap()).unwrap();
    fs.resize(&node, 8192, false, &|_| Ok(())).unwrap();
    node.map.shared_stores(&[0x5a; 8192], true);
    fs.fsync(&node, false).unwrap();
    assert_eq!(fs.read(&node, 0, &mut buffer).unwrap(), 8192);
    assert_eq!(&buffer[..8192], &[0x5a; 8192]);
    report("mapped-fsync", &disk, &cache, base, Some(&fs));
    node.map.shared_stores(&[], false);
    let vnode = Arc::downgrade(&node);
    drop(node);
    fs.shutdown("root-cost", u64::MAX).unwrap();
    report("shutdown", &disk, &cache, base, Some(&fs));
    assert_eq!(fs.dirty_bytes(), 0);
    drop(fs);
    assert!(mount.upgrade().is_none(), "mount identity survived unmount");
    assert!(vnode.upgrade().is_none(), "mapped VNode survived unmount");
    drop((mount, vnode));
    // VNodes pin Mount; Mount owns dirty state, Files/CachedNodeStore and
    // DataBlocks. Those owners retain the device and/or shared cache (vfs.rs,
    // node_cache.rs), so neither graph may retain an extra strong reference.
    assert_eq!(Arc::strong_count(&disk), 1, "mount-private device owner survived");
    assert_eq!(Arc::strong_count(&cache), 1, "mount-private cache owner survived");
    let (_, _, retained) = report("unmount", &disk, &cache, base, None);
    // Design 1.2 and E24 ledger 467/470: clean raw-device buffers may remain
    // within the shared cache charge; mount-private custody must retire.
    let charged = cache.resident_bytes();
    assert!(retained <= charged, "unmount retained {retained} bytes beyond cache charge {charged}");
    assert!(charged <= 4 * 1024 * 1024, "shared cache exceeded the 4 MiB budget");
    cache.discard_clean(disk.as_ref());
    assert_eq!(cache.resident_bytes(), 0, "private or dirty cache buffers survived unmount");
    // Reclaim the same writable device identity: a stale ownership claim
    // would refuse this mount with EBUSY before recovery (vfs.rs).
    drop(BreenFs::mount_writable(disk.clone(), cache.clone(), 24150, 1).unwrap());
    assert_eq!(Arc::strong_count(&disk), 1);
    assert_eq!(Arc::strong_count(&cache), 1);
    assert_eq!(cache.resident_bytes(), 0);
    assert!(start.elapsed().as_secs_f64() < 60.0, "U32: root count check exceeded 60 s");
}
