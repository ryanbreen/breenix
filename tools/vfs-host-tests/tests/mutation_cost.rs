//! E24 ledger 531(u), amended by coordinator rulings 541 and 544: production VFS cost oracles.
use crate::{attrs::InodeAttrs, block::cache::BufferCache,
    breenfs::{btree::measurement as tree, vfs::{BreenFs, measurement as commits}},
    read_conformance::{Disk, NoWait}, vfs::*};
use std::{sync::{Arc, Mutex}, time::Instant};

#[derive(Clone, Copy, Debug, Default)]
struct Counts { work: [u64; 8], commits: [u64; 3], decodes: [u64; 4], refresh_calls: u64,
    refresh_peaks: [Option<u64>; 2], directory_calls: u64, directory_visits: u64 }
impl Counts {
    fn take() -> Self { Self {work: tree::take_mutations(), commits: commits::take_work(), decodes: tree::take_work(), refresh_calls: tree::take_refresh_calls(), ..Self::default()} }
    fn add(&mut self, other: Self) {
        self.refresh_calls += other.refresh_calls;
        self.directory_calls += other.directory_calls;
        self.directory_visits += other.directory_visits;
        for (peak, value) in self.refresh_peaks.iter_mut().zip(other.refresh_peaks) {
            *peak = (*peak).into_iter().chain(value).max();
        }
        for (a,b) in self.work.iter_mut().zip(other.work) { *a += b; }
        for (a,b) in self.commits.iter_mut().zip(other.commits) { *a += b; }
        for (a,b) in self.decodes.iter_mut().zip(other.decodes) { *a += b; }
    }
}
fn mount() -> Arc<BreenFs<NoWait>> {
    let disk = Arc::new(Disk {bytes: Mutex::new(crate::model::image()), writable: true});
    BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 24151521, 1).unwrap()
}
fn make(fs: &dyn FileSystem, dir: u64, name: &str, directory: bool) -> u64 {
    let attrs = InodeAttrs::new(0o755, 0, 0);
    fs.create(dir, name.as_bytes(), if directory {NewNode::Directory {attrs}} else {NewNode::Regular {attrs}}).unwrap()
}
fn write(fs: &dyn FileSystem, node: &VNode, offset: u64, bytes: &[u8]) {
    assert_eq!(fs.write(node, WriteReq {at: WriteAt::Offset(offset), limit: u64::MAX, clear_setid: false}, bytes).unwrap().bytes, bytes.len());
}
fn file(fs: &dyn FileSystem, dir: u64, name: &str, bytes: &[u8]) -> Arc<VNode> {
    let node = fs.open(make(fs, dir, name, false)).unwrap();
    write(fs, &node, 0, bytes); node
}
// btree::change: per level, change loads the path node and validates its
// child; rebalance loads that child and at most two siblings (five reads).
// Root collapse adds at most one load per level plus the initial root load.
// pack saves at most two nodes; rebalance saves at most two (four writes).
// Thus ten touches per level, with one extra level for root transitions.
// Count each load/save, including buffered repeats, not decoded item counts.
const BTREE_TOUCHES_PER_LEVEL: usize = 10;
fn operation(label: &str, run: impl FnOnce(), total: &mut Counts, ok: &mut bool) {
    Counts::take(); tree::take_changes(); commits::take_events(); run(); let mut c = Counts::take();
    let events = commits::take_events();
    let mut refresh_calls = 0;
    let mut refresh_visits = 0;
    let mut admissions = 0;
    let mut attempts_count = 0;
    let mut pressure_publications = 0;
    for event in events {
        match event {
            commits::Event::Refresh { objects, visited } => {
                let given: std::collections::BTreeSet<_> = objects.into_iter().collect();
                let seen: std::collections::BTreeSet<_> = visited.iter().copied().collect();
                let visits = visited.len() as u64;
                // 544(1): check each edited() call, including duplicate-ID and
                // foreign-ID visits, rather than a mixed-operation average.
                let indexed = given.len() <= 4 && seen.len() == visited.len()
                    && seen.is_subset(&given) && visited.len() <= given.len();
                if !indexed { println!("REFRESH_BOUND {label} given={given:?} visited={visited:?}"); }
                *ok &= indexed;
                refresh_calls += 1;
                refresh_visits += visits;
                if let Some(kind) = if label.starts_with("unlink/") { Some(0) }
                    else if label.starts_with("rmdir/") { Some(1) } else { None } {
                    c.refresh_peaks[kind] = Some(c.refresh_peaks[kind].unwrap_or(0).max(visits));
                    if kind == 1 { c.directory_calls += 1; c.directory_visits += visits; }
                }
            }
            commits::Event::Admission { attempts, pressure } => {
                admissions += 1;
                attempts_count += attempts.len() as u64;
                let ordinary = pressure.is_empty() && attempts.len() == 1
                    && attempts[0].error.is_none() && attempts[0].refusal.is_none();
                // 544(2): a pressure publication must be bracketed by an
                // actual refused grant and a successful re-admission.
                let pressure_admitted = pressure == [true] && attempts.len() == 2
                    && attempts[0].error == Some(crate::syscall_errno::ENOSPC)
                    && attempts[0].refusal.is_some_and(|[free, _, outstanding, blocks, _]| {
                        blocks > free.saturating_sub(outstanding)
                    })
                    && attempts[1].error.is_none() && attempts[1].refusal.is_none();
                if !ordinary { println!("ADMISSION {label} attempts={attempts:?} pressure={pressure:?}"); }
                *ok &= ordinary || pressure_admitted;
                pressure_publications += u64::from(pressure_admitted);
            }
        }
    }
    let attributed = c.commits == [pressure_publications, 0, pressure_publications]
        && c.work[0] == attempts_count && attempts_count == admissions + pressure_publications;
    if !attributed { println!("PUBLICATION_ATTRIBUTION {label} commits={:?} reservations={} attempts={attempts_count} admissions={admissions} pressure={pressure_publications}", c.commits, c.work[0]); }
    *ok &= attributed && refresh_calls == c.refresh_calls && refresh_visits == c.work[7];
    // The fixtures create directories without open(): their removals have no
    // NodeState to refresh. Assert this separately from the peak comparison.
    if label.starts_with("rmdir/") {
        *ok &= c.directory_calls > 0 && c.directory_visits == 0;
    }
    let changes = tree::take_changes();
    for (i, change) in changes.iter().enumerate() {
        let bound = BTREE_TOUCHES_PER_LEVEL * (usize::from(change.height) + 1);
        if change.reads + change.writes > bound {
            println!("TREE_BOUND {label} item={i} {change:?} bound={bound}");
            *ok = false;
        }
    }
    println!("TREE_OPERATIONS {label} count={} max_touches={} heights={:?}", changes.len(),
        changes.iter().map(|c| c.reads + c.writes).max().unwrap_or(0),
        changes.iter().map(|c| c.height).collect::<std::collections::BTreeSet<_>>());
    println!("MUTATION {label} {c:?}");
    total.add(c);
}
fn remove_tree(fs: &dyn FileSystem, parent: u64, name: &[u8], total: &mut Counts, ok: &mut bool) {
    let ino = fs.lookup(parent, name).unwrap();
    let attrs = fs.getattr(ino).unwrap(); // Scratch::drop's symlink_metadata
    let directory = attrs.kind == NodeKind::Directory;
    if directory {
        let mut entries = Vec::new();
        fs.readdir(ino, 0, &mut |entry,_| {entries.push(entry.name); Ok(true)}).unwrap();
        for entry in entries { remove_tree(fs, ino, &entry, total, ok); }
    }
    operation(&format!("{}/{}", if directory {"rmdir"} else {"unlink"}, String::from_utf8_lossy(name)), || {
        fs.remove(parent, name, Remove {directory, expected: ino, owner: attrs.attrs.uid,
            parent: fs.getattr(parent).unwrap().attrs}).unwrap();
    }, total, ok);
}
fn fixture(fs: &dyn FileSystem, parent: u64, n: usize) {
    // fixtures::prepare: six directories, six etc files, two fixture files and
    // one clock page. Repeat whole config roots to scale entries, not depth.
    for i in 0..n {
        let root = make(fs, parent, &format!("session-{i}"), true);
        let etc = make(fs, root, "etc", true);
        make(fs, root, "home", true);
        let var = make(fs, root, "var", true); make(fs, var, "log", true);
        let fixtures = make(fs, root, "fixtures", true);
        for name in ["fonts.conf", "locale.conf", "hotkeys.conf", "bwm.conf", "uitest-cursor.conf", "app.conf"] {
            drop(file(fs, etc, name, b"fixture\n"));
        }
        for name in ["names.txt", "sample.txt"] { drop(file(fs, fixtures, name, b"fixture\n")); }
        drop(file(fs, root, "ui-clock", &[0;4096]));
    }
}
fn cleanup(ui: bool, n: usize, ok: &mut bool) -> Counts {
    let fs = mount();
    let root = make(fs.as_ref(), 2, "scratch", true);
    if ui { fixture(fs.as_ref(), root, n); }
    else { for i in 0..n { drop(file(fs.as_ref(), root, &format!("f{i:03}xxxxxxxxx"), b"x")); } }
    // No setup fsync: the real programs close their files without publishing.
    let mut total = Counts::default();
    remove_tree(fs.as_ref(), 2, b"scratch", &mut total, ok);
    println!("TOTAL ui={ui} entries={n} {total:?}");
    total
}
fn marker(fs: &dyn FileSystem, node: &VNode) -> u64 {
    for byte in 0..3 {
        fs.resize(node, 0, false, &|_| Ok(())).unwrap(); write(fs, node, 0, &[byte]);
        fs.fsync(node, false).unwrap();
        println!("MARKER byte={byte} free={} {:?}", fs.statfs().unwrap().free_blocks, Counts::take());
    }
    fs.statfs().unwrap().free_blocks
}
fn truncate(open: bool, ok: &mut bool) {
    let fs = mount();
    let node = file(fs.as_ref(), 2, "large", &vec![0x5a;129*4096]);
    fs.fsync(&node, false).unwrap();
    let mark = file(fs.as_ref(), 2, "marker", b"");
    let before = marker(fs.as_ref(), &mark);
    let opened = fs.open(node.ino).unwrap();
    let size = if open {0} else {3};
    let mut total = Counts::default();
    operation(if open {"ftruncate-large-open"} else {"ftruncate-large"}, || {
        fs.resize(&opened, size, false, &|_| Ok(())).unwrap();
    }, &mut total, ok);
    let after = marker(fs.as_ref(), &mark);
    let blocks = if open {129} else {128};
    println!("TRUNCATE open={open} FREE {before}->{after} expected={blocks}");
    *ok &= after >= before + blocks;
    let mut bytes = vec![0;130*4096];
    assert_eq!(fs.read(&opened, 0, &mut bytes).unwrap(), size as usize);
    assert_eq!(&bytes[..size as usize], &vec![0x5a;size as usize]);
    if !open {
        fs.resize(&opened, 129*4096, false, &|_| Ok(())).unwrap();
        assert_eq!(fs.read(&opened, 0, &mut bytes).unwrap(), 129*4096);
        assert!(bytes[3..129*4096].iter().all(|b| *b == 0));
        write(fs.as_ref(), &opened, 129*4096-1, b"Z");
        let before = marker(fs.as_ref(), &mark);
        operation("ftruncate-large-zero", || {fs.resize(&opened, 0, false, &|_| Ok(())).unwrap();}, &mut total, ok);
        let after = marker(fs.as_ref(), &mark); *ok &= after >= before + 2;
    } else { write(fs.as_ref(), &opened, 0, b"fresh"); }
}
#[test]
fn mutation_cost_counts() {
    let start = Instant::now();
    println!("COUNTS work=reservations,granted,retained,returned,inode_writes,node_writes,node_reads,vnode_refresh; commits=reservation_pressure,dirty_pressure,publications; decodes=nodes,items,fs_reservation_visits,space_reservation_visits");
    let mut ok = true;
    for (ui, sizes) in [(false, [75,150,300]), (true, [1,2,4])] {
        let mut rows = Vec::new();
        for n in sizes { rows.push(cleanup(ui, n, &mut ok)); }
        // Coordinator 544 requires identical peaks for each operation kind
        // at the three replay sizes, with zero-visit directory edits separate.
        for (n, row) in sizes.into_iter().zip(&rows) {
            println!("REFRESH ui={ui} entries={n} visits={} edits={} peaks_unlink_rmdir={:?} directory_calls={} directory_visits={} pressure_publications={}",
                row.work[7], row.refresh_calls, row.refresh_peaks, row.directory_calls, row.directory_visits, row.commits[2]);
            ok &= row.directory_calls > 0 && row.directory_visits == 0;
        }
        let small = rows.first().unwrap();
        for (n, row) in sizes.into_iter().zip(&rows) {
            if row.refresh_peaks != small.refresh_peaks {
                println!("REFRESH_PEAK_MISMATCH ui={ui} entries={n} expected={:?} actual={:?}", small.refresh_peaks, row.refresh_peaks);
                ok = false;
            }
        }
    }
    truncate(false, &mut ok); truncate(true, &mut ok);
    assert!(start.elapsed().as_secs_f64() < 60.0, "U32 mutation check exceeded 60 s");
    assert!(ok, "mutation refresh bound/peak, publication attribution, tree height bound, or truncate marker oracle failed");
}
