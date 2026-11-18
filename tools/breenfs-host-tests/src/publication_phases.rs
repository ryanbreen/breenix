//! E24 coordinator ledger 583(x8): count the remaining publication phases.
use super::*;
use super::super::btree;
use std::{cell::RefCell, collections::BTreeMap, sync::Mutex as HostMutex, time::Instant};

#[derive(Clone, Debug, Default)]
pub(super) struct Counts {
    pub work: [u64; 5], // NodeStates, mappings, orphans, grants, cache entries
    pub trees: [[u64; 3]; 2], // records, decodes, decoded slots
}
std::thread_local! {
    static ACTIVE: RefCell<Option<&'static str>> = const { RefCell::new(None) };
    static ROWS: RefCell<BTreeMap<&'static str, Counts>> = const { RefCell::new(BTreeMap::new()) };
}
pub(crate) fn visit(column: usize) {
    ACTIVE.with(|p| { if let Some(p) = *p.borrow() {
        ROWS.with(|r| r.borrow_mut().entry(p).or_default().work[column] += 1);
    }});
}
pub(super) struct Phase {
    previous: Option<&'static str>,
    name: &'static str,
    trees: [[u64; 3]; 2],
    cache: u64,
    orphans: u64,
}
impl Phase {
    pub(super) fn new(name: &'static str) -> Self {
        Self { previous: ACTIVE.with(|p| p.replace(Some(name))), name,
            trees: btree::measurement::publication_counts(),
            orphans: btree::measurement::orphan_visits(),
            cache: crate::block::cache::measurement::discard_visits() }
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        let now = btree::measurement::publication_counts();
        ROWS.with(|rows| { let mut rows = rows.borrow_mut(); let c = rows.entry(self.name).or_default();
            for t in 0..2 { for m in 0..3 { c.trees[t][m] += now[t][m] - self.trees[t][m]; }}
            c.work[2] += btree::measurement::orphan_visits() - self.orphans;
            c.work[4] += crate::block::cache::measurement::discard_visits() - self.cache;
        });
        ACTIVE.with(|p| p.replace(self.previous));
    }
}
pub(super) struct CacheCleanup(u64);
impl CacheCleanup {
    pub(super) fn new() -> Self { Self(crate::block::cache::measurement::discard_visits()) }
}
impl Drop for CacheCleanup {
    fn drop(&mut self) {
        let work = crate::block::cache::measurement::discard_visits() - self.0;
        ROWS.with(|r| r.borrow_mut().entry("cache_cleanup").or_default().work[4] += work);
    }
}
fn take() -> BTreeMap<&'static str, Counts> { ROWS.with(|r| core::mem::take(&mut *r.borrow_mut())) }

// Each changed inode is queued once for touch/close, and at most twice for
// retention or drain/charge. Cleanup visits its dirty node and its grants.
// No unchanged NodeState, mapping or orphan belongs to a generation's work.
const INDEX_ITEMS_PER_EDIT_LEVEL: u64 = 2;
// INODE plus ORPHAN deletion and one data extent use the COW and SPACE fixpoint
// paths counted by x5. Allow its 64 decodes per level for each of those three
// edits; a decode examines at most the format's minimum-width item capacity.
const RECORDS_PER_EDIT_LEVEL: u64 = 64;
const DECODES_PER_EDIT_LEVEL: u64 = 192;
const SLOTS_PER_EDIT_LEVEL: u64 = DECODES_PER_EDIT_LEVEL * ((format::NODE_SIZE as u64 - 48) / 21);
// Root handle, bounded inode cache and empty-range endpoints, independent of size.
const FIXED_WORK: u64 = 16;
// Cache removal reads and removes an entry (two visits). An inode COW path,
// three SPACE item paths per allocation, the format's eight-level maximum
// and ROOT/FREE/PENDING neighbours allow 32 new view entries per edit/level.
// This counts node-store retirement too, across the nested next-view span.
const CACHE_ITEMS_PER_EDIT_LEVEL: u64 = 64;

struct Disk(HostMutex<Vec<u8>>);
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { self.0.lock().unwrap().len() as u64 / BLOCK_SIZE as u64 }
    fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
        let at = n as usize * BLOCK_SIZE; b.copy_from_slice(&self.0.lock().unwrap()[at..at+b.len()]); Ok(())
    }
    fn write_block(&self, n: u64, b: &[u8]) -> Result<(), BlockError> {
        let at = n as usize * BLOCK_SIZE; self.0.lock().unwrap()[at..at+b.len()].copy_from_slice(b); Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
fn replay(files: usize) {
    let start = Instant::now();
    let baseline = incremental_files_view::image(files);
    let mut failures = Vec::new();
    for dirty in [10u64, 100] {
        let disk = Arc::new(Disk(HostMutex::new(baseline.clone())));
        let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(outstanding_grants::NoWait)),
            241515583 + files + dirty as usize, 1).unwrap();
        let root = fs.open(2).unwrap();
        // Keep the populated root's unchanged handles live, and warm their data
        // cache. Sixty-four unlinked-but-open files exercise orphan retention.
        let mut handles = Vec::new();
        {
            // Establish the identical live-handle set without exercising the
            // old retain_open scan after each setup open. Measurement below
            // still goes through the production operations and fsync path.
            let mut state = fs.inner.state.lock();
            for i in 0..files {
                let ino = 16+i as u64;
                let inode = state.files.inode(ino).unwrap();
                let node = fs.new_node(ino, inode, state.durable.generation).unwrap();
                let entry = state.nodes.entry(ino).or_default();
                entry.node = Arc::downgrade(&node);
                entry.touched = Some(node.pin::<NodePin<outstanding_grants::NoWait>>().unwrap().touched.clone());
                handles.push(Some(node));
            }
            state.files.retain_open_inodes(handles.iter().map(|n| n.as_ref().unwrap().ino).chain([2]).collect());
        }
        for i in 0..files {
            // Warm the same physical data blocks without a per-read inode/open
            // refresh in setup. Each cache entry still uses production verified I/O.
            fs.inner.cache.buffers().read_verified(fs.inner.data.as_ref(),20+i as u64,&mut [0; BLOCK_SIZE], |_| true).unwrap();
        }
        for i in 0..64 { fs.remove(2, alloc::format!("file-{i:04}").as_bytes(), Remove { directory: false, expected: 16+i as u64, owner: 0, parent: InodeAttrs { mode: 0o40755, uid: 0, gid: 0 } }).unwrap(); }
        fs.fsync(&root, false).unwrap();
        let before = fs.inner.state.lock().durable.generation;
        for group in 0..dirty/10 {
            let node = handles[100 + group as usize].as_ref().unwrap();
            fs.write(node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false }, b"x").unwrap();
            fs.touch(handles[200 + group as usize].as_ref().unwrap(), false);
            drop(handles[group as usize].take());
            for j in 0..7 { fs.setattr(316 + group*7+j, SetAttr { mode: Some(0o600), ..SetAttr::default() }).unwrap(); }
        }
        let heights = { let s=fs.inner.state.lock(); let p=s.files.reservation_paths(316).unwrap(); [p.fs_height,p.space_height] };
        take();
        fs.fsync(&root, false).unwrap();
        let rows = take();
        assert_eq!(fs.inner.state.lock().durable.generation, before+1);
        for (phase,c) in rows {
            println!("PHASE files={files} dirty={dirty} phase={phase} heights={heights:?} work={:?} trees={:?}", c.work,c.trees);
            let bound=dirty*(heights[0].max(heights[1])+1)*INDEX_ITEMS_PER_EDIT_LEVEL+FIXED_WORK;
            for (m,n) in c.work.into_iter().enumerate() {
                let bound = if phase == "cache_cleanup" { dirty*(heights[0].max(heights[1])+1)*CACHE_ITEMS_PER_EDIT_LEVEL+FIXED_WORK } else { bound };
                if n>bound { failures.push(alloc::format!("{dirty}/{phase}/work{m}={n}>{bound}")); }}
            for (t,h) in heights.into_iter().enumerate() {
                for (m,k) in [RECORDS_PER_EDIT_LEVEL,DECODES_PER_EDIT_LEVEL,SLOTS_PER_EDIT_LEVEL].into_iter().enumerate() {
                    let b=dirty*(h+1)*k+FIXED_WORK;
                    if c.trees[t][m]>b { failures.push(alloc::format!("{dirty}/{phase}/tree{t}/{m}={}>{b}",c.trees[t][m])); }
                }
            }
        }
        let cold = BreenFs::mount(disk.clone(), Arc::new(BufferCache::new(outstanding_grants::NoWait)),
            241515900 + files + dirty as usize, 1).unwrap();
        for i in 0..dirty/10 {
            assert_eq!(cold.getattr(216+i).unwrap().times.mtime.sec, 1);
        }
        for i in 0..dirty/10 { assert_eq!(fs.getattr(16+i).unwrap_err(), Errno(ENOENT));
            let mut byte=[0]; fs.read(handles[100+i as usize].as_ref().unwrap(),0,&mut byte).unwrap(); assert_eq!(byte,[b'x']); }
    }
    println!("PHASE_SECONDS files={files} seconds={:.3}",start.elapsed().as_secs_f64());
    assert!(failures.is_empty(),"583(x8) phase bounds: {failures:?}");
}
#[test]
fn populated_1100_phase_counts() { replay(1100); }
#[test]
fn populated_4400_phase_counts() { replay(4400); }

#[test]
fn publication_invalidates_reused_data_without_discarding_unchanged_data() {
    let disk = Arc::new(Disk(HostMutex::new(incremental_files_view::image(1100).clone())));
    let cache = Arc::new(BufferCache::new(outstanding_grants::NoWait));
    let fs = BreenFs::mount_writable(disk.clone(), cache.clone(), 241515999, 1).unwrap();
    let file = fs.open(16).unwrap();
    // Keep one unchanged data buffer resident. Seed the allocator's free range
    // with old disk bytes, as a prior generation's cached data would leave it.
    cache.read_verified(fs.inner.data.as_ref(),21,&mut [0; BLOCK_SIZE], |_| true).unwrap();
    let unchanged = cache.pin(fs.inner.data.as_ref(),21).unwrap();
    let sb = fs.inner.state.lock().durable.clone();
    let (start, length) = super::super::recover::space_items(&super::super::commit::DeviceStore(disk.as_ref()),&sb)
        .unwrap().into_iter().find_map(|(k,b)| (k.item_type == format::FREE)
            .then(|| (k.offset,format::Free::decode(&b).unwrap().length))).unwrap();
    let end = start + length.min(512);
    let mut old = Vec::new();
    for block in start..end {
        cache.read_verified(fs.inner.data.as_ref(),block,&mut [0; BLOCK_SIZE], |_| true).unwrap();
        old.push(cache.pin(fs.inner.data.as_ref(),block).unwrap());
    }
    fs.write(&file,WriteReq { at: WriteAt::Offset(0),limit:u64::MAX,clear_setid:false },b"x").unwrap();
    fs.fsync(&file,false).unwrap();
    let extent = fs.inner.state.lock().files.extents(16).unwrap()[0].1;
    assert!((start..end).contains(&extent.phys_start));
    assert!(cache.pin(fs.inner.data.as_ref(),extent.phys_start).is_none());
    assert!(Arc::ptr_eq(&unchanged,&cache.pin(fs.inner.data.as_ref(),21).unwrap()));
    let mut byte=[0]; fs.read(&file,0,&mut byte).unwrap(); assert_eq!(byte,[b'x']);
    drop(old);
}
