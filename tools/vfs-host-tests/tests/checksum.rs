//! Corruption observed through the production writable adapter (design 2.7).
use crate::{
    attrs::InodeAttrs,
    block::cache::BufferCache,
    breenfs::{
        commit::{Committer, DeviceStore},
        format::{self, Extent, Superblock, Time, BLOCK_SIZE, NODE_SIZE},
        ops::{self, Files},
        recover,
        vfs::BreenFs,
    },
    errno::Errno,
    read_conformance::{Disk, NoWait},
    syscall_errno::*,
    vfs::*,
};
use std::sync::{Arc, Mutex};

const GOOD: &[u8] = b"healthy bytes stay exact";
struct Image {
    disk: Arc<Disk>,
    target: u64,
    good: u64,
    data: usize,
    leaf: usize,
}
fn image() -> Image { image_target(179, 180) }
fn image_target(target_index: usize, count: usize) -> Image {
    let disk = Arc::new(Disk {
        bytes: Mutex::new(vec![0; 64 * 1024 * 1024]),
        writable: true,
    });
    let sb = Superblock {
        fs_uuid: [0x63; 16], generation: 1, slot: 1, total_blocks: 16384,
        global_reserve: 4096, next_root_id: 2, ..Superblock::default()
    };
    let at = Time::default();
    let mut files = Files::format(DeviceStore(disk.as_ref()), &sb).unwrap();
    let good = files.create(2, b"good", 0o644, at).unwrap();
    files.write(disk.as_ref(), good, 0, GOOD, at).unwrap();
    let dir = files.mkdir(2, b"rot", 0o755, at).unwrap();
    // Separate the target inode's leaf from the root and healthy inode.
    let mut target = 0;
    for n in 0..count {
        let ino = files.create(dir, format!("f{n:04}").as_bytes(), 0o644, at).unwrap();
        if n == target_index { target = ino; }
    }
    files.write(disk.as_ref(), target, 0, &vec![0x5a; 3 * BLOCK_SIZE], at).unwrap();
    let sb = files.fsync(disk.as_ref(), &mut Committer::format(sb)).unwrap();
    drop(files);
    let store = DeviceStore(disk.as_ref());
    let root = recover::space_items(&store, &sb).unwrap().into_iter()
        .find(|(k, _)| k.item_type == format::ROOT && k.offset == 1).unwrap().1;
    let root = format::Root::decode(&root).unwrap();
    assert!(root.root.level > 0);
    let data = ops::items(&store, root.root).unwrap().into_iter()
        .find(|(k, _)| k.object == target && k.item_type == format::EXTENT).unwrap().1;
    let data = Extent::decode(&data).unwrap();
    assert_eq!(data.num_blocks, 3);
    let bytes = disk.bytes.lock().unwrap();
    let leaf = (20..sb.total_blocks as usize - 7).find(|&block| {
        let node = &bytes[block * BLOCK_SIZE..block * BLOCK_SIZE + NODE_SIZE];
        let Ok(h) = format::NodeHeader::decode(node) else { return false; };
        h.tree == 1 && h.level == 0 && (0..h.nitems as usize).any(|i| {
            let item = format::LeafItemHeader::decode(&node[48 + i * 21..69 + i * 21]).unwrap();
            item.key == ops::key(target, format::INODE, 0)
        })
    }).unwrap();
    drop(bytes);
    Image { disk, target, good, data: data.phys_start as usize, leaf }
}
fn mount(image: &Image) -> Arc<BreenFs<NoWait>> {
    let fs = BreenFs::mount_writable(image.disk.clone(), Arc::new(BufferCache::new(NoWait)), 93, 1).unwrap();
    fs.init_service().unwrap();
    assert_eq!(fs.read_only(), Ok(false));
    fs
}
fn healthy(fs: &dyn FileSystem, ino: u64) {
    let node = fs.open(ino).unwrap();
    let mut bytes = vec![0; GOOD.len()];
    assert_eq!(fs.read(&node, 0, &mut bytes), Ok(GOOD.len()));
    assert_eq!(bytes, GOOD);
}
fn req(off: u64) -> WriteReq {
    WriteReq { at: WriteAt::Offset(off), limit: u64::MAX, clear_setid: false }
}
#[test]
fn data_flip_refuses_read_page_in_and_partial_edits() {
    let image = image();
    // Corrupt a non-first block: checking only an extent's first CRC is wrong.
    image.disk.bytes.lock().unwrap()[(image.data + 1) * BLOCK_SIZE + 31] ^= 1;
    let fs = mount(&image);
    let node = fs.open(image.target).unwrap();
    let mut bytes = vec![0xa5; 3 * BLOCK_SIZE];
    let read = fs.read(&node, 0, &mut bytes);
    let page_in = fs.mapping_io(&node).unwrap().read(BLOCK_SIZE as u64, BLOCK_SIZE);
    let before = image.disk.bytes.lock().unwrap().clone();
    let write = fs.write(&node, req(BLOCK_SIZE as u64 + 7), b"overwrite");
    let truncate = fs.resize(&node, BLOCK_SIZE as u64 + 19, false, &|_| Ok(()));
    healthy(fs.as_ref(), image.good);
    assert_eq!(read, Err(Errno(EIO)), "read exposed corrupt data");
    assert_eq!(page_in, Err(Errno(EIO)), "mmap page-in exposed corrupt data");
    assert!(matches!(write, Err(WriteError::Filesystem(Errno(EIO)))), "partial write: {write:?}");
    assert_eq!(truncate, Err(Errno(EIO)));
    assert_eq!(*image.disk.bytes.lock().unwrap(), before, "failed partial edit changed disk");
    assert_eq!(fs.getattr(image.target).unwrap().size, (3 * BLOCK_SIZE) as u64);
    assert_eq!(fs.dirty_bytes(), 0);
    assert_eq!(fs.read_only(), Ok(false), "data corruption is local to the file");
    fs.shutdown("checksum-data", u64::MAX).unwrap();
}
#[test]
fn node_flip_latches_read_only_without_losing_healthy_reads_or_unmount() {
    let image = image();
    let fs = mount(&image);
    // Writable recovery walks the FS tree for ORPHANs. Flip after recovery,
    // before this mount has fetched the target leaf into its read cache.
    image.disk.bytes.lock().unwrap()[image.leaf * BLOCK_SIZE + NODE_SIZE - 1] ^= 1;
    // Exercise each entry point as the first observer, with fresh node caches.
    let attr = fs.getattr(image.target);
    assert_eq!(attr, Err(Errno(EIO)));
    let ro = fs.read_only();
    let flags = fs.statfs().unwrap().flags.read_only;
    let before = image.disk.bytes.lock().unwrap().clone();
    let service = crate::breenfs::vfs::measurement::service(fs.as_ref());
    let create = fs.create(2, b"refused", NewNode::Regular { attrs: InodeAttrs::new(0o100600, 0, 0) });
    let open = fs.open(image.target).unwrap_err();
    healthy(fs.as_ref(), image.good);
    fs.shutdown("checksum-node-getattr", u64::MAX).unwrap();
    assert_eq!(*image.disk.bytes.lock().unwrap(), before, "failed mount published writes");
    assert_eq!(service, Err(Errno(EIO)));
    assert_eq!(ro, Ok(true));
    assert!(flags, "mount flags must report ro");
    assert_eq!(create, Err(Errno(EROFS)));
    assert_eq!(open, Errno(EIO));
    drop(fs);
    image.disk.bytes.lock().unwrap()[image.leaf * BLOCK_SIZE + NODE_SIZE - 1] ^= 1;
    let fs = mount(&image);
    image.disk.bytes.lock().unwrap()[image.leaf * BLOCK_SIZE + NODE_SIZE - 1] ^= 1;
    assert_eq!(fs.open(image.target).unwrap_err(), Errno(EIO));
    assert_eq!(fs.read_only(), Ok(true), "open must latch failure independently");
    healthy(fs.as_ref(), image.good);
    fs.shutdown("checksum-node-open", u64::MAX).unwrap();
}

#[test]
fn extent_read_looks_up_one_ref_for_all_its_blocks() {
    use crate::breenfs::btree::{self, NodeStore};
    use std::cell::Cell;
    struct Count<'a> { disk: &'a Disk, root: u64, visits: &'a Cell<usize> }
    impl NodeStore for Count<'_> {
        fn get(&self, addr: u64) -> Result<Vec<u8>, btree::Error> {
            if addr == self.root { self.visits.set(self.visits.get() + 1); }
            DeviceStore(self.disk).get(addr)
        }
        fn put(&mut self, _: u64, _: Vec<u8>) -> Result<(), btree::Error> { panic!("read wrote a node") }
        fn alloc(&mut self) -> Result<u64, btree::Error> { panic!("read allocated a node") }
    }
    let image = image();
    let sb = recover::recover(image.disk.as_ref(), false).unwrap();
    let visits = Cell::new(0);
    let files = Files::new(Count { disk: &image.disk, root: sb.space_root.addr, visits: &visits }, &sb, 1).unwrap();
    visits.set(0);
    let mut bytes = vec![0; 3 * BLOCK_SIZE];
    assert_eq!(files.read(image.disk.as_ref(), image.target, 0, &mut bytes), Ok(bytes.len()));
    assert!(bytes.iter().all(|&byte| byte == 0x5a));
    assert_eq!(visits.get(), 1, "REF lookup must cover the extent, not a block at a time");
}

#[test]
fn corrupt_descendant_mounts_writable_until_first_observation() {
    let image = image_target(150, 360);
    image.disk.bytes.lock().unwrap()[image.leaf * BLOCK_SIZE + NODE_SIZE - 1] ^= 1;
    let fs = mount(&image);
    healthy(fs.as_ref(), image.good);
    assert_eq!(fs.getattr(image.target), Err(Errno(EIO)));
    assert_eq!(fs.read_only(), Ok(true));
    assert_eq!(fs.open(image.target).unwrap_err(), Errno(EIO));
    assert_eq!(fs.create(2, b"refused", NewNode::Regular { attrs: InodeAttrs::new(0o100600, 0, 0) }), Err(Errno(EROFS)));
    healthy(fs.as_ref(), image.good);
    fs.shutdown("checksum-preexisting-node", u64::MAX).unwrap();
}
