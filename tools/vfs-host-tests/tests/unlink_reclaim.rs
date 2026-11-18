//! E24 coordinator 567: release by extent identity, with 533 inode absence.
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

#[derive(Debug)]
struct Stopped {
    generation: u64,
    status: i32,
    records: (bool, bool),
    extents: Vec<(u64, u64)>,
    owners: Vec<(u64, u64)>,
    free: Vec<(u64, u64)>,
    pending: Vec<(u64, u64, u64)>,
    refs: Vec<(u64, u64)>,
    findings: Vec<String>,
}
fn stopped(bytes: &[u8], ino: u64, full: bool) -> Stopped {
    let path = std::env::temp_dir().join(format!("breenfs-unlink-{}.img", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let image = image::Image::open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let mut report = report::Report::new();
    let slots = slots::read(&image, &mut report).unwrap();
    slots::check_agreement(&slots, &mut report);
    let selected = recovery::choose(&image, &slots, &mut report).unwrap().unwrap();
    let walk = walk::run(&image, selected, &mut report).unwrap();
    assert_eq!(walk.free_total, Some(selected.free_blocks));
    assert!(walk.roots.iter().all(|root| root.id < selected.next_root_id));
    assert_eq!(selected.global_reserve, (selected.total_blocks / 200).clamp(4096, 262144));
    if full { data::verify(&image, &walk, &mut report).unwrap(); }
    for (id, detail) in walk.space_findings() { report.error(id, detail); }
    walk.check_files(full, &mut report).unwrap();
    let root = walk.roots.iter().find(|root| root.id == 1).unwrap();
    let mut stack = vec![root.ptr.addr];
    let mut records = (false, false);
    let mut extents = Vec::new();
    while let Some(addr) = stack.pop() {
        match walk.fs_nodes.get(&addr).unwrap() {
            walk::FsNode::Internal(children) => stack.extend(children),
            walk::FsNode::Leaf(items) => {
                for (key, value) in items {
                    records.0 |= key.object == ino && key.ty == format::INODE;
                    records.1 |= key.object == walk::ORPHAN_OBJECT
                        && key.ty == format::ORPHAN && key.offset == ino;
                    if key.object == ino && key.ty == format::EXTENT {
                        // Independent checker's raw payload, design 2.3 offsets.
                        let start = raw::u64_at(value, 0) + u64::from(raw::u32_at(value, 12));
                        extents.push((start, u64::from(raw::u32_at(value, 16))));
                    }
                }
            }
        }
    }
    // Include EXTENT records in other roots and DEAD leaves as owners too.
    let owners = walk.extents.iter().map(|e| (e.phys_start, e.phys_len)).collect();
    let refs = walk.ref_items.iter().map(|r| (r.start, r.length)).collect();
    Stopped { generation: selected.generation, status: report.exit_code(), records, extents, owners,
        free: walk.free, pending: walk.pending, refs, findings: report.lines }
}
fn contains(ranges: &[(u64, u64)], block: u64) -> bool {
    ranges.iter().any(|&(start, length)| (start..start + length).contains(&block))
}
fn released(snapshot: &Stopped, blocks: &[u64], publication: Option<u64>) -> bool {
    snapshot.records == (false, false) && blocks.iter().all(|&block| {
        !contains(&snapshot.owners, block) && !contains(&snapshot.refs, block)
            && if let Some(generation) = publication {
                snapshot.pending.iter().any(|&(start, length, released)|
                    released == generation && (start..start + length).contains(&block))
                    && !contains(&snapshot.free, block)
            } else {
                contains(&snapshot.free, block) && !snapshot.pending.iter().any(|&(start, length, _)|
                    (start..start + length).contains(&block))
            }
    })
}

// Deliberately leak one former data extent in a stopped copy. Rewrite only
// SPACE records and their checksums; no writer/allocator fault hook is needed.
// The independent checker must report only A4 (a REF with no incoming owner).
fn plant_extent(bytes: &mut [u8], start: u64, length: u64, snapshot: &Stopped) {
    use crate::breenfs::{crc32c, format::{BlockPtr, Free, Key, LeafItemHeader, NodeHeader, Ref, Superblock}};
    fn edit(bytes: &mut [u8], mut ptr: BlockPtr, key: Key, value: Option<Vec<u8>>) -> BlockPtr {
        let offset = ptr.addr as usize * 4096;
        let mut node = bytes[offset..offset + format::NODE_SIZE].to_vec();
        let mut header = NodeHeader::decode(&node).unwrap();
        if header.level == 0 {
            let mut items = std::collections::BTreeMap::new();
            for i in 0..usize::from(header.nitems) {
                let item = LeafItemHeader::decode(&node[48 + i * 21..]).unwrap();
                items.insert(item.key, node[usize::from(item.data_off)..usize::from(item.data_off + item.data_len)].to_vec());
            }
            if let Some(value) = value { items.insert(key, value); } else { assert!(items.remove(&key).is_some()); }
            node.fill(0);
            header.nitems = items.len() as u16;
            let mut end = node.len();
            for (i, (key, value)) in items.into_iter().enumerate() {
                end -= value.len();
                assert!(end >= 48 + usize::from(header.nitems) * 21);
                node[end..end + value.len()].copy_from_slice(&value);
                node[48 + i * 21..48 + (i + 1) * 21].copy_from_slice(&LeafItemHeader {
                    key, data_off: end as u16, data_len: value.len() as u16,
                }.encode());
            }
        } else {
            let index = (0..usize::from(header.nitems)).rfind(|&i|
                Key::decode(&node[48 + i * 37..]).unwrap() <= key).unwrap_or(0);
            let at = 48 + index * 37;
            let child = BlockPtr { addr: raw::u64_at(&node, at + 17),
                generation: raw::u64_at(&node, at + 25), csum: raw::u32_at(&node, at + 33), level: header.level - 1 };
            let child = edit(bytes, child, key, value);
            let child_at = child.addr as usize * 4096;
            let first = bytes[child_at + 48..child_at + 65].to_vec();
            node[at..at + 17].copy_from_slice(&first);
            node[at + 33..at + 37].copy_from_slice(&child.csum.to_le_bytes());
        }
        header.encode(&mut node).unwrap();
        ptr.csum = raw::u32_at(&node, 8);
        bytes[offset..offset + node.len()].copy_from_slice(&node);
        ptr
    }
    let mut sb = (16..20).filter_map(|block| Superblock::decode(&bytes[block * 4096..(block + 1) * 4096]).ok())
        .max_by_key(|sb| sb.generation).unwrap();
    let &(free_start, free_len) = snapshot.free.iter().find(|&&(s, n)| s <= start && start + length <= s + n).unwrap();
    let free_key = |offset| Key { object: 0xffff_ffff_ffff_ff20, item_type: format::FREE, offset };
    sb.space_root = edit(bytes, sb.space_root, free_key(free_start), None);
    for (offset, length) in [(free_start, start - free_start), (start + length, free_start + free_len - start - length)] {
        if length != 0 { sb.space_root = edit(bytes, sb.space_root, free_key(offset), Some(Free { length }.encode().to_vec())); }
    }
    let crcs = (start..start + length).map(|block| crc32c::checksum(&bytes[block as usize * 4096..(block + 1) as usize * 4096])).collect();
    let value = Ref { refcount: 1, kind: 3, alloc_generation: sb.generation, crcs }.encode();
    sb.space_root = edit(bytes, sb.space_root, Key { object: start, item_type: format::REF, offset: length }, Some(value));
    sb.free_blocks -= length;
    for ring in 0..2 {
        sb.ring = ring;
        let block = if ring == 0 { 16 + u64::from(sb.slot) } else { sb.total_blocks - 4 + u64::from(sb.slot) };
        bytes[block as usize * 4096..(block + 1) as usize * 4096].copy_from_slice(&sb.encode());
    }
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
fn last_close_releases_identified_extents_and_removes_inode() {
    let disk = Arc::new(Disk { bytes: Mutex::new(crate::root_case_setup::populated_image()), writable: true });
    let fs = BreenFs::mount_writable(disk.clone(), Arc::new(BufferCache::new(NoWait)), 241533, 1).unwrap();
    let marker = create(fs.as_ref(), b"marker");
    let file = create(fs.as_ref(), b"data");
    write(fs.as_ref(), &file, 0, b"head");
    write(fs.as_ref(), &file, 61440, b"tail");
    let ino = file.ino;
    let second = fs.open(ino).unwrap();
    let before = free(fs.as_ref(), &marker);
    let baseline = stopped(&disk.bytes.lock().unwrap(), ino, false);
    assert_eq!(baseline.status, 0, "{:?}", baseline.findings);
    let physical: Vec<_> = baseline.extents.iter().flat_map(|&(start, length)| start..start + length).collect();
    assert_eq!(physical.len(), 2);
    assert_eq!(physical.iter().collect::<std::collections::BTreeSet<_>>().len(), 2);
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
    let planted = stopped(&planted_disk.bytes.lock().unwrap(), ino, true);
    assert_eq!(planted.status, 0, "{:?}", planted.findings);
    assert_eq!(planted.records, (true, true));
    assert!(!released(&planted, &physical, Some(planted.generation)));
    println!("PLANTED_ORPHAN rejected=true extents={} blocks={}", planted.extents.len(), physical.len());
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
    let publication = stopped(&disk.bytes.lock().unwrap(), ino, false);
    assert_eq!(publication.status, 0, "{:?}", publication.findings);
    assert!(released(&publication, &physical, Some(publication.generation)), "{publication:?} blocks={physical:?}");
    println!("UNLINK_EXTENTS generation={} extents={} blocks={} pending={} physical={physical:?}",
        publication.generation, baseline.extents.len(), physical.len(), physical.len());
    // Exactly two subsequent publications; do not allocate a different marker
    // or add a control span that changes quarantine history.
    for byte in 0..2 {
        fs.resize(&marker, 0, false, &|_| Ok(())).unwrap();
        write(fs.as_ref(), &marker, 0, &[byte]);
        fs.fsync(&marker, false).unwrap();
    }
    let final_image = disk.bytes.lock().unwrap().clone();
    let final_state = stopped(&final_image, ino, true);
    assert_eq!(final_state.status, 0, "{:?}", final_state.findings);
    assert_eq!(final_state.generation, publication.generation + 2);
    assert!(released(&final_state, &physical, None), "{final_state:?} blocks={physical:?}");
    let after = fs.statfs().unwrap().free_blocks;
    assert!(after >= before + blocks);
    println!("RECLAIM_EXTENTS generation={} free={} owners=0 FREE={before}->{after}", final_state.generation, physical.len());
    let mut allocated = final_image;
    let (start, length) = baseline.extents[0];
    plant_extent(&mut allocated, start, length, &final_state);
    let planted = stopped(&allocated, ino, true);
    assert_eq!(planted.records, (false, false));
    assert!(planted.findings.iter().any(|line| line.starts_with("error A4 ")), "{:?}", planted.findings);
    assert!(planted.findings.iter().all(|line| line.starts_with("error A4 ")), "{:?}", planted.findings);
    assert!(contains(&planted.refs, start));
    assert!(!released(&planted, &physical, None));
    println!("PLANTED_EXTENT rejected=true extents=1 blocks={length} inode=false orphan=false findings={:?}", planted.findings);
    assert_eq!(fs.lookup(2, b"data"), Err(Errno(2)));
    assert_eq!(fs.getattr(ino), Err(Errno(2)));
    assert!(matches!(fs.open(ino), Err(Errno(2))));
    let mut names = Vec::new();
    fs.readdir(2, 0, &mut |entry, _| { names.push(entry.ino); Ok(true) }).unwrap();
    assert!(!names.contains(&ino));
    drop(marker);
    drop(fs);
    assert_eq!(final_state.records, (false, false));
}
