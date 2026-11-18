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
