//! E24 coordinator ledger 597(x11a), #3072: check only; no publication repair.
use super::*;
use alloc::vec;
use std::println;
use super::super::{btree, preempt::context::{self, Event}};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc,
    sync::{Mutex as HostMutex, atomic::AtomicUsize, mpsc}, thread, time::{Duration, Instant}};

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
    reads: AtomicUsize,
}
impl Disk {
    fn new(bytes: Vec<u8>) -> Self {
        Self { bytes: HostMutex::new(bytes), mid_write: HostMutex::new(None),
            record: core::sync::atomic::AtomicBool::new(false), reads: AtomicUsize::new(0) }
    }
    fn image(&self) -> Vec<u8> { self.bytes.lock().unwrap().clone() }
}
impl BlockDevice for Disk {
    fn block_size(&self) -> usize { BLOCK_SIZE }
    fn num_blocks(&self) -> u64 { self.bytes.lock().unwrap().len() as u64 / BLOCK_SIZE as u64 }
    fn read_block(&self, n: u64, b: &mut [u8]) -> Result<(), BlockError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
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

const CREATE_REPLAY_WATCHDOG: Duration = Duration::from_secs(20);
const CREATE_ACQUISITIONS: usize = 8;

struct PausedPublication {
    resume: mpsc::Sender<()>,
    installed: mpsc::Receiver<Result<bool, Errno>>,
}

impl PausedPublication {
    fn install(self) {
        self.resume.send(()).unwrap();
        self.wait_for_install();
    }
    fn wait_for_install(self) {
        assert_eq!(self.installed.recv_timeout(CREATE_REPLAY_WATCHDOG).unwrap(), Ok(true));
    }
}

#[derive(Debug)]
struct InstallWait {
    epoch: u64,
    generation: Option<u64>,
    free: u64,
    reserve: u64,
    outstanding: u64,
}

#[derive(Debug, Default)]
struct CreateCosts {
    locks: usize,
    waits: Vec<InstallWait>,
    pressure_work: Vec<[u64; 3]>,
}

/// Count at the VFS entry used by open(O_CREAT). The mount acquisitions
/// used by the test to sample accounting are outside the observed call.
fn create_during_publication(
    fs: &Arc<Fs>,
    index: usize,
    frozen_grants: u64,
    pause: &Rc<RefCell<Option<PausedPublication>>>,
) -> (usize, usize, u64, u64, usize) {
    use super::super::reserve::observation;

    let lock_id = &fs.inner.state as *const _ as usize;
    let signal_id = Arc::as_ptr(&fs.inner.signal) as usize;
    let worker_epoch = fs.inner.signal.worker.epoch();
    let (generation, free, reserve, frozen, open) = {
        let state = fs.inner.state.lock();
        (state.durable.generation, state.durable.free_blocks, state.durable.global_reserve,
            if state.pending.is_some() { frozen_grants } else { 0 }, state.grants.blocks)
    };
    let expected_available = free.saturating_sub(reserve).saturating_sub(frozen + open);
    let costs = Rc::new(RefCell::new(CreateCosts::default()));
    let (capture, live, publication) = (costs.clone(), fs.clone(), pause.clone());
    context::observer(Some(Box::new(move |event| match event {
        Event::Acquire(id) if id == lock_id => capture.borrow_mut().locks += 1,
        Event::CommitWait { signal, epoch } if signal == signal_id => {
            let state = live.inner.state.try_lock().expect("admission waited with mount exclusion held");
            capture.borrow_mut().waits.push(InstallWait {
                epoch,
                generation: state.pending.as_ref().map(|_| state.durable.generation + 1),
                free: state.durable.free_blocks,
                reserve: state.durable.global_reserve,
                outstanding: state.reservations.outstanding(),
            });
            drop(state);
            if let Some(publisher) = publication.borrow_mut().take() {
                // The first wait belongs to the publication already in flight.
                publisher.install();
                let state = live.inner.state.lock();
                assert_eq!(state.durable.generation, generation + 1);
                assert!(state.pending.is_none());
                assert!(state.files.log.as_ref().unwrap().len() >= MATERIALIZE_STEPS);
            } else {
                // A new pressure request must make the worker runnable even
                // though the previous publisher installed and has exited.
                assert!(capture.borrow().pressure_work.is_empty(), "repeated pressure wait");
                assert_ne!(live.inner.signal.worker.epoch(), worker_epoch, "pressure did not wake the worker");
                let publisher = live.clone();
                let (complete, completed) = mpsc::channel();
                let worker = thread::spawn(move || {
                    let result = measurement::service(&publisher);
                    complete.send((result, measurement::take_work())).unwrap();
                });
                let (result, work) = completed.recv_timeout(CREATE_REPLAY_WATCHDOG).unwrap();
                worker.join().unwrap();
                assert_eq!(result, Ok(true));
                capture.borrow_mut().pressure_work.push(work);
            }
        }
        _ => (),
    })));
    measurement::take_events();
    measurement::take_materialized();
    measurement::take_work();
    observation::take_write_grant();
    let name = alloc::format!("create-{index:03}");
    let mut node = None;
    let result = fs.create_open(2, name.as_bytes(), NodeKind::Regular,
        &|_| Ok(InodeAttrs::new(0o644, 0, 0)),
        &mut |prepared| { node = Some(prepared); Ok(()) });
    context::observer(None);
    let replay = measurement::take_materialized();
    let work = measurement::take_work();
    let events = measurement::take_events();
    let allocation = observation::take_write_grant();
    let costs = costs.borrow();
    println!("CREATE index={index} durable={generation} free={free} reserve={reserve} frozen={frozen} open={open} available={expected_available} allocation={allocation:?} locks={} replay={replay} waits={:?} work={work:?} pressure_work={:?} result={result:?}",
        costs.locks, costs.waits, costs.pressure_work);
    assert_eq!(result, Ok(()));
    assert!(node.is_some(), "create did not prepare the opened VNode");
    assert!(costs.locks > 0);
    assert!(replay <= MATERIALIZE_STEPS as u64, "create {index} replayed {replay} steps");
    assert!(costs.locks <= CREATE_ACQUISITIONS, "create {index} acquired {} times", costs.locks);
    assert_eq!(work, [0; 3], "create {index} published during the replay");

    let mut admissions = events.into_iter().filter_map(|event| match event {
        measurement::Event::Admission { attempts, pressure } => Some((attempts, pressure)),
        _ => None,
    });
    let (attempts, pressure) = admissions.next().unwrap();
    assert!(admissions.next().is_none());
    if costs.pressure_work.is_empty() {
        assert!(pressure.is_empty(), "create {index} published with sufficient installed space");
    } else {
        assert_eq!(costs.pressure_work, [[1, 0, 1]]);
        assert_eq!(pressure, [true]);
    }
    let (success, refusals) = attempts.split_last().unwrap();
    assert_eq!(success.error, None);
    assert_eq!(success.refusal, None);
    let [admitted_free, admitted_reserve, charged_with_grant, request, available] = allocation.unwrap();
    assert!(request <= available);
    assert_eq!(available, admitted_free.saturating_sub(admitted_reserve)
        .saturating_sub(charged_with_grant - request));
    assert_eq!(refusals.len(), costs.waits.len(), "create {index} refusal-to-wait attribution");
    let mut installed = generation;
    for (refusal, wait) in refusals.iter().zip(&costs.waits) {
        println!("CREATE_REFUSAL index={index} epoch={} refusal={:?}", wait.epoch, refusal.refusal);
        assert_eq!(refusal.error, Some(ENOSPC));
        let [free_at_refusal, reserve_at_refusal, outstanding, requested, space] = refusal.refusal.unwrap();
        assert_eq!(wait.generation, (frozen > 0).then_some(generation + 1), "wait epoch {}", wait.epoch);
        assert_eq!((wait.free, wait.reserve, wait.outstanding),
            (free_at_refusal, reserve_at_refusal, outstanding));
        assert_eq!((free_at_refusal, reserve_at_refusal, outstanding, space),
            (free, reserve, frozen + open, expected_available));
        assert!(requested > space, "create {index} waited for install with adequate space");
        installed += 1;
    }
    assert_eq!(fs.inner.state.lock().durable.generation, installed);
    if costs.waits.is_empty() {
        assert_eq!((admitted_free, admitted_reserve, charged_with_grant - request, available),
            (free, reserve, frozen + open, expected_available));
    }
    (costs.waits.len(), costs.locks, replay, available - request, costs.pressure_work.len())
}

#[test]
fn create_during_frozen_preparation_has_exact_admission_and_bounded_replay() {
    const CREATES: usize = 300;
    let started = Instant::now();
    let disk = Arc::new(Disk::new(incremental_files_view::image(FILES).clone()));
    let fs = mount(disk, true, 241515613);
    write(&fs, &fs.open(16).unwrap(), b'N');
    write(&fs, &fs.open(17).unwrap(), b'N');
    for ino in 18..16 + DIRTY_OPS {
        fs.setattr(ino, SetAttr { mode: Some(0o600), ..SetAttr::default() }).unwrap();
    }
    let (generation, frozen_grants) = {
        let state = fs.inner.state.lock();
        (state.durable.generation + 1, state.grants.blocks)
    };
    assert!(frozen_grants > 0);
    let (ready_tx, ready) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel();
    let (installed_tx, installed) = mpsc::channel();
    let publisher = fs.clone();
    let worker = thread::spawn(move || {
        let mut armed = false;
        context::observer(Some(Box::new(move |event| match event {
            Event::Phase { op: "commit", phase: "drain-dirty", begin: true } => armed = true,
            Event::Point if armed => {
                armed = false;
                ready_tx.send(()).unwrap();
                resume_rx.recv_timeout(CREATE_REPLAY_WATCHDOG).unwrap();
            }
            _ => (),
        })));
        installed_tx.send(publisher.commit_generation_up_to(0, generation, true)).unwrap();
        context::observer(None);
    });
    ready.recv_timeout(CREATE_REPLAY_WATCHDOG).unwrap();
    assert!(fs.inner.state.lock().pending.is_some());
    let pause = Rc::new(RefCell::new(Some(PausedPublication { resume, installed })));
    let (mut waits, mut locks, mut max_steps, mut min_headroom, mut pressure) = (0, 0, 0, u64::MAX, 0);
    let mut first_after_install = None;
    for i in 0..=CREATES {
        let after_install = pause.borrow().is_none() && first_after_install.is_none();
        let (waited, acquired, steps, headroom, publications) = create_during_publication(&fs, i, frozen_grants, &pause);
        if after_install {
            first_after_install = Some((waited, acquired, steps));
        }
        waits += waited;
        locks += acquired;
        max_steps = max_steps.max(steps);
        min_headroom = min_headroom.min(headroom);
        pressure += publications;
    }
    if let Some(publication) = pause.borrow_mut().take() {
        publication.install();
    }
    worker.join().unwrap();
    // A create while preparation is held has no installed log to replay.
    // Exercise the bound with the same log still backlogged after install.
    let (after_waits, after_locks, after_steps) = first_after_install.unwrap();
    assert!(after_steps > 0, "post-install create reached no queued replay");
    assert_eq!(pressure, 1, "post-install shortage must publish exactly once");
    println!("CREATE_SUMMARY files={FILES} dirty={DIRTY_OPS} creates={CREATES} generation={generation} waits={waits} locks={locks} peak_replay={max_steps} min_grant_headroom={min_headroom} post_install_waits={after_waits} post_install_locks={after_locks} post_install_replay={after_steps} pressure_publications={pressure} seconds={:.3}",
        started.elapsed().as_secs_f64());
}

// E24 coordinator 621(y2): create admission at the guest's real headroom.
// The stopped ARM root of run 20261119T051835Z-qemu-44689 (generation 889)
// had 19,092 FREE blocks, global_reserve 4,096 and 156 PENDING blocks.
const SHORTAGE_FREE: u64 = 19_092;
// outstanding_grants::populated_image(80_000) leaves 16,084 FREE blocks.
const X1_FREE: u64 = 16_084;
// A refused admission records its first eight receipts per installed
// generation; the report keeps the newest RECEIPT_RING of them.
const RECEIPTS_PER_PUBLICATION: usize = 8;
const RECEIPT_RING: usize = 32;

/// One background publication, paused at its first drain-dirty preemption
/// point exactly as the commit worker's would be while it prepares N.
struct Publisher {
    paused: PausedPublication,
    preparing_reap: mpsc::Receiver<()>,
    thread: thread::JoinHandle<()>,
    generation: u64,
    grants: u64,
}

/// The commit worker's own entry, run when the x2 trigger asked for it.
fn start_publisher(fs: &Arc<Fs>) -> Publisher {
    let (generation, grants) = {
        let state = fs.inner.state.lock();
        assert!(state.pending.is_none());
        (state.durable.generation + 1, state.grants.blocks)
    };
    let (ready_tx, ready) = mpsc::channel();
    let (resume, resume_rx) = mpsc::channel();
    let (reap_tx, preparing_reap) = mpsc::channel();
    let (installed_tx, installed) = mpsc::channel();
    let worker = fs.clone();
    let thread = thread::spawn(move || {
        let mut armed = false;
        let paused = ready_tx.clone();
        context::observer(Some(Box::new(move |event| match event {
            Event::Phase { op: "commit", phase: "drain-dirty", begin: true } => armed = true,
            Event::Phase { op: "commit", phase: "prepare-reap", begin: true } => { let _ = reap_tx.send(()); }
            Event::Point if armed => {
                armed = false;
                paused.send(true).unwrap();
                resume_rx.recv_timeout(CREATE_REPLAY_WATCHDOG).unwrap();
            }
            _ => (),
        })));
        let result = measurement::service(&*worker);
        context::observer(None);
        // A publication that stopped before drain-dirty reports false.
        let _ = ready_tx.send(false);
        installed_tx.send(result).unwrap();
    });
    assert!(ready.recv_timeout(CREATE_REPLAY_WATCHDOG).unwrap(), "the trigger publication did not freeze");
    assert!(fs.inner.state.lock().pending.is_some());
    Publisher { paused: PausedPublication { resume, installed }, preparing_reap, thread, generation, grants }
}

/// PENDING blocks in the installed space tree and the oldest released
/// generation among them (design 2.4, 2.5 reuse rule).
fn pending_census(fs: &Fs) -> (u64, u64) {
    let state = fs.inner.state.lock();
    let sb = &state.durable;
    let store = fs.inner.cache.store(fs.inner.device.clone(), sb);
    let lower = format::Key { object: 0xffff_ffff_ffff_ff30, item_type: format::PENDING, offset: 0 };
    let upper = format::Key { offset: u64::MAX, ..lower };
    let (mut blocks, mut oldest) = (0, 0);
    for (_, bytes) in btree::range(&store, sb.space_root, lower, upper).unwrap() {
        let pending = format::Pending::decode(&bytes).unwrap();
        // Commit G converts released_generation <= G - 2 (promote_pending).
        assert!(pending.released_generation + 2 > sb.generation,
            "installed generation {} still quarantines {pending:?}", sb.generation);
        blocks += pending.length;
        oldest = if oldest == 0 { pending.released_generation } else { oldest.min(pending.released_generation) };
    }
    (blocks, oldest)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Receipt {
    op: &'static str,
    generation: u64,
    release_generation: u64,
    free: u64,
    reserve: u64,
    pending: u64,
    pending_oldest: u64,
    grants_n: u64,
    grants_next: u64,
    request: u64,
}

#[derive(Debug)]
struct ShortageWait {
    generation: u64,
    named: u64,
    free: u64,
    reserve: u64,
    outstanding: u64,
    grants_next: u64,
    pending: (u64, u64),
}

fn report_receipts(fs: &Fs) -> Vec<(u64, Receipt)> {
    let mut text = String::new();
    fs.mutation_costs("root", &mut text).unwrap();
    text.lines().filter(|line| line.starts_with("admission_refusal ")).map(|line| {
        let value = |key: &str| line.split_whitespace()
            .find_map(|field| field.strip_prefix(key)).unwrap_or_else(|| panic!("{key} in {line}"));
        let number = |key: &str| value(key).parse::<u64>().unwrap();
        assert_eq!((number("mount="), number("root=")), (fs.inner.id as u64, fs.inner.root_id), "{line}");
        let op = match value("op=") { "create" => "create", "write" => "write", other => panic!("op {other}") };
        (number("seq="), Receipt {
            op, generation: number("generation="), release_generation: number("release_generation="),
            free: number("free="), reserve: number("reserve="), pending: number("pending="),
            pending_oldest: number("pending_oldest="), grants_n: number("grants_n="),
            grants_next: number("grants_next="), request: number("request="),
        })
    }).collect()
}

#[test]
fn create_shortage_waits_only_for_the_install_that_frees_it() {
    use super::super::reserve::observation;
    const CREATES: usize = 300;
    let started = Instant::now();
    let image = outstanding_grants::populated_image(80_000 + SHORTAGE_FREE - X1_FREE);
    let fs = mount(Arc::new(Disk::new(image)), true, 241515621);
    let built = started.elapsed().as_secs_f64();
    {
        let state = fs.inner.state.lock();
        assert_eq!((state.durable.free_blocks, state.durable.global_reserve), (SHORTAGE_FREE, 4096));
    }
    // libc's readdir-many creates its names in a fresh directory of the root.
    let dir = fs.lookup(2, b"tmp").unwrap();
    let signal_id = Arc::as_ptr(&fs.inner.signal) as usize;
    let publisher: Rc<RefCell<Option<Publisher>>> = Rc::new(RefCell::new(None));
    let mut expected = Vec::new();
    let mut in_window = (0, 0);
    let (mut publications, mut refusals, mut write_refusals, mut retries) = (0, 0, 0, 0);
    let (mut peak_pending, mut max_replay, mut since_publication) = (0, 0, 0);
    let mut files_per_publication = Vec::new();
    let (mut create_requests, mut create_retained, mut write_grants) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..CREATES {
        if publisher.borrow().is_none() && fs.inner.signal.requested.load(Ordering::Acquire) {
            *publisher.borrow_mut() = Some(start_publisher(&fs));
            publications += 1;
            files_per_publication.push(core::mem::take(&mut since_publication));
        }
        since_publication += 1;
        let name = alloc::format!("f{i:03}xxxxxxxxx");
        let mut node = None;
        for op in ["create", "write"] {
            let waits = Rc::new(RefCell::new(Vec::<ShortageWait>::new()));
            let (observed, live, owner) = (waits.clone(), fs.clone(), publisher.clone());
            context::observer(Some(Box::new(move |event| {
                let Event::CommitWait { signal, .. } = event else { return };
                if signal != signal_id { return; }
                let (generation, free, reserve, outstanding, grants_next, pending) = {
                    let state = live.inner.state.try_lock().expect("admission waited with mount exclusion held");
                    (state.durable.generation, state.durable.free_blocks, state.durable.global_reserve,
                        state.reservations.outstanding(), state.grants.blocks, state.pending.is_some())
                };
                assert!(pending, "{op} waited with no publication in flight");
                let census = pending_census(&live);
                let Publisher { paused, thread, generation: named, .. } =
                    owner.borrow_mut().take().expect("wait without the worker's publication");
                observed.borrow_mut().push(ShortageWait { generation, named, free, reserve,
                    outstanding, grants_next, pending: census });
                // The install this refusal named: resume the worker and let it finish.
                paused.install();
                thread.join().unwrap();
            })));
            measurement::take_events();
            measurement::take_materialized();
            observation::take_write_grant();
            let (before, frozen) = (fs.inner.state.lock().reservations.outstanding(),
                publisher.borrow().as_ref().map_or(0, |p| p.grants));
            let named = publisher.borrow().as_ref().map(|p| p.generation);
            if op == "create" {
                fs.create_open(dir, name.as_bytes(), NodeKind::Regular,
                    &|_| Ok(InodeAttrs::new(0o644, 0, 0)),
                    &mut |prepared| { node = Some(prepared); Ok(()) }).unwrap();
            } else {
                // h_put_file writes one byte and closes before the next create.
                let opened = node.take().unwrap();
                assert_eq!(fs.write(&opened, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
                    clear_setid: false }, b"x").unwrap().bytes, 1);
            }
            context::observer(None);
            let replay = measurement::take_materialized();
            assert!(replay <= MATERIALIZE_STEPS as u64, "{op} {i} replayed {replay} items");
            max_replay = max_replay.max(replay);
            let grant = observation::take_write_grant().unwrap();
            let after = fs.inner.state.lock().reservations.outstanding();
            if op == "create" {
                create_requests.push(grant[3]);
                create_retained.push(after.saturating_sub(before));
            } else {
                write_grants.push(grant[3]);
            }
            let mut admissions = measurement::take_events().into_iter().filter_map(|event| match event {
                measurement::Event::Admission { attempts, pressure } => Some((attempts, pressure)),
                _ => None,
            });
            let (attempts, pressure) = admissions.next().unwrap();
            assert!(admissions.next().is_none());
            assert!(pressure.is_empty(), "{op} {i}: the named install did not free its request");
            let (success, denied) = attempts.split_last().unwrap();
            assert_eq!((success.error, success.refusal), (None, None));
            let waits = core::mem::take(&mut *waits.borrow_mut());
            assert_eq!(denied.len(), waits.len(), "{op} {i}: refusal-to-wait attribution");
            for (refusal, wait) in denied.iter().zip(&waits) {
                assert_eq!(refusal.error, Some(ENOSPC));
                let [free, reserve, outstanding, request, available] = refusal.refusal.unwrap();
                assert_eq!((free, reserve, outstanding), (wait.free, wait.reserve, wait.outstanding));
                // The worker is paused between its freeze and install, so
                // outstanding equals frozen N's grants plus open N+1's.
                assert_eq!(outstanding, frozen + wait.grants_next, "{op} {i}");
                assert_eq!(available, free.saturating_sub(reserve).saturating_sub(frozen + wait.grants_next));
                assert!(available < request, "{op} {i} waited with {available} for {request}");
                assert_eq!(Some(wait.named), named);
                assert_eq!(wait.named, wait.generation + 1);
                println!("SHORTAGE op={op} index={i} generation={} release_generation={} free={free} reserve={reserve} pending={} pending_oldest={} grants_n={frozen} grants_next={} request={request} available={available}",
                    wait.generation, wait.named, wait.pending.0, wait.pending.1, wait.grants_next);
                if in_window.0 != wait.generation { in_window = (wait.generation, 0); }
                if in_window.1 < RECEIPTS_PER_PUBLICATION {
                    in_window.1 += 1;
                    expected.push(Receipt { op, generation: wait.generation, release_generation: wait.named,
                        free, reserve, pending: wait.pending.0, pending_oldest: wait.pending.1,
                        grants_n: frozen, grants_next: wait.grants_next, request });
                }
                if op == "create" { refusals += 1 } else { write_refusals += 1 }
            }
            if let Some(wait) = waits.last() {
                // The retry that succeeded ran after the install it named.
                assert_eq!(fs.inner.state.lock().durable.generation, wait.named);
                peak_pending = peak_pending.max(pending_census(&fs).0);
                retries += 1;
            }
        }
    }
    let leftover = publisher.borrow_mut().take();
    if let Some(Publisher { paused, thread, .. }) = leftover {
        paused.install();
        thread.join().unwrap();
        peak_pending = peak_pending.max(pending_census(&fs).0);
    }
    let range = |values: &[u64]| (values.iter().min().copied().unwrap_or(0), values.iter().max().copied().unwrap_or(0));
    println!("SHORTAGE_SUMMARY creates={CREATES} free={SHORTAGE_FREE} reserve=4096 publications={publications} files_per_publication={:?} create_refusals={refusals} write_refusals={write_refusals} retries_after_install={retries} pressure_publications=0 peak_pending={peak_pending} peak_replay={max_replay} create_request={:?} create_retained={:?} write_grant={:?} fixture_seconds={built:.3} seconds={:.3}",
        files_per_publication.get(1..).unwrap_or(&[]), range(&create_requests), range(&create_retained), range(&write_grants),
        started.elapsed().as_secs_f64());
    assert!(publications > 1 && refusals + write_refusals > 0, "the replay reached no real shortage");

    // 621(y2) receipt: the first eight refusals per installed generation,
    // emitted with the x10 mutation_cost records, newest RECEIPT_RING kept.
    let receipts = report_receipts(&fs);
    let kept = expected.len().min(RECEIPT_RING);
    assert_eq!(receipts.iter().map(|(_, receipt)| *receipt).collect::<Vec<_>>(),
        expected[expected.len() - kept..].to_vec(), "refusal receipts");
    assert!(receipts.windows(2).all(|pair| pair[0].0 + 1 == pair[1].0));
    assert_eq!(receipts.last().map(|(seq, _)| *seq), Some(expected.len() as u64));
}

#[test]
fn readdir_many_setups_enumerates_and_unlinks_with_bounded_grants() {
    use super::super::reserve::observation;

    const GUEST_FREE: u64 = 25_075;
    let started = Instant::now();
    let image = outstanding_grants::populated_image(80_000 + GUEST_FREE - X1_FREE);
    let fs = mount(Arc::new(Disk::new(image)), true, 241515630);
    {
        let state = fs.inner.state.lock();
        let paths = state.files.reservation_paths(2).unwrap();
        assert_eq!((paths.fs_height, paths.space_height), (1, 1));
        assert_eq!((state.durable.free_blocks, state.durable.global_reserve), (GUEST_FREE, 4096));
    }
    let attrs = InodeAttrs::new(0o644, 0, 0);
    let dir = fs.create(2, b"libc-readdir-many", NewNode::Directory {
        attrs: InodeAttrs::new(0o700, 0, 0),
    }).unwrap();
    let before = fs.inner.state.lock().durable.generation;
    // Calls, maximum and total granted blocks, retries, pressure publications,
    // worker publications, maximum replay steps.
    let mut rows = [[0u64; 7]; 3];
    let mut unlink_ns = 0;
    let mut names = BTreeMap::new();
    let mut observe = |phase: usize| {
        let grant = observation::take_write_grant();
        let events = measurement::take_events();
        let replay = measurement::take_materialized();
        let row = &mut rows[phase];
        row[0] += 1;
        if let Some(grant) = grant {
            row[1] = row[1].max(grant[3]);
            row[2] += grant[3];
        }
        row[6] = row[6].max(replay);
        for event in events {
            if let measurement::Event::Admission { attempts, pressure } = event {
                assert_eq!(attempts.last().unwrap().error, None);
                row[3] += (attempts.len() - 1) as u64;
                row[4] += pressure.into_iter().filter(|&worked| worked).count() as u64;
            }
        }
        // Deliver a deterministic worker service opportunity without running
        // the periodic timer. The y2 replay separately parks a frozen worker.
        if fs.inner.signal.requested.load(Ordering::Acquire) {
            let worked = fs.commit_due(0).unwrap();
            row[5] += u64::from(worked);
        }
    };
    measurement::take_events();
    measurement::take_materialized();
    publication_phases::take();
    let replay_started = Instant::now();
    for i in 0..300 {
        let name = alloc::format!("f{i:03}xxxxxxxxx");
        let mut node = None;
        fs.create_open(dir, name.as_bytes(), NodeKind::Regular,
            &|_| Ok(attrs), &mut |opened| { node = Some(opened); Ok(()) }).unwrap();
        observe(0);
        let node = node.unwrap();
        assert_eq!(fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
            clear_setid: true }, b"x").unwrap().bytes, 1);
        names.insert(name.into_bytes(), node.ino);
        drop(node);
        observe(1);
    }
    let mut seen = BTreeMap::new();
    let mut cursor = 2;
    loop {
        let mut count = 0;
        let next = fs.readdir(dir, cursor, &mut |entry, _| {
            if count == 32 { return Ok(false); }
            assert!(seen.insert(entry.name, entry.ino).is_none());
            count += 1;
            Ok(true)
        }).unwrap();
        if next == cursor { break; }
        cursor = next;
    }
    assert_eq!(seen, names);
    let parent = fs.getattr(dir).unwrap().attrs;
    for (name, ino) in names {
        let at = Instant::now();
        fs.remove(dir, &name, Remove { directory: false, expected: ino, owner: 0, parent }).unwrap();
        unlink_ns += at.elapsed().as_nanos();
        observe(2);
    }
    fs.remove(2, b"libc-readdir-many", Remove { directory: true, expected: dir, owner: 0,
        parent: fs.getattr(2).unwrap().attrs }).unwrap();
    let after = fs.inner.state.lock().durable.generation;
    let reap = publication_phases::take().remove("prepare_reap").unwrap_or_default();
    let publications = after - before;
    let replay_seconds = replay_started.elapsed().as_secs_f64();
    let mut costs = String::new();
    fs.mutation_costs("root", &mut costs).unwrap();
    for line in costs.lines().filter(|line| line.contains(" op=unlink ")) {
        println!("READDIR_MANY_UNLINK {line}");
    }
    fs.fsync(&fs.open(2).unwrap(), false).unwrap();
    assert_eq!(fs.lookup(2, b"libc-readdir-many"), Err(Errno(ENOENT)));
    println!(concat!("READDIR_MANY rows={:?} publications={} ",
        "prepare_work={:?} prepare_trees={:?} unlink_host_ns={} ",
        "guest_publication_seconds={:.3} replay_seconds={:.3} seconds={:.3}"),
        rows, publications, reap.work, reap.trees, unlink_ns, publications as f64 * (6.174_023_123 / 8.0),
        replay_seconds, started.elapsed().as_secs_f64());
    assert_eq!(rows.map(|row| row[0]), [300; 3]);
    assert_eq!(publications, rows.iter().map(|row| row[4] + row[5]).sum());
    assert!(rows.iter().all(|row| row[6] <= MATERIALIZE_STEPS as u64));
    assert!(rows[0][1] < 400 && rows[1][2] < 300 * 160, "height-one admission: {rows:?}");
    assert!(publications <= 6, "reservation costs still dominate the libc case");
}

#[test]
fn readdir_many_admits_under_captured_headroom_during_publication() {
    use super::super::reserve::observation;

    const GUEST_FREE: u64 = 25_075;
    const HEADROOM: [u64; 6] = [178, 489, 257, 197, 169, 443];
    let started = Instant::now();
    let image = outstanding_grants::populated_image(80_000 + GUEST_FREE - X1_FREE);
    let fs = mount(Arc::new(Disk::new(image)), true, 241515646);
    {
        let state = fs.inner.state.lock();
        let paths = state.files.reservation_paths(2).unwrap();
        assert_eq!((paths.fs_height, paths.space_height), (1, 1));
        assert_eq!((state.durable.free_blocks, state.durable.global_reserve), (GUEST_FREE, 4096));
    }
    let attrs = InodeAttrs::new(0o644, 0, 0);
    let dir = fs.create(2, b"libc-readdir-many", NewNode::Directory {
        attrs: InodeAttrs::new(0o700, 0, 0),
    }).unwrap();
    let signal = Arc::as_ptr(&fs.inner.signal) as usize;
    let active = Rc::new(RefCell::new(None::<Publisher>));
    let waits = Rc::new(RefCell::new(0u64));
    let (wait_count, worker) = (waits.clone(), active.clone());
    let install = |publisher: Publisher| {
        publisher.paused.install();
        publisher.thread.join().unwrap();
    };
    context::observer(Some(Box::new(move |event| {
        if matches!(event, Event::CommitWait { signal: id, .. } if id == signal) {
            *wait_count.borrow_mut() += 1;
            if let Some(publisher) = worker.borrow_mut().take() { install(publisher); }
        }
    })));

    let mut counts = [[0u64; 6]; 4];
    let mut publications = 0;
    let mut in_flight = 0;
    let mut drive = |phase: usize, overlaps: bool| {
        let row = &mut counts[phase];
        row[0] += 1;
        row[1] += u64::from(overlaps && active.borrow().is_some());
        let steps = measurement::take_materialized();
        row[4] += steps;
        row[5] = row[5].max(steps);
        for event in measurement::take_events() {
            if let measurement::Event::Admission { attempts, pressure } = event {
                assert_eq!(attempts.last().unwrap().error, None);
                row[2] += (attempts.len() - 1) as u64;
                row[3] += pressure.into_iter().filter(|&worked| worked).count() as u64;
            }
        }
        if overlaps && active.borrow().is_some() {
            in_flight += 1;
            if in_flight == 16 { install(active.borrow_mut().take().unwrap()); }
        }
        if active.borrow().is_none() && fs.inner.signal.requested.load(Ordering::Acquire) {
            *active.borrow_mut() = Some(start_publisher(&fs));
            publications += 1;
            in_flight = 0;
        }
    };
    measurement::take_events();
    measurement::take_materialized();
    let mut names = BTreeMap::new();
    let mut probes = Vec::new();
    let mut prefix = 0;
    let mut generation = None;
    let mut frozen_write = 0;
    for i in 0..300 {
        let name = alloc::format!("f{i:03}xxxxxxxxx");
        let mut opened = None;
        let frozen = active.borrow().as_ref().map(|p| p.generation);
        if frozen != generation { generation = frozen; prefix = 0; }
        // Match the capture's admission headroom after earlier scratch grants.
        // The fixture grant occupies only the difference and is released after
        // this operation. The worker owns N and resumes even on the red path.
        let probe = frozen.filter(|_| prefix >= 2).and_then(|_| HEADROOM.get(probes.len()).copied());
        let held = probe.map(|headroom| {
            let mut state = fs.inner.state.lock();
            let free = state.durable.free_blocks;
            let available = state.reservations.available(free);
            assert!(available >= headroom);
            let held = state.reservations.reserve(free, available - headroom, Operation::Allocate).unwrap();
            assert_eq!(state.reservations.available(free), headroom);
            held
        });
        fs.create_open(dir, name.as_bytes(), NodeKind::Regular,
            &|_| Ok(attrs), &mut |node| { opened = Some(node); Ok(()) }).unwrap();
        if let Some(held) = held {
            fs.inner.state.lock().reservations.release(held);
            probes.push((probe.unwrap(), active.borrow().is_some()));
        }
        drive(0, frozen.is_some());
        let node = opened.unwrap();
        let writing_frozen = active.borrow().as_ref().map(|p| p.generation);
        observation::take_write_grant();
        assert_eq!(fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
            clear_setid: true }, b"x").unwrap().bytes, 1);
        if prefix >= 2 && writing_frozen.is_some() && writing_frozen == generation {
            frozen_write = frozen_write.max(observation::take_write_grant().unwrap()[3]);
        }
        names.insert(name.into_bytes(), node.ino);
        drop(node);
        drive(1, writing_frozen.is_some());
        prefix += 1;
    }
    // Start a fresh publication at enumeration. Its worker must advance from
    // drain to prepare-reap while readdir is still visiting its first entry.
    if let Some(publisher) = active.borrow_mut().take() { install(publisher); }
    fs.inner.signal.requested.store(true, Ordering::Release);
    drive(2, false);
    let mut seen = BTreeMap::new();
    let mut cursor = 2;
    loop {
        let mut count = 0;
        let overlaps = active.borrow().is_some();
        let next = fs.readdir(dir, cursor, &mut |entry, _| {
            if cursor == 2 && count == 0 {
                let worker = active.borrow();
                let publisher = worker.as_ref().unwrap();
                publisher.paused.resume.send(()).unwrap();
                publisher.preparing_reap.recv_timeout(CREATE_REPLAY_WATCHDOG).unwrap();
            }
            if count == 32 { return Ok(false); }
            assert!(seen.insert(entry.name, entry.ino).is_none());
            count += 1;
            Ok(true)
        }).unwrap();
        drive(2, overlaps);
        if cursor == 2 {
            let publisher = active.borrow_mut().take().unwrap();
            publisher.paused.wait_for_install();
            publisher.thread.join().unwrap();
        }
        if next == cursor { break; }
        cursor = next;
    }
    assert_eq!(seen, names);
    let parent = fs.getattr(dir).unwrap().attrs;
    if active.borrow().is_none() {
        fs.inner.signal.requested.store(true, Ordering::Release);
        drive(3, false);
    }
    for (name, ino) in names {
        let overlaps = active.borrow().is_some();
        fs.remove(dir, &name, Remove { directory: false, expected: ino, owner: 0, parent }).unwrap();
        drive(3, overlaps);
    }
    if let Some(publisher) = active.borrow_mut().take() { install(publisher); }
    context::observer(None);
    fs.remove(2, b"libc-readdir-many", Remove { directory: true, expected: dir, owner: 0,
        parent: fs.getattr(2).unwrap().attrs }).unwrap();
    fs.fsync(&fs.open(2).unwrap(), false).unwrap();
    assert_eq!(fs.lookup(2, b"libc-readdir-many"), Err(Errno(ENOENT)));
    println!("FROZEN_READDIR calls/overlap/refusals/pressure/replay/peak={counts:?} waits={} background_publications={publications} probes={probes:?} peak_shared_write={frozen_write} seconds={:.3}",
        waits.borrow(), started.elapsed().as_secs_f64());
    assert!(counts.iter().all(|row| row[1] > 0));
    assert_eq!(probes, HEADROOM.map(|blocks| (blocks, true)), "funded scratch generation should admit each create before install");
    assert!((1..169).contains(&frozen_write), "a subsequent first write must share the generation's SPACE base");
}

#[test]
fn frozen_create_and_first_write_replay_a_cascading_split() {
    use super::super::reserve::observation;

    let image = incremental_files_view::full_height_one_image();
    for create in [true, false] {
        let fs = mount(Arc::new(Disk::new(image.clone())), true, 646_000 + usize::from(create));
        fs.inner.signal.requested.store(true, Ordering::Release);
        let publisher = start_publisher(&fs);
        let installed = publisher.generation;
        let node = if create {
            let ino = fs.create(2, b"new", NewNode::Regular { attrs: InodeAttrs::new(0o644, 0, 0) }).unwrap();
            assert_eq!(ino, 17);
            fs.open(ino).unwrap()
        } else {
            let opened = fs.open(16).unwrap();
            assert_eq!(fs.write(&opened, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
                clear_setid: false }, b"x").unwrap().bytes, 1);
            opened
        };
        let request = observation::take_write_grant().unwrap()[3];
        assert!(fs.inner.state.lock().pending.is_some());
        publisher.paused.install();
        publisher.thread.join().unwrap();
        let (before, retained, replay_height) = {
            let state = fs.foreground_lock().unwrap();
            assert_eq!(state.durable.generation, installed);
            (state.durable.free_blocks, state.grants.blocks, state.files.reservation_paths(node.ino).unwrap().fs_height)
        };
        assert_eq!(replay_height, if create { 2 } else { 1 });
        fs.fsync(&node, false).unwrap();
        let state = fs.foreground_lock().unwrap();
        assert_eq!(state.files.reservation_paths(node.ino).unwrap().fs_height, 2);
        let allocated = before.saturating_sub(state.durable.free_blocks);
        assert!(allocated > 0 && allocated <= retained && retained <= request,
            "create={create} allocated={allocated} retained={retained} request={request}");
        drop(state);
        if !create {
            let mut bytes = [0];
            assert_eq!(fs.read(&node, 0, &mut bytes), Ok(1));
            assert_eq!(bytes, [b'x']);
        }
        println!("FROZEN_SPLIT create={create} request={request} retained={retained} allocated={allocated}");
    }
}

#[test]
fn height_one_create_and_first_write_admit_a_cascading_split() {
    use super::super::reserve::observation;

    let image = incremental_files_view::full_height_one_image();
    for create in [true, false] {
        let fs = mount(Arc::new(Disk::new(image.clone())), true, 630_000 + usize::from(create));
        let (free, space_cap, held) = {
            let mut state = fs.inner.state.lock();
            let paths = state.files.reservation_paths(16).unwrap();
            assert_eq!((paths.fs_height, paths.space_height), (1, 1));
            let cap = 4 * (2 * paths.space_nodes + paths.space_height);
            let free = state.durable.free_blocks;
            // Test exactly the calculated admission headroom, with the
            // unrelated grant still held during prepare and publication.
            let headroom = if create { 44 + cap } else { 125 + cap };
            let held = state.reservations.available(free) - headroom;
            (free, cap, state.reservations.reserve(free, held, Operation::Allocate).unwrap())
        };
        let node = if create {
            let ino = fs.create(2, b"new", NewNode::Regular { attrs: InodeAttrs::new(0o644, 0, 0) }).unwrap();
            assert_eq!(ino, 17);
            fs.open(ino).unwrap()
        } else {
            let opened = fs.open(16).unwrap();
            assert_eq!(fs.write(&opened, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
                clear_setid: false }, b"x").unwrap().bytes, 1);
            opened
        };
        let grant = observation::take_write_grant().unwrap()[3];
        fs.fsync(&node, false).unwrap();
        let mut state = fs.inner.state.lock();
        assert_eq!(state.files.reservation_paths(node.ino).unwrap().fs_height, 2);
        assert!(free - state.durable.free_blocks <= grant);
        println!("HEIGHT_ONE_SPLIT create={create} space_cap={space_cap} grant={grant} allocated={}",
            free - state.durable.free_blocks);
        assert!(held.blocks() > 0);
        state.reservations.release(held);
        drop(state);
        if !create {
            let mut bytes = [0];
            assert_eq!(fs.read(&node, 0, &mut bytes), Ok(1));
            assert_eq!(bytes, [b'x']);
        }
    }
}

// E24 coordinator 630(a5), review 638 C1: atime changes can copy durable
// inode leaves before the next operation asks to fund the open generation.
fn apply_atime_to_durable_leaves(fs: &Fs) {
    for ino in (17..1011).step_by(13) {
        let node = fs.open(ino).unwrap();
        node.pin::<NodePin<YieldWait>>().unwrap().touched
            .record(fs.inner.generation.load(Ordering::Acquire), true, 90_001);
        fs.getattr(ino).unwrap();
        assert_eq!(fs.inner.state.lock().files.stored_inode(ino).unwrap().atime.seconds, 90_000);
    }
}

#[test]
fn review_fixes_create_covers_earlier_inode_copies_and_publishes() {
    let disk = Arc::new(Disk::new(outstanding_grants::populated_image(80_000)));
    let fs = mount(disk.clone(), true, 630_501);
    let dir = fs.lookup(2, b"tmp").unwrap();
    let before = fs.inner.state.lock().durable.generation;
    apply_atime_to_durable_leaves(&fs);
    {
        let state = fs.inner.state.lock();
        assert_eq!(state.files.reservation_paths(dir).unwrap().fs_height, 1);
        assert!(state.files.metadata_debt().unwrap() > 300);
        assert!(state.grants.is_empty());
    }
    let ino = fs.create(dir, b"after-atime", NewNode::Regular {
        attrs: InodeAttrs::new(0o644, 0, 0),
    }).expect("create must fund the generation it settles");
    fs.fsync(&fs.open(ino).unwrap(), false).unwrap();
    {
        let state = fs.inner.state.lock();
        assert!(!state.failed);
        assert_eq!(state.durable.generation, before + 1);
    }
    drop(fs);
    let remounted = mount(disk, false, 630_502);
    assert_eq!(remounted.lookup(dir, b"after-atime"), Ok(ino));
    assert_eq!(remounted.inner.state.lock().files.stored_inode(17).unwrap().atime.seconds, 90_000);
}

#[test]
fn review_fixes_first_write_retains_funds_for_earlier_inode_copies() {
    let fs = mount(Arc::new(Disk::new(outstanding_grants::populated_image(80_000))), true, 630_503);
    let ino = fs.create(2, b"empty", NewNode::Regular { attrs: InodeAttrs::new(0o644, 0, 0) }).unwrap();
    let node = fs.open(ino).unwrap();
    fs.fsync(&node, false).unwrap();
    let baseline = {
        let mut state = fs.inner.state.lock();
        let reservation = state.reserve_data(ino, 0, 1, Operation::Allocate, true).unwrap();
        let blocks = reservation.grant.blocks();
        state.reservations.release(reservation.grant);
        blocks
    };
    apply_atime_to_durable_leaves(&fs);
    let earlier_debt = fs.inner.state.lock().files.metadata_debt().unwrap();
    assert_eq!(fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
        clear_setid: false }, b"x").unwrap().bytes, 1);
    {
        let state = fs.inner.state.lock();
        assert_eq!(state.metadata_reserved, earlier_debt);
        let space_base = state.first_write_space;
        assert!(earlier_debt > space_base);
        assert!(state.grants.blocks >= baseline + earlier_debt - space_base,
            "retained {} blocks, including {earlier_debt} of earlier debt with at most {space_base} shared",
            state.grants.blocks);
    }
    fs.fsync(&node, false).unwrap();
}

#[test]
fn review_fixes_refusal_records_its_own_decision_without_device_io() {
    use super::super::reserve::observation;

    let disk = Arc::new(Disk::new(incremental_files_view::full_height_one_image()));
    let fs = mount(disk.clone(), true, 630_504);
    fs.inner.signal.requested.store(true, Ordering::Release);
    let publisher = start_publisher(&fs);
    let release_generation = publisher.generation;
    let (mut ledger, free, reserve, held) = {
        let mut state = fs.inner.state.lock();
        let free = state.durable.free_blocks;
        let held = state.reservations.available(free);
        let grant = state.reservations.reserve(free, held, Operation::Allocate).unwrap();
        (state.reservations.clone(), free, state.durable.global_reserve, grant)
    };
    let snapshot = Rc::new(RefCell::new(None));
    let outcome = snapshot.clone();
    let reads_between = Rc::new(RefCell::new(None));
    let reads = reads_between.clone();
    let mut publisher = Some(publisher);
    let mut held = Some(held);
    let mut second_grant = None;
    context::observer(Some(Box::new(move |event| match event {
        Event::Phase { op: "create", phase: "reservation", begin: false }
            if outcome.borrow().is_none() => {
                let decision = observation::take().expect("the create was actually denied");
                *outcome.borrow_mut() = Some((decision, disk.reads.load(Ordering::Relaxed)));
                // Simulate frozen prepare-reap acquiring the shared ledger
                // after this create's decision while the mount lock is held.
                second_grant = Some(ledger.reserve(free, 1, Operation::DeleteSnapshot).unwrap());
                assert!(ledger.reserve(free, reserve, Operation::DeleteSnapshot).is_err());
            }
        Event::CommitWait { .. } if publisher.is_some() => {
            let (_, before) = outcome.borrow().unwrap();
            *reads.borrow_mut() = Some(disk.reads.load(Ordering::Relaxed) - before);
            ledger.release(second_grant.take().unwrap());
            ledger.release(held.take().unwrap());
            let publisher = publisher.take().unwrap();
            publisher.paused.install();
            publisher.thread.join().unwrap();
        }
        _ => (),
    })));
    let create = fs.create(2, b"pressure", NewNode::Regular { attrs: InodeAttrs::new(0o644, 0, 0) });
    context::observer(None);
    assert!(create.is_ok(), "{create:?}");
    let ([_, _, charged, request, available], _) = snapshot.borrow().unwrap();
    assert!(request > available);
    let receipts = report_receipts(&fs);
    assert_eq!(receipts.len(), 1);
    let receipt = receipts[0].1;
    assert_eq!((receipt.grants_n, receipt.request, *reads_between.borrow()),
        (charged, request, Some(0)), "the receipt must describe the original denial without I/O");
    assert_eq!(receipt.release_generation, release_generation);
}

#[test]
fn review_fixes_terminal_enospc_records_no_release() {
    let fs = mount(Arc::new(Disk::new(incremental_files_view::full_height_one_image())), true, 630_505);
    let (generation, free, reserve, held) = {
        let mut state = fs.inner.state.lock();
        let sb = &state.durable;
        let (generation, free, reserve) = (sb.generation, sb.free_blocks, sb.global_reserve);
        assert!(state.grants.is_empty());
        let held = state.reservations.reserve(free, free - reserve, Operation::Allocate).unwrap();
        (generation, free, reserve, held)
    };
    assert_eq!(pending_census(&fs), (0, 0));
    assert_eq!(fs.create(2, b"full", NewNode::Regular { attrs: InodeAttrs::new(0o644, 0, 0) }),
        Err(Errno(ENOSPC)));
    let receipts = report_receipts(&fs);
    assert_eq!(receipts.len(), 1, "terminal admission refusal must be observable");
    let receipt = receipts[0].1;
    assert_eq!(receipt, Receipt { op: "create", generation, release_generation: generation,
        free, reserve, pending: 0, pending_oldest: 0, grants_n: held.blocks(), grants_next: 0,
        request: 72 });
    assert_eq!(fs.lookup(2, b"full"), Err(Errno(ENOENT)));
    fs.inner.state.lock().reservations.release(held);
}

#[test]
fn populated_readdir_edit_and_publication_codec_budget() {
    use btree::measurement::{start_edit_cost, take_edit_cost, EditCost};
    use std::rc::Rc;

    const GUEST_FREE: u64 = 25_075;
    // New-view ROOT/SPACE endpoints and the single publication's reservation
    // shadow can revisit fixed roots. The edited paths have no per-edit slack.
    const FIXED_DECODES: u64 = 16;
    let image = outstanding_grants::populated_image(80_000 + GUEST_FREE - X1_FREE);
    let fs = mount(Arc::new(Disk::new(image)), true, 241515646);
    let dir = fs.create(2, b"libc-readdir-many", NewNode::Directory {
        attrs: InodeAttrs::new(0o700, 0, 0),
    }).unwrap();
    let attrs = InodeAttrs::new(0o644, 0, 0);
    let file = |i| alloc::format!("f{i:03}xxxxxxxxx");
    let files: Vec<_> = (0..300).map(|i| {
        fs.create(dir, file(i).as_bytes(), NewNode::Regular { attrs }).unwrap()
    }).collect();
    let root = fs.open(2).unwrap();
    fs.fsync(&root, false).unwrap();
    let state = fs.inner.state.lock();
    let paths = state.files.reservation_paths(dir).unwrap();
    assert_eq!((paths.fs_height, paths.space_height), (1, 1));
    drop(state);
    let parent = fs.getattr(dir).unwrap().attrs;
    let unlink = |i: usize| {
        fs.remove(dir, file(i).as_bytes(), Remove {
            directory: false, expected: files[i], owner: 0, parent,
        }).unwrap();
    };

    type Sample = (&'static str, EditCost, u128, u32, u32);
    let samples = Rc::new(RefCell::new(Vec::<Sample>::new()));
    let observed = samples.clone();
    let mut active = Vec::new();
    let mut points = 0u32;
    context::request();
    context::observer(Some(Box::new(move |event| {
        if let Event::Point = event { points += 1; }
        if let Event::Phase { op, phase, begin } = event {
            let name = match (op, phase) {
                ("create" | "unlink", "inode-edit") => op,
                ("commit", "drain-dirty" | "prepare-reap" | "publication") => phase,
                _ => return,
            };
            if begin {
                start_edit_cost();
                active.push((name, Instant::now(), points, context::yields()));
            } else {
                let (start_name, now, start_points, start_yields) = active.pop().unwrap();
                assert_eq!(start_name, name);
                observed.borrow_mut().push((
                    name, take_edit_cost(), now.elapsed().as_nanos(),
                    points - start_points, context::yields() - start_yields,
                ));
            }
        }
    })));
    fs.create(dir, b"new-entry", NewNode::Regular { attrs }).unwrap();
    unlink(299);
    context::observer(None);

    // One publication drains 10 dirty pages and reclaims 10 orphans, like
    // x8's 100-edit workload. The other operations touch distinct inodes.
    for i in 0..10 {
        let node = fs.open(files[i]).unwrap();
        fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX,
            clear_setid: false }, b"x").unwrap();
        unlink(i + 10);
        for j in 0..8 {
            fs.setattr(files[20 + i * 8 + j], SetAttr {
                mode: Some(0o600), ..SetAttr::default()
            }).unwrap();
        }
    }
    let observed = samples.clone();
    let mut active = Vec::new();
    let mut points = 0u32;
    context::request();
    context::observer(Some(Box::new(move |event| {
        if let Event::Point = event { points += 1; context::request_reschedule(); }
        if let Event::Phase { op: "commit", phase, begin } = event {
            if !matches!(phase, "drain-dirty" | "prepare-reap" | "publication") { return; }
            if begin {
                start_edit_cost();
                active.push((phase, Instant::now(), points, context::yields()));
            } else {
                let (start_phase, now, start_points, start_yields) = active.pop().unwrap();
                assert_eq!(start_phase, phase);
                observed.borrow_mut().push((
                    phase, take_edit_cost(), now.elapsed().as_nanos(),
                    points - start_points, context::yields() - start_yields,
                ));
            }
        }
    })));
    fs.fsync(&root, false).unwrap();
    context::observer(None);

    let samples = samples.borrow();
    assert_eq!(samples.len(), 5);
    let mut failures = Vec::new();
    for (name, cost, ns, points, yields) in samples.iter() {
        let decodes = cost.decoded.values().sum::<u64>();
        let distinct = cost.decoded.len() as u64;
        let encodes = cost.encoded.values().sum::<u64>();
        let encoded = cost.encoded.len() as u64;
        println!("EDIT_COST phase={name} reads={} decodes={decodes}/{distinct} encodes={encodes}/{encoded} copies={} checksum_bytes={} slots={} points={points} yields={yields} ns={ns}",
            cost.reads, cost.copies, cost.checksum_bytes, cost.slots);
        if decodes > distinct + FIXED_DECODES || encodes > encoded {
            failures.push((*name, decodes, distinct, encodes, encoded));
        }
    }
    assert!(failures.is_empty(), "646(b2) codec budget: {failures:?}");
}
