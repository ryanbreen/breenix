//! E24 ledger 531(t), amended by 533: release, without inode-number reuse.
use crate::{
    attrs::InodeAttrs,
    errno::Errno,
    block::cache::BufferCache,
    breenfs::format,
    breenfs::vfs::BreenFs,
    read_conformance::{Disk, NoWait},
    vfs::*,
};
use std::sync::{Arc, Mutex};

// Use breenfs-check's independent parser and verified walk, without adding a
// filesystem or checker interface. Inspect only the selected root, not DEAD
// nodes that can still contain historical inode records.
#[path = "../../breenfs/src/check/data.rs"] mod data;
#[path = "../../breenfs/src/check/files.rs"] mod files;
#[path = "../../breenfs/src/check/image.rs"] mod image;
#[path = "../../breenfs/src/check/node.rs"] mod node;
#[path = "../../breenfs/src/check/raw.rs"] mod raw;
#[path = "../../breenfs/src/check/recovery.rs"] mod recovery;
#[path = "../../breenfs/src/check/report.rs"] mod report;
#[path = "../../breenfs/src/check/slots.rs"] mod slots;
#[path = "../../breenfs/src/check/space.rs"] mod space;
#[path = "../../breenfs/src/check/walk.rs"] mod walk;

fn stopped_inode_records(bytes: &[u8], ino: u64, tag: &str) -> (bool, bool) {
    let path = std::env::temp_dir().join(format!("breenfs-unlink-{}-{tag}.img", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let image = image::Image::open(&path).unwrap();
    // The open file retains the disposable image until this check returns.
    std::fs::remove_file(&path).unwrap();
    let mut report = report::Report::new();
    let slots = slots::read(&image, &mut report).unwrap();
    slots::check_agreement(&slots, &mut report);
    let selected = recovery::choose(&image, &slots, &mut report).unwrap().unwrap();
    let walk = walk::run(&image, selected, &mut report).unwrap();
    assert_eq!(walk.free_total, Some(selected.free_blocks));
    assert!(walk.roots.iter().all(|root| root.id < selected.next_root_id));
    assert_eq!(selected.global_reserve, (selected.total_blocks / 200).clamp(4096, 262144));
    data::verify(&image, &walk, &mut report).unwrap();
    for (id, detail) in walk.space_findings() { report.error(id, detail); }
    walk.check_files(true, &mut report).unwrap();
    assert_eq!(report.exit_code(), 0, "{:?}", report.lines);
    let root = walk.roots.iter().find(|root| root.id == 1).unwrap();
    let mut stack = vec![root.ptr.addr];
    let mut records = (false, false);
    while let Some(addr) = stack.pop() {
        match walk.fs_nodes.get(&addr).unwrap() {
            walk::FsNode::Internal(children) => stack.extend(children),
            walk::FsNode::Leaf(items) => {
                for (key, _) in items {
                    records.0 |= key.object == ino && key.ty == format::INODE;
                    records.1 |= key.object == walk::ORPHAN_OBJECT
                        && key.ty == format::ORPHAN && key.offset == ino;
                }
            }
        }
    }
    records
}

fn create(fs: &dyn FileSystem, name: &[u8]) -> Arc<VNode> {
    fs.open(fs.create(2, name, NewNode::Regular {
        attrs: InodeAttrs::new(0o600, 0, 0),
    }).unwrap()).unwrap()
}
fn write(fs: &dyn FileSystem, node: &VNode, off: u64, bytes: &[u8]) {
    assert_eq!(fs.write(node, WriteReq {
        at: WriteAt::Offset(off), limit: u64::MAX, clear_setid: false,
    }, bytes).unwrap().bytes, bytes.len());
}
fn free(fs: &dyn FileSystem, marker: &VNode) -> u64 {
    // Same three truncate/write/fsync commits as guest RootMarker::free (465).
    for byte in 0..3 {
        fs.resize(marker, 0, false, &|_| Ok(())).unwrap();
        write(fs, marker, 0, &[byte]);
        fs.fsync(marker, false).unwrap();
    }
    fs.statfs().unwrap().free_blocks
}

#[test]
fn last_close_returns_exact_data_blocks_and_removes_inode() {
    let disk = Arc::new(Disk { bytes: Mutex::new(crate::root_case_setup::populated_image()), writable: true });
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 241533, 1).unwrap();
    let marker = create(fs.as_ref(), b"marker");
    let file = create(fs.as_ref(), b"data");
    write(fs.as_ref(), &file, 0, b"head");
    write(fs.as_ref(), &file, 61440, b"tail");
    let ino = file.ino;
    let second = fs.open(ino).unwrap();
    let control = create(fs.as_ref(), b"ctrl");
    write(fs.as_ref(), &control, 0, b"head");
    write(fs.as_ref(), &control, 61440, b"tail");
    let control_ino = control.ino;
    let control_second = fs.open(control_ino).unwrap();
    let control_before = free(fs.as_ref(), &marker);
    drop(control);
    drop(control_second);
    fs.fsync(&marker, false).unwrap();
    let control_after = free(fs.as_ref(), &marker);
    assert_eq!(fs.lookup(2, b"ctrl").unwrap(), control_ino);
    let before = free(fs.as_ref(), &marker);
    let raw_before = crate::root_case_setup::raw(&disk);
    let blocks = fs.getattr(ino).unwrap().blocks * 512 / fs.statfs().unwrap().block_size;
    assert_eq!(blocks, 2);
    // Isolate the planted leak on a stopped copy of this baseline: its
    // publication must not change the measured guest sequence.
    let planted_disk = Arc::new(Disk { bytes: Mutex::new(disk.bytes.lock().unwrap().clone()), writable: true });
    let planted_fs = BreenFs::mount_writable(planted_disk.clone(), Arc::new(BufferCache::new(NoWait)), 241534, 1).unwrap();
    let leaked = planted_fs.open(ino).unwrap();
    planted_fs.remove(2, b"data", Remove { directory: false, expected: ino,
        owner: 0, parent: planted_fs.getattr(2).unwrap().attrs }).unwrap();
    planted_fs.fsync(&leaked, false).unwrap();
    assert_eq!(stopped_inode_records(&planted_disk.bytes.lock().unwrap(), ino, "planted-orphan"), (true, true));
    assert_eq!(planted_fs.getattr(ino).unwrap().nlink, 0);
    let mut tail = [0; 4];
    assert_eq!(planted_fs.read(&leaked, 61440, &mut tail).unwrap(), 4);
    assert_eq!(&tail, b"tail");
    drop(leaked);
    drop(planted_fs);
    drop(planted_disk);

    fs.remove(2, b"data", Remove {
        directory: false, expected: ino, owner: fs.getattr(ino).unwrap().attrs.uid,
        parent: fs.getattr(2).unwrap().attrs,
    }).unwrap();
    drop(file);
    drop(second);
    fs.fsync(&marker, false).unwrap();
    let after = free(fs.as_ref(), &marker);
    println!("UNLINK_RAW {raw_before:?}->{:?}", crate::root_case_setup::raw(&disk));
    println!("unlink inode={ino} FREE {before}->{after} data_blocks={blocks}");
    let control_delta = i128::from(control_after) - i128::from(control_before);
    let unlink_delta = i128::from(after) - i128::from(before);
    println!("DIFFERENTIAL control={control_before}->{control_after} delta={control_delta} unlink_delta={unlink_delta}");
    assert_eq!(unlink_delta - control_delta, i128::from(blocks));
    assert_eq!(fs.lookup(2, b"data"), Err(Errno(2)));
    assert_eq!(fs.getattr(ino), Err(Errno(2)));
    assert!(matches!(fs.open(ino), Err(Errno(2))));
    let mut names = Vec::new();
    fs.readdir(2, 0, &mut |entry, _| { names.push(entry.ino); Ok(true) }).unwrap();
    assert!(!names.contains(&ino));
    drop(marker);
    drop(fs);
    assert_eq!(stopped_inode_records(&disk.bytes.lock().unwrap(), ino, "closed"), (false, false));
}
