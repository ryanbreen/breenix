//! E24 coordinator 531(w): production mutation phases for failure observations.
use crate::{attrs::InodeAttrs, block::cache::BufferCache,
    breenfs::vfs::BreenFs, read_conformance::{Disk, NoWait}, vfs::*};
use std::sync::{Arc, Mutex};
#[path = "../../../libs/libbreenix/src/suite_mutation_costs.rs"]
mod observations;

fn report(fs: &dyn FileSystem) -> String {
    let mut text = String::new();
    fs.report(Report::State, "/", &mut text).unwrap();
    text
}

#[test]
fn production_mutation_subcosts_cover_cleanup_and_publication() {
    let disk = Arc::new(Disk { bytes: Mutex::new(crate::model::image()), writable: true });
    let fs = BreenFs::mount_writable(disk, Arc::new(BufferCache::new(NoWait)), 24151523, 1).unwrap();
    let before = report(fs.as_ref());
    let baseline = observations::Snapshot::parse(&before);
    let _reader = observations::Snapshot::read;
    let ino = fs.create(2, b"cost", NewNode::Regular { attrs: InodeAttrs::new(0o600, 0, 0) }).unwrap();
    let node = fs.open(ino).unwrap();
    fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false }, b"bytes").unwrap();
    fs.resize(&node, 1, false, &|_| Ok(())).unwrap();
    fs.fsync(&node, false).unwrap();
    drop(node);
    fs.remove(2, b"cost", Remove { directory: false, expected: ino, owner: 0,
        parent: fs.getattr(2).unwrap().attrs }).unwrap();
    let after = report(fs.as_ref());
    for (op, phases) in [
        ("create", & ["operation", "reservation", "inode-edit", "wait"][..]),
        ("write", & ["operation", "reservation", "inode-edit", "wait"][..]),
        ("resize", & ["operation", "reservation", "inode-edit", "wait"][..]),
        ("unlink", & ["operation", "reservation", "inode-edit", "wait"][..]),
        ("fsync", & ["operation", "wait"][..]),
        ("commit", & ["publication", "wait"][..]),
    ] {
        for phase in phases {
            let key = format!("mutation_cost mount=24151523 root=1 op={op} phase={phase} ");
            let line = after.lines().find(|line| line.starts_with(&key))
                .unwrap_or_else(|| panic!("missing {key}:\n{after}"));
            let count = |name| line.split_whitespace().find_map(|s| s.strip_prefix(name))
                .unwrap().parse::<u64>().unwrap();
            assert!(count("started=") > 0, "{line}");
            assert_eq!(count("started="), count("completed="), "{line}");
            assert!(!before.lines().any(|line| line.starts_with(&key)), "fresh mount has no operation history");
        }
    }
    assert!(after.lines().filter(|l| l.starts_with("mutation_cost ")).count() <= 64);
    assert_eq!(fs.lookup(2, b"cost"), Err(crate::errno::Errno(crate::syscall_errno::ENOENT)));
    let mut transport = String::new();
    fs.mutation_costs("root", &mut transport).unwrap();
    assert!(baseline.records("ui-testing", "tree/enabled-flag", false, || panic!("success snapshot read")).is_empty());
    let lines = baseline.records("ui-testing", "tree/enabled-flag", true,
        || Some(observations::Snapshot::parse(&transport)));
    assert!(lines.iter().any(|line| line.contains("op=vfs-unlink-inode-edit started=1 completed=1")), "{lines:?}");

    // Keep a production inode edit in flight while reading its counter path.
    // Release the writer even if the read wrongly waits behind its mount lock.
    let (entered, enter) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let writer_fs = fs.clone();
    let writer = std::thread::spawn(move || {
        writer_fs.create_open(2, b"held", NodeKind::Regular,
            &|_| Ok(InodeAttrs::new(0o600, 0, 0)), &mut |_| {
                entered.send(()).unwrap(); released.recv().unwrap(); Ok(())
            }).unwrap();
    });
    enter.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    let reader_fs = fs.clone();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut text = String::new(); reader_fs.mutation_costs("root", &mut text).unwrap();
        send.send(text).unwrap();
    });
    let captured = receive.recv_timeout(std::time::Duration::from_secs(2));
    release.send(()).unwrap(); writer.join().unwrap(); reader.join().unwrap();
    let captured = captured.expect("diagnostic counters waited for mutation exclusion");
    let lines = baseline.records("gui-apps", "guskit/session-dialog", true,
        || Some(observations::Snapshot::parse(&captured)));
    assert!(lines.iter().any(|line| line.contains("op=vfs-create-inode-edit started=2 completed=1")
        && line.contains("active=1")), "{lines:?}");
}

// Coordinator 574(x6): observe real publication and exclusion boundaries.
#[test]
fn publication_subspans_are_bounded_and_emitted_only_on_failure() {
    use crate::block::{BlockDevice, BlockError};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::{Duration, Instant};

    struct DiskGate {
        disk: Disk,
        flush: Mutex<Option<(Sender<()>, Receiver<()>)>>,
    }
    impl BlockDevice for DiskGate {
        fn block_size(&self) -> usize { self.disk.block_size() }
        fn num_blocks(&self) -> u64 { self.disk.num_blocks() }
        fn read_block(&self, n: u64, out: &mut [u8]) -> Result<(), BlockError> {
            self.disk.read_block(n, out)
        }
        fn write_block(&self, n: u64, bytes: &[u8]) -> Result<(), BlockError> {
            self.disk.write_block(n, bytes)
        }
        fn flush(&self) -> Result<(), BlockError> {
            let gate = self.flush.lock().unwrap().take();
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.recv().unwrap();
            }
            self.disk.flush()
        }
    }
    fn counters(text: &str, op: &str, phase: &str) -> (u64, u64) {
        let key = format!("mutation_cost mount=24151526 root=1 op={op} phase={phase} ");
        let line = text.lines().find(|line| line.starts_with(&key))
            .unwrap_or_else(|| panic!("missing {key}:\n{text}"));
        let count = |key| line.split_whitespace().find_map(|s| s.strip_prefix(key))
            .unwrap().parse::<u64>().unwrap();
        (count("started="), count("completed="))
    }
    let disk = Arc::new(DiskGate {
        disk: Disk { bytes: Mutex::new(crate::model::image()), writable: true },
        flush: Mutex::new(None),
    });
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 24151526, 1).unwrap();
    let baseline = observations::Snapshot::parse(&report(fs.as_ref()));
    let ino = fs.create(2, b"spans", NewNode::Regular { attrs: InodeAttrs::new(0o600, 0, 0) }).unwrap();
    let node = fs.open(ino).unwrap();
    fs.write(&node, WriteReq { at: WriteAt::Offset(0), limit: u64::MAX, clear_setid: false }, b"persist").unwrap();
    let (entered, enter) = channel();
    let (release, released) = channel();
    *disk.flush.lock().unwrap() = Some((entered, released));
    let writer_fs = fs.clone();
    let writer = std::thread::spawn(move || writer_fs.fsync(&node, false).unwrap());
    enter.recv_timeout(Duration::from_secs(2)).unwrap();
    let mut held = String::new();
    fs.mutation_costs("root", &mut held).unwrap();
    // Release before asserting, so a failed check cannot leave a worker parked.
    release.send(()).unwrap();
    writer.join().unwrap();
    assert_eq!(counters(&held, "commit", "prepare-files"), (1, 1));
    assert_eq!(counters(&held, "commit", "next-files-view"), (1, 1));
    assert_eq!(counters(&held, "commit", "write-cached"), (1, 0));
    assert_eq!(counters(&held, "commit", "device-flush"), (1, 0));
    assert!(baseline.records("libc", "dirent-glob/readdir-many", false,
        || panic!("success must not read or format observations")).is_empty());
    let lines = baseline.records("libc", "dirent-glob/readdir-many", true,
        || Some(observations::Snapshot::parse(&held)));
    for phase in ["prepare-files", "next-files-view", "write-cached", "device-flush"] {
        assert_eq!(lines.iter().filter(|l| l.contains(&format!("op=vfs-commit-{phase} "))).count(), 1, "{lines:?}");
    }
    let after = report(fs.as_ref());
    assert_eq!(counters(&after, "commit", "write-cached"), (1, 1));
    assert_eq!(counters(&after, "commit", "device-flush"), (2, 2));
    let read = fs.open(ino).unwrap();
    let mut bytes = [0; 7];
    assert_eq!(fs.read(&read, 0, &mut bytes).unwrap(), 7);
    assert_eq!(&bytes, b"persist");
    drop(read);

    // Hold production mount exclusion in an inode-edit callback, then make a
    // foreground lookup wait behind it; the observation read must not wait.
    let (entered, enter) = channel();
    let (release, released) = channel();
    let holder_fs = fs.clone();
    let holder = std::thread::spawn(move || holder_fs.create_open(2, b"held-x6", NodeKind::Regular,
        &|_| Ok(InodeAttrs::new(0o600, 0, 0)), &mut |_| {
            entered.send(()).unwrap(); released.recv().unwrap(); Ok(())
        }).unwrap());
    enter.recv_timeout(Duration::from_secs(2)).unwrap();
    let (done, completed) = channel();
    let caller_fs = fs.clone();
    let caller = std::thread::spawn(move || { done.send(caller_fs.lookup(2, b"spans")).unwrap(); });
    let until = Instant::now() + Duration::from_secs(2);
    let waiting = loop {
        let mut snapshot = String::new();
        fs.mutation_costs("root", &mut snapshot).unwrap();
        let lines = baseline.records("ui-testing", "tree/enabled-flag", true,
            || Some(observations::Snapshot::parse(&snapshot)));
        if lines.iter().any(|l| l.contains("op=vfs-foreground-mount-exclusion ") && l.contains("active=1 ")) {
            break Some(snapshot);
        }
        if Instant::now() >= until { break None; }
        std::thread::yield_now();
    };
    let blocked = completed.try_recv().is_err();
    release.send(()).unwrap(); holder.join().unwrap(); caller.join().unwrap();
    assert!(blocked, "foreground lookup passed held exclusion");
    assert_eq!(completed.recv().unwrap().unwrap(), ino);
    let waiting = waiting.expect("foreground exclusion wait has no separate observation");
    let (started, completed) = counters(&waiting, "foreground", "mount-exclusion");
    assert_eq!(started, completed + 1);
    let after = report(fs.as_ref());
    let (started, completed) = counters(&after, "foreground", "mount-exclusion");
    assert_eq!(started, completed);
    let lines = baseline.records("ui-testing", "tree/enabled-flag", true,
        || Some(observations::Snapshot::parse(&after)));
    assert!(lines.len() <= 64 && lines.iter().all(|l| l.len() < 384));
    println!("PUBLICATION_SUBSPANS prepare=1 next=1 write=1 flush=2 foreground_wait=observed failure_only=yes bounded=yes");
}
