//! E2.5 (design §7 and §8 E2.5, rulings DEV-38(4) and DEV-53): BreenFS waits
//! for the device with no MountLock held, apart from the named I7 exceptions,
//! and only commit workers take the device's publication exclusion.
//!
//! The device below is the I7 guard. Each request checks the MountLock guards
//! its thread holds (counted at the Acquire and Release hooks) and the
//! innermost I7 exception scope. A request under a guard outside a named
//! exception is a violation. A gate can stop chosen threads inside the device,
//! which fixes the interleavings below without timing.
use super::*;
use alloc::{format, string::String, vec};
use std::{
    sync::{mpsc, Condvar, Mutex as HostMutex},
    thread,
    time::{Duration, Instant},
};

const LIMIT: Duration = Duration::from_secs(10);

/// I7's exception list (ruling DEV-53(2)): the tree edits that still read
/// cold nodes under the mount state lock until E5's per-node locks and
/// no-replay copy-on-write close it. This list may only shrink: a name the
/// code no longer uses fails `i7_exception_list_names_exactly_the_sites`, and
/// a scope the list does not name fails it too.
const I7_EXCEPTIONS: &[(&str, &str)] = &[
    ("mutation", "namespace, attribute and snapshot edits and their admission reads"),
    ("apply_touches", "pending touch stamps, applied as an edit after a read pass"),
    ("refresh", "the refresh loops in edited() and materialize()"),
    ("replay", "N+1's changes replayed onto N's view in materialize()"),
];

// Callers share the cache from several threads, so a wait yields and the
// caller rechecks (Wait permits an early return).
struct YieldWait;
impl Wait for YieldWait {
    fn generation(&self) -> u64 {
        0
    }
    fn wait(&self, _: u64) {
        thread::yield_now();
    }
    fn wake_all(&self) {}
}

#[derive(Default)]
struct Gate {
    // Requests on threads with this name stop inside the device until admitted.
    thread: Option<String>,
    reads: bool,
    writes: bool,
    entered: u32,
    tickets: u32,
}

#[derive(Default)]
struct Log {
    violations: Vec<String>,
    exceptions: BTreeSet<&'static str>,
    // Writes and flushes: (thread name, whether it stood in for the worker).
    writers: Vec<(Option<String>, bool)>,
}

struct Disk {
    bytes: HostMutex<Vec<u8>>,
    gate: HostMutex<Gate>,
    turn: Condvar,
    log: HostMutex<Log>,
}

impl Disk {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            bytes: HostMutex::new(vec![0; 64 * 1024 * 1024]),
            gate: HostMutex::default(),
            turn: Condvar::new(),
            log: HostMutex::default(),
        })
    }
    fn observe(&self, kind: &'static str, block: u64) {
        let held = super::super::mount_lock::held_by_current_thread();
        let name = thread::current().name().map(String::from);
        let mut log = self.log.lock().unwrap();
        if kind != "read" {
            log.writers.push((name.clone(), super::i7::is_worker()));
        }
        if held != 0 {
            match super::i7::current() {
                Some(exception) => {
                    log.exceptions.insert(exception);
                }
                None => log.violations.push(format!(
                    "{kind} of block {block} under {held} MountLock guard(s) on {name:?}"
                )),
            }
        }
        drop(log);
        let mut gate = self.gate.lock().unwrap();
        let stops = gate.thread.is_some()
            && gate.thread == name
            && if kind == "read" { gate.reads } else { gate.writes };
        if stops {
            gate.entered += 1;
            self.turn.notify_all();
            while gate.thread == name && gate.tickets == 0 {
                gate = self.turn.wait(gate).unwrap();
            }
            if gate.thread == name {
                gate.tickets -= 1;
            }
        }
    }
    /// Stop `thread`'s reads (and writes, with `writes`) inside the device.
    fn stop(&self, thread: &str, reads: bool, writes: bool) {
        *self.gate.lock().unwrap() = Gate { thread: Some(thread.into()), reads, writes, ..Gate::default() };
    }
    /// Wait until the stopped thread has entered the device `count` times.
    fn entered(&self, count: u32, what: &str) {
        let deadline = Instant::now() + LIMIT;
        let mut gate = self.gate.lock().unwrap();
        while gate.entered < count {
            let now = Instant::now();
            assert!(now < deadline, "{what}");
            gate = self.turn.wait_timeout(gate, deadline - now).unwrap().0;
        }
    }
    fn entries(&self) -> u32 {
        self.gate.lock().unwrap().entered
    }
    /// Let the stopped thread's current request finish.
    fn admit(&self) {
        self.gate.lock().unwrap().tickets += 1;
        self.turn.notify_all();
    }
    fn release(&self) {
        // A failed assertion can poison the gate; release it anyway.
        *self.gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Gate::default();
        self.turn.notify_all();
    }
    fn take_log(&self) -> Log {
        core::mem::take(&mut *self.log.lock().unwrap())
    }
}

impl BlockDevice for Disk {
    fn read_block(&self, block: u64, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(block, 1, bytes)
    }
    fn read_blocks(&self, block: u64, count: usize, bytes: &mut [u8]) -> Result<(), BlockError> {
        self.observe("read", block);
        let start = block as usize * BLOCK_SIZE;
        let length = count * BLOCK_SIZE;
        bytes[..length].copy_from_slice(&self.bytes.lock().unwrap()[start..start + length]);
        Ok(())
    }
    fn write_block(&self, block: u64, bytes: &[u8]) -> Result<(), BlockError> {
        self.observe("write", block);
        let start = block as usize * BLOCK_SIZE;
        self.bytes.lock().unwrap()[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }
    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }
    fn num_blocks(&self) -> u64 {
        16384
    }
    fn flush(&self) -> Result<(), BlockError> {
        self.observe("flush", 0);
        Ok(())
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        self.release();
    }
}

/// Run `run` on a thread named `name`; fail if it has not finished in LIMIT,
/// as a caller does that waits for a mount lock held across a device wait.
fn bounded<T: Send + 'static>(name: &str, run: impl FnOnce() -> T + Send + 'static) -> T {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _ = sender.send(run());
        })
        .unwrap();
    receiver
        .recv_timeout(LIMIT)
        .unwrap_or_else(|_| panic!("{name} did not finish within {LIMIT:?}"))
}

fn spawn<T: Send + 'static>(name: &str, run: impl FnOnce() -> T + Send + 'static) -> thread::JoinHandle<T> {
    thread::Builder::new().name(name.into()).spawn(run).unwrap()
}

fn join<T>(handle: thread::JoinHandle<T>, what: &str) -> T {
    let deadline = Instant::now() + LIMIT;
    while !handle.is_finished() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(1));
    }
    handle.join().unwrap()
}

type Fs = BreenFs<YieldWait>;

fn regular() -> NewNode<'static> {
    NewNode::Regular { attrs: InodeAttrs::new(0o100600, 0, 0) }
}

fn at(offset: u64) -> WriteReq {
    WriteReq { at: WriteAt::Offset(offset), limit: u64::MAX, clear_setid: false }
}

fn pattern(seed: u8, length: usize) -> Vec<u8> {
    (0..length).map(|i| seed.wrapping_add((i % 251) as u8)).collect()
}

fn mount(disk: &Arc<Disk>, id: usize) -> Arc<Fs> {
    let fs = Fs::mount_writable(disk.clone(), Arc::new(BufferCache::new(YieldWait)), id, 1).unwrap();
    fs.init_service().unwrap();
    fs
}

/// A committed volume: files `a`, `b` and `f` (three blocks each, seeds 1, 2
/// and 3), a directory `d` holding 600 names, a symlink `s` and an xattr on
/// `a`. Unmounted, so the next mount reads everything cold.
#[derive(Clone, Copy)]
struct Inos {
    a: u64,
    b: u64,
    f: u64,
    d: u64,
    s: u64,
}

fn image() -> (Arc<Disk>, Inos) {
    let disk = Disk::new();
    let sb = Superblock {
        fs_uuid: [0x75; 16],
        generation: 1,
        slot: 1,
        total_blocks: 16384,
        next_root_id: 2,
        ..Superblock::default()
    };
    let mut files = Files::format(super::super::commit::DeviceStore(disk.as_ref()), &sb).unwrap();
    files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    let fs = mount(&disk, 120);
    let mut made = BTreeMap::new();
    for (name, seed) in [("a", 1u8), ("b", 2), ("f", 3)] {
        let ino = fs.create(2, name.as_bytes(), regular()).unwrap();
        let node = fs.open(ino).unwrap();
        assert_eq!(fs.write(&node, at(0), &pattern(seed, 3 * BLOCK_SIZE)).map(|w| w.bytes), Ok(3 * BLOCK_SIZE));
        made.insert(name, ino);
    }
    let dir = fs.create(2, b"d", NewNode::Directory { attrs: InodeAttrs::new(0o040755, 0, 0) }).unwrap();
    for i in 0..600 {
        fs.create(dir, format!("entry-{i:04}").as_bytes(), regular()).unwrap();
    }
    made.insert("d", dir);
    made.insert("s", fs.create(2, b"s", NewNode::Symlink { attrs: InodeAttrs::new(0o120777, 0, 0), target: b"a" }).unwrap());
    let inos = Inos { a: made["a"], b: made["b"], f: made["f"], d: made["d"], s: made["s"] };
    fs.xattr(inos.a, Xattr::Set { name: b"user.k", value: b"v", mode: XattrSet::Upsert }).unwrap();
    fs.sync().unwrap();
    fs.shutdown("e2-5-image", u64::MAX).unwrap();
    disk.take_log();
    (disk, inos)
}

fn read_all(fs: &Fs, ino: u64, length: usize) -> Result<Vec<u8>, Errno> {
    let node = fs.open(ino)?;
    let mut bytes = vec![0; length];
    let read = fs.read(&node, 0, &mut bytes)?;
    bytes.truncate(read);
    Ok(bytes)
}

fn names(fs: &Fs, dir: u64) -> Result<Vec<(Vec<u8>, u64)>, Errno> {
    let mut out = Vec::new();
    fs.readdir(dir, 0, &mut |entry, cookie| {
        out.push((entry.name, cookie));
        Ok(true)
    })?;
    Ok(out)
}

fn assert_guarded(log: &Log, what: &str) {
    assert!(
        log.violations.is_empty(),
        "{what}: device requests under a MountLock outside the I7 exceptions:\n  {}",
        log.violations.join("\n  ")
    );
    for name in &log.exceptions {
        assert!(I7_EXCEPTIONS.iter().any(|(listed, _)| listed == name), "{what}: unlisted I7 exception {name}");
    }
}

fn assert_worker_publishes(log: &Log, what: &str) {
    let foreground: Vec<_> = log.writers.iter().filter(|(_, worker)| !*worker).collect();
    assert!(foreground.is_empty(), "{what}: device writes outside the commit worker: {foreground:?}");
    let publishers: Vec<_> = super::i7::take_publishers().into_iter().filter(|(_, worker)| !*worker).collect();
    assert!(publishers.is_empty(), "{what}: foreground threads took publication exclusion: {publishers:?}");
}

/// The block that backs `ino`'s file block `index` in the open view.
fn phys(fs: &Fs, ino: u64, index: u64) -> u64 {
    let state = fs.inner.state.lock();
    let extents = state.files.extents(ino).unwrap();
    extents.iter().find_map(|(key, extent)| {
        let offset = index * BLOCK_SIZE as u64;
        let blocks = u64::from(extent.num_blocks);
        (key.offset <= offset && offset < key.offset + blocks * BLOCK_SIZE as u64)
            .then(|| extent.phys_start + u64::from(extent.extent_offset) + (offset - key.offset) / BLOCK_SIZE as u64)
    }).unwrap()
}

/// Whether `block` lies in a FREE range of the device's durable space tree.
fn durable_free(disk: &Arc<Disk>, block: u64) -> bool {
    let fs = Fs::mount(disk.clone(), Arc::new(BufferCache::new(YieldWait)), 199, 1).unwrap();
    let sb = fs.inner.state.lock().durable.clone();
    let store = super::super::commit::DeviceStore(disk.as_ref());
    let items = recover::space_items(&store, &sb).unwrap();
    items.iter().any(|(key, bytes)| {
        key.item_type == format::FREE
            && format::Free::decode(bytes).is_ok_and(|free| key.offset <= block && block < key.offset + free.length)
    })
}

#[test]
fn i7_guard_read_paths_wait_with_no_mount_lock() {
    let (disk, inos) = image();
    let fs = mount(&disk, 121);
    disk.take_log();
    let _ = super::i7::take_publishers();
    let worker = fs.clone();
    bounded("i7-reads", move || {
        let fs = worker;
        fs.getattr(inos.a).unwrap();
        assert_eq!(fs.lookup(2, b"f"), Ok(inos.f));
        assert_eq!(fs.lookup(inos.d, b"entry-0599").map(|ino| ino != 0), Ok(true));
        assert_eq!(names(&fs, inos.d).unwrap().len(), 600);
        assert_eq!(read_all(&fs, inos.b, 3 * BLOCK_SIZE).unwrap(), pattern(2, 3 * BLOCK_SIZE));
        assert_eq!(fs.readlink(inos.s).unwrap(), b"a");
        assert_eq!(fs.parent(inos.d), Ok(2));
        let mut value = [0; 4];
        assert_eq!(fs.xattr(inos.a, Xattr::Get { name: b"user.k", value: &mut value }), Ok(1));
    });
    assert_guarded(&disk.take_log(), "read paths");
}

#[test]
fn i7_guard_mutations_stay_inside_the_exception_list() {
    let (disk, inos) = image();
    let fs = mount(&disk, 122);
    disk.take_log();
    let _ = super::i7::take_publishers();
    let worker = fs.clone();
    bounded("i7-mutations", move || {
        let fs = worker;
        let dir = fs.create(2, b"new-dir", NewNode::Directory { attrs: InodeAttrs::new(0o040755, 0, 0) }).unwrap();
        let file = fs.create(dir, b"new-file", regular()).unwrap();
        fs.create(dir, b"new-link", NewNode::Symlink { attrs: InodeAttrs::new(0o120777, 0, 0), target: b"x" }).unwrap();
        fs.link(inos.a, dir, b"hard", &|_, _| Ok(())).unwrap();
        fs.rename(dir, b"new-file", 2, b"moved", RenameMode::Replace, &|_, _, _, _| Ok(())).unwrap();
        fs.setattr(inos.b, SetAttr { mode: Some(0o100644), ..SetAttr::default() }).unwrap();
        fs.xattr(inos.b, Xattr::Set { name: b"user.n", value: b"w", mode: XattrSet::Upsert }).unwrap();
        let parent = fs.getattr(2).unwrap().attrs;
        fs.remove(2, b"moved", Remove { directory: false, expected: file, owner: 0, parent }).unwrap();
        fs.sync().unwrap();
    });
    let log = disk.take_log();
    assert_guarded(&log, "mutations");
    assert_worker_publishes(&log, "mutations");
}

#[test]
fn a_cached_stat_completes_while_a_reader_waits_on_the_device() {
    let (disk, inos) = image();
    let fs = mount(&disk, 123);
    // Reader B's inode is cached; reader A's data is cold.
    fs.getattr(inos.b).unwrap();
    disk.stop("reader-a", true, false);
    let a = {
        let fs = fs.clone();
        spawn("reader-a", move || read_all(&fs, inos.a, 3 * BLOCK_SIZE))
    };
    disk.entered(1, "reader A never reached the device");
    let b = {
        let fs = fs.clone();
        bounded("reader-b", move || fs.getattr(inos.b).map(|attrs| attrs.size))
    };
    assert_eq!(b, Ok(3 * BLOCK_SIZE as u64));
    assert!(!a.is_finished(), "reader A finished while its request was stopped in the device");
    disk.release();
    assert_eq!(join(a, "reader A did not finish"), Ok(pattern(1, 3 * BLOCK_SIZE)));
}

#[test]
fn a_reader_that_waited_across_a_commit_answers_from_the_new_generation() {
    let (disk, inos) = image();
    let fs = mount(&disk, 124);
    disk.stop("reader-a", true, false);
    let a = {
        let fs = fs.clone();
        spawn("reader-a", move || read_all(&fs, inos.f, 3 * BLOCK_SIZE))
    };
    disk.entered(1, "reader A never reached the device");
    // G+1 changes the middle block while A's request is out.
    let writer = fs.clone();
    bounded("writer", move || {
        let node = writer.open(inos.f).unwrap();
        assert_eq!(writer.write(&node, at(BLOCK_SIZE as u64), &pattern(9, BLOCK_SIZE)).map(|w| w.bytes), Ok(BLOCK_SIZE));
        writer.fsync(&node, false).unwrap();
    });
    disk.release();
    let mut expected = pattern(3, 3 * BLOCK_SIZE);
    expected[BLOCK_SIZE..2 * BLOCK_SIZE].copy_from_slice(&pattern(9, BLOCK_SIZE));
    let read = join(a, "reader A did not finish").unwrap();
    assert!(read == expected, "reader A mixed generation G's bytes into its answer from G+1");
}

#[test]
fn a_publication_hold_lets_a_reader_finish_while_commits_keep_arriving() {
    let (disk, inos) = image();
    let fs = mount(&disk, 125);
    disk.stop("reader-a", true, false);
    let a = {
        let fs = fs.clone();
        spawn("reader-a", move || read_all(&fs, inos.f, BLOCK_SIZE))
    };
    // Each time A is in the device, publish a generation that moves the
    // block it is reading, so its next pass needs the device again.
    let mut refused = 0;
    let mut round = 0;
    while !a.is_finished() {
        round += 1;
        assert!(round <= 32, "reader A did not finish while commits kept arriving");
        let deadline = Instant::now() + LIMIT;
        while disk.entries() < round && !a.is_finished() {
            assert!(Instant::now() < deadline, "reader A never reached the device");
            thread::sleep(Duration::from_millis(1));
        }
        if a.is_finished() {
            break;
        }
        let writer = fs.clone();
        let published = bounded("writer", move || {
            let node = writer.open(inos.f).unwrap();
            writer.write(&node, at(0), &pattern(20 + round as u8, BLOCK_SIZE)).unwrap();
            writer.inner.signal.requested.store(true, Ordering::Release);
            measurement::service(&writer).unwrap()
        });
        refused += u32::from(!published);
        disk.admit();
    }
    disk.release();
    let read = join(a, "reader A did not finish").unwrap();
    assert_eq!(read.len(), BLOCK_SIZE);
    assert!(refused != 0, "no publication waited for the reader's hold");
}

/// A getdents64-shaped reply: records appended until `budget` of them fit.
fn reply(fs: &Fs, dir: u64, budget: usize) -> Result<Vec<(Vec<u8>, u64)>, Errno> {
    let mut out = Vec::new();
    fs.readdir(dir, 0, &mut |entry, cookie| {
        if out.len() == budget {
            return Ok(false);
        }
        out.push((entry.name, cookie));
        Ok(true)
    })?;
    Ok(out)
}

/// Design §8 E2.5, a late miss in readdir: the first leaf is cached and a
/// later one cold, so a pass emits entries before it needs the device, and a
/// commit adds an entry while the read is out. The reply has no duplicate
/// names or cookies and matches one generation's listing.
#[test]
fn a_directory_reply_comes_from_one_pass_of_one_generation() {
    let (disk, inos) = image();
    let fs = mount(&disk, 134);
    // Warm the first leaf only: a one-entry walk of the open view.
    assert_eq!(fs.inner.state.lock().files.directory_entries(inos.d, 0, 1).unwrap().len(), 1);
    disk.stop("reader-a", true, false);
    let a = {
        let fs = fs.clone();
        spawn("reader-a", move || reply(&fs, inos.d, 500))
    };
    disk.entered(1, "reader A never reached the device");
    let writer = fs.clone();
    bounded("writer", move || {
        writer.create(inos.d, b"added-late", regular()).unwrap();
        writer.fsync(&writer.open(inos.d).unwrap(), false).unwrap();
    });
    disk.release();
    let got = join(a, "reader A did not finish").unwrap();
    let names: BTreeSet<_> = got.iter().map(|(name, _)| name.clone()).collect();
    let cookies: BTreeSet<_> = got.iter().map(|(_, cookie)| *cookie).collect();
    assert_eq!(names.len(), got.len(), "the reply repeats names");
    assert_eq!(cookies.len(), got.len(), "the reply repeats cookies");
    assert_eq!(got, reply(&fs, inos.d, 500).unwrap(), "the reply is not one generation's listing");
}

fn reuse_past_the_horizon(read_only: bool) {
    let (disk, inos) = image();
    let fs = mount(&disk, 126);
    let reader = if read_only {
        Fs::mount(disk.clone(), Arc::new(BufferCache::new(YieldWait)), 127, 1).unwrap()
    } else {
        fs.clone()
    };
    // Warm block 0's path, so A's first device request is block 1's data.
    let node = reader.open(inos.f).unwrap();
    let mut first = vec![0; BLOCK_SIZE];
    assert_eq!(reader.read(&node, 0, &mut first), Ok(BLOCK_SIZE));
    let old = phys(&fs, inos.f, 1);
    disk.stop("reader-a", true, false);
    let a = {
        let reader = reader.clone();
        spawn("reader-a", move || {
            let node = reader.open(inos.f).unwrap();
            let mut bytes = vec![0; BLOCK_SIZE];
            reader.read(&node, BLOCK_SIZE as u64, &mut bytes).map(|_| bytes)
        })
    };
    disk.entered(1, "reader A never reached the device");
    // P+1 releases A's block; P+2 to P+4 publish and allocate.
    let writer = fs.clone();
    bounded("writer", move || {
        let f = writer.open(inos.f).unwrap();
        let other = writer.open(inos.b).unwrap();
        writer.write(&f, at(BLOCK_SIZE as u64), &pattern(40, BLOCK_SIZE)).unwrap();
        writer.fsync(&f, false).unwrap();
        for round in 0..3u8 {
            writer.write(&other, at(0), &pattern(50 + round, 3 * BLOCK_SIZE)).unwrap();
            writer.fsync(&other, false).unwrap();
        }
    });
    assert!(!durable_free(&disk, old), "block {old} was freed while generation P was pinned");
    disk.release();
    let read = join(a, "reader A did not finish");
    assert!(read.is_ok(), "reader A: {:?}", read.err());
}

#[test]
fn a_pinned_generation_keeps_its_released_blocks_pending() {
    reuse_past_the_horizon(false);
}

#[test]
fn a_read_only_mounts_pin_keeps_the_writers_released_blocks_pending() {
    reuse_past_the_horizon(true);
}

#[test]
fn a_writer_at_the_full_budget_parks_on_commit_progress() {
    let (disk, inos) = image();
    let fs = mount(&disk, 128);
    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let (fs, stop) = (fs.clone(), stop.clone());
        spawn("commit-worker", move || measurement::as_worker(&fs, || {
            while !stop.load(Ordering::Acquire) {
                let _ = fs.commit_due(monotonic_ns());
                thread::sleep(Duration::from_millis(1));
            }
        }))
    };
    disk.take_log();
    let _ = super::i7::take_publishers();
    let writer = fs.clone();
    bounded("writer", move || {
        let node = writer.open(inos.f).unwrap();
        let chunk = pattern(60, 1 << 20);
        for i in 0..12u64 {
            assert_eq!(writer.write(&node, at(i << 20), &chunk).map(|w| w.bytes), Ok(1 << 20));
        }
    });
    stop.store(true, Ordering::Release);
    join(worker, "the commit worker did not stop");
    let log = disk.take_log();
    assert_guarded(&log, "full-budget writes");
    assert_worker_publishes(&log, "full-budget writes");
}

/// The commit worker reaches the full budget while it stages mapped dirty
/// pages. With `other_publisher`, another publisher owns the detached
/// generation and is stopped inside its writes. The check fails if the worker
/// parks on commit progress or if a later fsync does not complete.
fn worker_at_the_full_budget(other_publisher: bool) {
    let (disk, inos) = image();
    let fs = mount(&disk, 132 + usize::from(other_publisher));
    let mapped = fs.open(inos.a).unwrap();
    fs.resize(&mapped, 64 * BLOCK_SIZE as u64, false, &|_| Ok(())).unwrap();
    fs.fsync(&mapped, false).unwrap();
    fs.mapping_io(&mapped).unwrap().publish(&mut |_| Ok(())).unwrap();
    // Another file's dirty pages fill the budget to just short of the whole.
    let other = fs.open(inos.b).unwrap();
    let fill = pattern(80, DIRTY_BUDGET as usize - 32 * BLOCK_SIZE);
    assert_eq!(fs.write(&other, at(0), &fill).map(|w| w.bytes), Ok(fill.len()));
    let publisher = other_publisher.then(|| {
        disk.stop("publisher", false, true);
        let fs = fs.clone();
        let handle = spawn("publisher", move || measurement::as_worker(&fs, || {
            fs.inner.signal.requested.store(true, Ordering::Release);
            fs.commit_generation(monotonic_ns())
        }));
        disk.entered(1, "the other publisher never reached its writes");
        handle
    });
    let stores = pattern(90, 64 * BLOCK_SIZE);
    mapped.map.shared_stores(&stores, true);
    mapped.map.request_writeback();
    let pass = {
        let fs = fs.clone();
        bounded("commit-worker", move || measurement::as_worker(&fs, || {
            let parked = Arc::new(AtomicBool::new(false));
            let seen = parked.clone();
            super::super::preempt::context::observer(Some(alloc::boxed::Box::new(move |event| {
                if matches!(event, super::super::preempt::context::Event::CommitWait { .. }) {
                    seen.store(true, Ordering::SeqCst);
                }
            })));
            fs.inner.signal.mappings.store(true, Ordering::Release);
            let result = fs.commit_due(monotonic_ns());
            super::super::preempt::context::observer(None);
            (result, parked.load(Ordering::SeqCst))
        }))
    };
    assert!(pass.0.is_ok(), "the worker's pass failed: {:?}", pass.0);
    assert!(!pass.1, "the commit worker parked on commit progress");
    if let Some(handle) = publisher {
        disk.release();
        assert_eq!(join(handle, "the other publisher did not finish"), Ok(true));
    }
    let fsync = {
        let (fs, mapped) = (fs.clone(), mapped.clone());
        bounded("fsync", move || fs.fsync(&mapped, false))
    };
    assert_eq!(fsync, Ok(()));
    let observer = Fs::mount(disk.clone(), Arc::new(BufferCache::new(YieldWait)), 140, 1).unwrap();
    assert!(read_all(&observer, inos.a, 64 * BLOCK_SIZE).unwrap() == stores, "the mapped stores did not reach the device");
}

#[test]
fn the_commit_worker_at_the_full_budget_publishes_what_it_holds() {
    worker_at_the_full_budget(false);
}

#[test]
fn the_commit_worker_at_the_full_budget_never_waits_for_another_publisher() {
    worker_at_the_full_budget(true);
}

#[test]
fn a_partial_page_edit_is_redone_after_a_commit_moves_its_page() {
    let (disk, inos) = image();
    let fs = mount(&disk, 129);
    disk.take_log();
    disk.stop("writer-a", true, false);
    let a = {
        let fs = fs.clone();
        spawn("writer-a", move || {
            let node = fs.open(inos.f)?;
            fs.write(&node, at(10), &[0xee; 100]).map(|w| w.bytes).map_err(|error| error.errno())
        })
    };
    disk.entered(1, "writer A never reached the device");
    // A commit lands between A's staging read and its retake of the lock.
    let writer = fs.clone();
    bounded("writer-b", move || {
        let node = writer.open(inos.f).unwrap();
        writer.write(&node, at(0), &pattern(70, BLOCK_SIZE)).unwrap();
        writer.fsync(&node, false).unwrap();
    });
    disk.release();
    assert_eq!(join(a, "writer A did not finish"), Ok(100));
    let mut expected = pattern(70, BLOCK_SIZE);
    expected[10..110].fill(0xee);
    expected.extend_from_slice(&pattern(3, 3 * BLOCK_SIZE)[BLOCK_SIZE..]);
    assert!(read_all(&fs, inos.f, 3 * BLOCK_SIZE).unwrap() == expected, "the edit kept generation G's page");
    assert_guarded(&disk.take_log(), "partial-page write");
}

#[test]
fn table_7_3a_paths_wait_with_no_mount_lock() {
    let (disk, inos) = image();
    let fs = mount(&disk, 130);
    disk.take_log();
    let _ = super::i7::take_publishers();
    let worker = fs.clone();
    let disk_for = disk.clone();
    bounded("i7-table", move || {
        let fs = worker;
        let node = fs.open(inos.a).unwrap();
        // A cold partial-page write and a truncate to the middle of a page.
        fs.write(&node, at(5), &[1; 7]).unwrap();
        let tail = fs.open(inos.b).unwrap();
        fs.resize(&tail, BLOCK_SIZE as u64 + 100, false, &|_| Ok(())).unwrap();
        fs.sync().unwrap();
        // The durable-root check and snapshot status.
        fs.check_durable_roots(None).unwrap();
        fs.snapshot_status().unwrap();
        // A scrub pass.
        fs.start_scrub().unwrap();
        while measurement::scrub_step(&fs, super::super::format::NODE_SIZE).unwrap() {}
        // A read-only snapshot read, then the snapshot's deletion.
        let mut request = SnapshotRequest { flags: SNAPSHOT_READ_ONLY, name_len: 4, name: [0; 256] };
        request.name[..4].copy_from_slice(b"snap");
        fs.snapshot_request(&request, true).unwrap();
        let snapshot = Fs::mount_named(disk_for.clone(), Arc::new(BufferCache::new(YieldWait)), 131, Some(b"root=snap"), false).unwrap();
        assert_eq!(read_all(&snapshot, inos.f, 3 * BLOCK_SIZE).unwrap(), pattern(3, 3 * BLOCK_SIZE));
        drop(snapshot);
        request.flags = 0;
        fs.snapshot_request(&request, false).unwrap();
    });
    let log = disk.take_log();
    assert_guarded(&log, "the §7.3a paths");
    assert_worker_publishes(&log, "the §7.3a paths");
}

/// Fails when an `i7::exception("...")` site in vfs.rs names an exception the
/// list does not, or when a listed exception has no site left.
#[test]
fn i7_exception_list_names_exactly_the_sites() {
    let source = include_str!("../../../kernel/src/fs/breenfs/vfs.rs");
    let mut sites = BTreeSet::new();
    for (index, _) in source.match_indices("i7::exception(\"") {
        let rest = &source[index + "i7::exception(\"".len()..];
        sites.insert(&rest[..rest.find('"').unwrap()]);
    }
    let listed: BTreeSet<&str> = I7_EXCEPTIONS.iter().map(|(name, _)| *name).collect();
    let unlisted: Vec<_> = sites.difference(&listed).collect();
    assert!(unlisted.is_empty(), "I7 exception scopes the list does not name: {unlisted:?}");
    let stale: Vec<_> = listed.difference(&sites).collect();
    assert!(stale.is_empty(), "I7 exceptions that match no site (remove them): {stale:?}");
}
