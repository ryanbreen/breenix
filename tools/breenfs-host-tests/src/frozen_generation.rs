//! E24 coordinator ledger 597(x11a), #3072: check only; no publication repair.
use super::*;
use alloc::vec;
use std::println;
use super::super::{btree, preempt::context::{self, Event}};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc,
    sync::{Mutex as HostMutex, atomic::AtomicUsize}, thread, time::{Duration, Instant}};

const FILES: usize = 4400;
const DIRTY_OPS: u64 = 100;
// 597 permits pointer/queue swaps, not generation work. Freeze exchanges the
// Files view, node/dirty/touch/deferred queues, grant custody and generation
// policy; install exchanges durable/pending/publication ownership. Budget two
// guard accesses per exchanged owner, at most eight owners in either section.
// Tree records/decodes/slots, cache visits and preemption work are also charged;
// a traversal cannot be hidden by borrowing the guard once before a loop.
const MAX_EXCLUDED_ITEMS: u64 = 16;

struct YieldWait;
impl Wait for YieldWait {
    fn generation(&self) -> u64 { 0 }
    fn wait(&self, _: u64) { thread::yield_now(); }
    fn wake_all(&self) {}
}
struct Disk {
    bytes: HostMutex<Vec<u8>>,
    mid_write: HostMutex<Option<Vec<u8>>>,
    record: core::sync::atomic::AtomicBool,
}
impl Disk {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes: HostMutex::new(bytes), mid_write: HostMutex::new(None),
            record: core::sync::atomic::AtomicBool::new(false) }
    }
    fn image(&self) -> Vec<u8> { self.bytes.lock().unwrap().clone() }
}
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { 16384 }
    fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
        let at = n as usize * BLOCK_SIZE;
        b.copy_from_slice(&self.bytes.lock().unwrap()[at..at+b.len()]); Ok(())
    }
    fn write_block(&self, n: u64, b: &[u8]) -> Result<(), BlockError> {
        let mut image = self.bytes.lock().unwrap();
        let at = n as usize * BLOCK_SIZE;
        image[at..at+b.len()].copy_from_slice(b);
        // Fixed mid-write cut: the first issued block survives, later writes
        // do not. Other cuts retain the issued prefix too, including ring writes.
        if self.record.swap(false, Ordering::SeqCst) {
            *self.mid_write.lock().unwrap() = Some(image.clone());
        }
        Ok(())
    }
    fn flush(&self) -> Result<(), BlockError> { Ok(()) }
}
type Fs = BreenFs<YieldWait>;
type Manifest = BTreeMap<format::Key, Vec<u8>>;
fn mount(disk: Arc<Disk>, writable: bool, id: usize) -> Arc<Fs> {
    let cache = Arc::new(BufferCache::new(YieldWait));
    if writable { BreenFs::mount_writable(disk, cache, id, 1).unwrap() }
    else { BreenFs::mount(disk, cache, id, 1).unwrap() }
}
// Compare the complete FS tree, including names, metadata, orphan records and
// data bytes. Replace physical EXTENT pointers with their actual block bytes:
// address allocation is not a POSIX property. No sampled-inode crash oracle.
fn manifest(fs: &Fs, image: &[u8]) -> Manifest {
    fs.inner.state.lock().files.items().unwrap().into_iter().map(|(key, value)| {
        let value = if key.item_type == format::EXTENT {
            let extent = format::Extent::decode(&value).unwrap();
            let at = (extent.phys_start + u64::from(extent.extent_offset)) as usize * BLOCK_SIZE;
            image[at..at + extent.num_blocks as usize * BLOCK_SIZE].to_vec()
        } else { value };
        (key, value)
    }).collect()
}
fn inode_key(ino: u64) -> format::Key { format::Key { object: ino, item_type: format::INODE, offset: 0 } }
fn expected_n(previous: &Manifest) -> Manifest {
    let mut expected = previous.clone();
    // Independent expected state for the two writes and 98 chmod operations.
    // The fixture starts with one data block filled with 0 per file; host time is 1.
    for ino in 16..16+DIRTY_OPS {
        let key = inode_key(ino);
        let mut inode = Inode::decode(&expected[&key]).unwrap();
        inode.ctime = Time { seconds: 1, nanos: 0 };
        if ino < 18 {
            inode.size = BLOCK_SIZE as u64;
            inode.mtime = inode.ctime;
            expected.insert(format::Key { object: ino, item_type: format::EXTENT, offset: 0 }, vec![b'N'; BLOCK_SIZE]);
        } else { inode.mode = 0o100600; }
        expected.insert(key, inode.encode().to_vec());
    }
    expected
}
fn write(fs: &Fs, node: &VNode, byte: u8) {
    assert_eq!(fs.write(node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
        clear_setid: false }, &vec![byte; BLOCK_SIZE]).unwrap().bytes, BLOCK_SIZE);
}
fn read(fs: &Fs, node: &VNode, byte: u8) -> bool {
    let mut bytes = [0; BLOCK_SIZE];
    fs.read(node, 0, &mut bytes) == Ok(BLOCK_SIZE) && bytes == [byte; BLOCK_SIZE]
}
#[derive(Default, Debug)]
struct Observed {
    sections: Vec<u64>,
    held: bool,
    accesses: u64,
    tree_start: [[u64; 3]; 2],
    cache_start: u64,
    excluded_yields: u64,
    preparing: bool,
    publication: bool,
    in_drain: bool,
    steps: u64,
    blocked_steps: u64,
    coherent_steps: u64,
    incoherent_steps: u64,
    issued: bool,
    preparation_end_seen: bool,
    next_generation_steps: u64,
    progress_before_end: [bool; 4],
    cuts: Vec<(&'static str, Vec<u8>)>,
}

#[test]
fn frozen_generation_keeps_foreground_progress_and_atomic_recovery() {
    let started = Instant::now();
    let disk = Arc::new(Disk::new(incremental_files_view::image(FILES).clone()));
    let fs = mount(disk.clone(), true, 241515597);
    let previous = manifest(&fs, &disk.image());
    let expected = expected_n(&previous);
    let generation = fs.inner.state.lock().durable.generation + 1;
    let first = fs.open(16).unwrap();
    let overwritten = fs.open(17).unwrap();
    write(&fs, &first, b'N');
    write(&fs, &overwritten, b'N');
    for ino in 18..16+DIRTY_OPS {
        fs.setattr(ino, SetAttr { mode: Some(0o600), ..SetAttr::default() }).unwrap();
    }
    assert_eq!(fs.inner.state.lock().durable.generation, generation-1);
    let observed = Rc::new(RefCell::new(Observed::default()));
    observed.borrow_mut().cuts.push(("before-freeze", disk.image()));
    let completed = Arc::new([const { AtomicUsize::new(0) }; 4]);
    let jobs = Rc::new(RefCell::new(Vec::<thread::JoinHandle<()>>::new()));
    let lock_id = &fs.inner.state as *const _ as usize;
    let (capture, workers, done, live, device, n_page, next_page) =
        (observed.clone(), jobs.clone(), completed.clone(), fs.clone(), disk.clone(), first.clone(), overwritten.clone());
    context::request();
    disk.record.store(true, Ordering::SeqCst);
    context::observer(Some(Box::new(move |event| {
        let mut o = capture.borrow_mut();
        match event {
            Event::Acquire(id) if id == lock_id => {
                o.held = true; o.accesses = 0;
                o.tree_start = btree::measurement::publication_counts();
                o.cache_start = crate::block::cache::measurement::discard_visits();
            }
            Event::Access(id) if id == lock_id && o.held => { o.accesses += 1; }
            Event::Release(id) if id == lock_id && o.held => {
                let trees = btree::measurement::publication_counts();
                let work = o.accesses + trees.iter().flatten().sum::<u64>()
                    - o.tree_start.iter().flatten().sum::<u64>()
                    + crate::block::cache::measurement::discard_visits() - o.cache_start;
                o.sections.push(work); o.held = false;
            }
            Event::Yield if o.held => { o.excluded_yields += 1; }
            Event::Phase { op: "commit", phase: "publication", begin } => {
                o.publication = begin;
                if begin { o.preparing = true; }
            }
            Event::Phase { op: "commit", phase: "drain-dirty", begin } => { o.in_drain = begin; }
            Event::Phase { op: "commit", phase: "next-files-view", begin: false } => {
                o.preparation_end_seen = true;
                o.progress_before_end = core::array::from_fn(|i| done[i].load(Ordering::SeqCst) == 2);
            }
            Event::Phase { op: "commit", phase: "write-cached", begin: true } => {
                o.preparing = false;
                // Join only AFTER measuring progress at preparation end. A
                // blocked operation may now finish; that cannot pass progress.
                drop(o);
                for job in workers.borrow_mut().drain(..) { job.join().unwrap(); }
                return;
            }
            Event::Phase { op: "commit", phase: "write-cached", begin: false } => {
                o.cuts.push(("before-install", device.image()));
            }
            Event::Point if o.preparing && o.publication => {
                context::request_reschedule();
                o.steps += 1;
                if o.held { o.accesses += 1; }
                if o.in_drain && !o.issued {
                    o.issued = true;
                    o.cuts.push(("mid-preparation", device.image()));
                    let parks = live.inner.state.parks();
                    for kind in 0..4 {
                        let (fs, node, flags) = (live.clone(), if kind == 0 { n_page.clone() } else { next_page.clone() }, done.clone());
                        workers.borrow_mut().push(thread::spawn(move || {
                            flags[kind].store(1, Ordering::SeqCst);
                            match kind {
                                0 => assert!(read(&fs, &node, b'N')),
                                1 => assert_eq!(fs.lookup(2, b"file-0000").unwrap(), 16),
                                2 => { fs.create(2, b"next-generation", NewNode::Regular {
                                    attrs: InodeAttrs::new(0o100600, 0, 0) }).unwrap(); }
                                3 => write(&fs, &node, b'O'),
                                _ => unreachable!(),
                            }
                            flags[kind].store(2, Ordering::SeqCst);
                        }));
                    }
                    // Scheduling watchdog only. The verdict compares completion
                    // ordering: finish OR demonstrably park behind exclusion.
                    let until = Instant::now()+Duration::from_secs(3);
                    loop {
                        let finished = done.iter().filter(|n| n.load(Ordering::SeqCst) == 2).count();
                        if finished == 4 || live.inner.state.parks()-parks + finished as u64 >= 4 { break; }
                        assert!(Instant::now()<until, "foreground actors did not run or park");
                        thread::yield_now();
                    }
                }
                // At EACH scheduler-exposed preparation step try to enter the
                // production read path. An excluded step is counted as a failed
                // observation, not bypassed with direct/unsafe MountState reads.
                if o.held { o.blocked_steps += 1; }
                else {
                    // At an unlocked boundary the paused publisher cannot
                    // obstruct these finite foreground operations. Join before
                    // choosing the expected layer, avoiding a read/write race.
                    for job in workers.borrow_mut().drain(..) { job.join().unwrap(); }
                    let newest = if done[3].load(Ordering::SeqCst) == 2 { b'O' } else { b'N' };
                    if read(&live, &n_page, b'N') && read(&live, &next_page, newest) {
                        o.coherent_steps += 1;
                        o.next_generation_steps += u64::from(newest == b'O');
                    } else { o.incoherent_steps += 1; }
                }
            }
            _ => (),
        }
    })));
    assert!(fs.commit_generation_up_to(0, generation, true).unwrap());
    context::observer(None);
    context::preempt_enable();
    let mut o = observed.borrow_mut();
    o.cuts.insert(2, ("mid-write", disk.mid_write.lock().unwrap().take().unwrap()));
    o.cuts.push(("after-install", disk.image()));
    assert_eq!(o.cuts.len(), 5);
    let mut crash_failures = 0;
    for (index, (cut, bytes)) in o.cuts.iter().enumerate() {
        let cold = mount(Arc::new(Disk::new(bytes.clone())), false, 241515600+index);
        let recovered = cold.inner.state.lock().durable.generation;
        let actual = manifest(&cold, bytes);
        let matches = (recovered == generation-1 && actual == previous)
            || (recovered == generation && actual == expected);
        println!("FROZEN_CRASH cut={cut} generation={recovered} exact={matches} records={}", actual.len());
        crash_failures += usize::from(!matches);
    }
    // Old handles and independently opened live handles must agree after N is
    // installed, including an N page overwritten into N+1 during preparation.
    let agree = read(&fs, &first, b'N') && read(&fs, &fs.open(16).unwrap(), b'N')
        && read(&fs, &overwritten, b'O') && read(&fs, &fs.open(17).unwrap(), b'O');
    let exclusion_ok = o.sections.len() == 2 && o.sections.iter().all(|n| *n <= MAX_EXCLUDED_ITEMS)
        && o.excluded_yields == 0;
    let progress_ok = o.issued && o.preparation_end_seen && o.progress_before_end == [true; 4];
    let coherence_ok = o.steps > 0 && o.coherent_steps == o.steps
        && o.next_generation_steps > 0 && o.blocked_steps == 0 && o.incoherent_steps == 0 && agree;
    println!("FROZEN files={FILES} dirty={DIRTY_OPS} c={MAX_EXCLUDED_ITEMS} sections={:?} excluded_yields={} steps={} blocked={} coherent={} incoherent={} progress={:?} next_generation_steps={} after_install_agree={agree} crash_failures={crash_failures} seconds={:.3}",
        o.sections, o.excluded_yields, o.steps, o.blocked_steps, o.coherent_steps,
        o.incoherent_steps, o.progress_before_end, o.next_generation_steps, started.elapsed().as_secs_f64());
    assert!(exclusion_ok && progress_ok && coherence_ok && crash_failures == 0,
        "597(x11a): exclusion={exclusion_ok}, progress={progress_ok}, coherence={coherence_ok}, crash_failures={crash_failures}");
}
