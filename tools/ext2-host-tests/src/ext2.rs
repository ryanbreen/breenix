#[path = "../../../kernel/src/fs/ext2/block_group.rs"]
pub mod block_group;
#[path = "../../../kernel/src/fs/ext2/file.rs"]
pub mod file;
#[path = "../../../kernel/src/fs/ext2/inode.rs"]
pub mod inode;
#[path = "../../../kernel/src/fs/ext2/dir.rs"]
pub mod dir;
#[path = "../../../kernel/src/fs/ext2/create_node.rs"]
pub mod create_node;
#[path = "../../../kernel/src/fs/ext2/directory_io.rs"]
mod directory_io;
#[path = "../../../kernel/src/fs/ext2/directory_extension.rs"]
mod directory_extension;
#[path = "../../../kernel/src/fs/ext2/reclaim.rs"]
pub mod reclaim;
#[path = "../../../kernel/src/fs/ext2/superblock.rs"]
pub mod superblock;
pub use block_group::*;
pub use inode::*;
pub use dir::*;
pub use superblock::*;
/// The kernel's open-inode registry, as directory_io.rs uses it. The harness
/// opens no files, so no inode has a live page-cache object to mark.
pub struct LiveInodes;
impl LiveInodes {
    pub fn get(&self, _inode: u32) -> Option<&LiveInode> {
        None
    }
}
pub struct LiveInode {
    pub map: FileMap,
}
pub struct FileMap;
impl FileMap {
    pub fn note_disk_write(&self, _epoch: u64) -> u64 {
        0
    }
}
pub struct Ext2Fs {
    pub device: Box<dyn crate::block::BlockDevice>,
    pub superblock: Ext2Superblock,
    pub block_groups: Vec<Ext2BlockGroupDesc>,
    durable_epoch: core::sync::atomic::AtomicU64,
    live_inodes: LiveInodes,
}
impl Ext2Fs {
    pub fn host_new(device: Box<dyn crate::block::BlockDevice>, superblock: Ext2Superblock, block_groups: Vec<Ext2BlockGroupDesc>) -> Self {
        Self { device, superblock, block_groups, durable_epoch: core::sync::atomic::AtomicU64::new(0), live_inodes: LiveInodes }
    }
    /// Lay down a fixture directory's records, block by block.
    pub fn write_directory_for_host(&self, ino: u32, data: &[u8]) -> Result<(), &'static str> {
        let inode = self.read_inode(ino)?;
        let size = self.superblock.block_size();
        for (index, records) in data.chunks(size).enumerate() {
            let block = file::get_block_num(self.device.as_ref(), &inode, &self.superblock, index as u32)
                .map_err(|_| "map")?
                .ok_or("unmapped directory block")?;
            file::write_ext2_block(self.device.as_ref(), block, size, records).map_err(|_| "write")?;
        }
        Ok(())
    }
    pub fn read_directory(&self, inode: &Ext2Inode) -> Result<Vec<u8>, &'static str> {
        if !inode.is_dir() { return Err("Not a directory"); }
        file::read_file(self.device.as_ref(),inode,&self.superblock).map_err(|e| if e == crate::block::BlockError::OutOfMemory { "No memory for directory data" } else { "read directory" })
    }
    /// The kernel's mount stops writing here; the harness disk has no write
    /// fence, so a test reads the disk to see what a failure left.
    fn fail_writes(&mut self) {}
    pub fn sync(&self) -> Result<(), &'static str> {
        self.device.flush().map_err(|_| "flush")?;
        self.durable_epoch.fetch_add(1, core::sync::atomic::Ordering::Release);
        Ok(())
    }

    pub fn durable_epoch(&self) -> u64 {
        self.durable_epoch.load(core::sync::atomic::Ordering::Acquire)
    }
    pub fn read_inode(&self, ino: u32) -> Result<Ext2Inode, &'static str> {
        Ext2Inode::read_from(
            self.device.as_ref(),
            ino,
            &self.superblock,
            &self.block_groups,
        )
        .map_err(|_| "read")
    }
    fn write_inode_disk(&self, ino: u32, inode: &Ext2Inode) -> Result<(), &'static str> {
        inode
            .write_to(self.device.as_ref(), ino, &self.superblock, &self.block_groups)
            .map_err(|_| "write")
    }
    pub fn write_inode(&mut self, ino: u32, inode: &Ext2Inode) -> Result<(), &'static str> {
        inode
            .write_to(
                self.device.as_ref(),
                ino,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "write")
    }
}
#[cfg(test)]
mod reclaim_checks {
    use super::*;
    use crate::block;
    use std::sync::{Arc, Mutex};
    struct Disk {
        data: Arc<Mutex<Vec<u8>>>,
        fail: Arc<Mutex<Option<u64>>>,
    }
    impl block::BlockDevice for Disk {
        fn read_block(&self, b: u64, out: &mut [u8]) -> Result<(), block::BlockError> {
            let i = b as usize * 512;
            out.copy_from_slice(&self.data.lock().unwrap()[i..i + out.len()]);
            Ok(())
        }
        fn write_block(&self, b: u64, input: &[u8]) -> Result<(), block::BlockError> {
            let i = b as usize * 512;
            self.data.lock().unwrap()[i..i + input.len()].copy_from_slice(input);
            let mut fail = self.fail.lock().unwrap();
            if *fail == Some(b) {
                *fail = None;
                return Err(block::BlockError::IoError);
            }
            Ok(())
        }
        fn block_size(&self) -> usize {
            512
        }
        fn num_blocks(&self) -> u64 {
            4096
        }
        fn flush(&self) -> Result<(), block::BlockError> {
            Ok(())
        }
    }
    struct BarrierDisk {
        disk: Disk,
        flushes: Arc<core::sync::atomic::AtomicUsize>,
        fail_flush: Arc<core::sync::atomic::AtomicBool>,
    }
    impl block::BlockDevice for BarrierDisk {
        fn read_block(&self, b: u64, out: &mut [u8]) -> Result<(), block::BlockError> {
            block::BlockDevice::read_block(&self.disk, b, out)
        }
        fn write_block(&self, b: u64, input: &[u8]) -> Result<(), block::BlockError> {
            block::BlockDevice::write_block(&self.disk, b, input)
        }
        fn block_size(&self) -> usize { 512 }
        fn num_blocks(&self) -> u64 { 4096 }
        fn flush(&self) -> Result<(), block::BlockError> {
            self.flushes.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if self.fail_flush.load(core::sync::atomic::Ordering::Relaxed) {
                Err(block::BlockError::IoError)
            } else { Ok(()) }
        }
    }

    type BarrierFixture = (Ext2Fs, Arc<Mutex<Vec<u8>>>, Arc<core::sync::atomic::AtomicUsize>, Arc<core::sync::atomic::AtomicBool>);
    fn two_orphans() -> BarrierFixture {
        let data = Arc::new(Mutex::new(vec![0; 512 * 4096]));
        let mut sb = Ext2Superblock::from_bytes(&[0; 1024]).unwrap();
        sb.s_blocks_count = 512;
        sb.s_free_blocks_count = 510;
        sb.s_inodes_count = 128;
        sb.s_free_inodes_count = 126;
        sb.s_log_block_size = 2;
        sb.s_blocks_per_group = 512;
        sb.s_inodes_per_group = 128;
        let mut group: Ext2BlockGroupDesc = unsafe { core::mem::zeroed() };
        group.bg_inode_table = 8;
        group.bg_block_bitmap = 3;
        group.bg_inode_bitmap = 4;
        group.bg_free_blocks_count = 510;
        group.bg_free_inodes_count = 126;
        {
            let mut bytes = data.lock().unwrap();
            bytes[3 * 4096 + 2] = 0x30;
            bytes[4 * 4096] = 3;
        }
        let flushes = Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let fail_flush = Arc::new(core::sync::atomic::AtomicBool::new(false));
        let mut fs = Ext2Fs {
            device: Box::new(BarrierDisk {
                disk: Disk { data: data.clone(), fail: Arc::new(Mutex::new(None)) },
                flushes: flushes.clone(), fail_flush: fail_flush.clone(),
            }),
            superblock: sb, block_groups: vec![group],
            durable_epoch: core::sync::atomic::AtomicU64::new(0),
            live_inodes: LiveInodes,
        };
        for ino in 1..=2 {
            let mut inode = Ext2Inode::new_regular_file(0o600, 0, 0);
            inode.i_links_count = 0;
            inode.i_size = 4096;
            inode.i_blocks = 8;
            inode.i_block[0] = 19 + ino;
            fs.write_inode(ino, &inode).unwrap();
        }
        (fs, data, flushes, fail_flush)
    }

    fn allocated_pair(data: &Arc<Mutex<Vec<u8>>>) -> (u8, u8) {
        let bytes = data.lock().unwrap();
        (bytes[3 * 4096 + 2], bytes[4 * 4096])
    }

    #[test]
    fn two_orphan_detachments_share_one_barrier_before_bitmap_release() {
        let (mut fs, data, flushes, _) = two_orphans();
        let mut a = reclaim::Reclaim::prepare(&fs, 1).ok().expect("first orphan");
        let mut b = reclaim::Reclaim::prepare(&fs, 2).ok().expect("second orphan");
        assert!(a.stage(&fs, 1, &mut 128).ok().expect("stage first"));
        assert!(b.stage(&fs, 2, &mut 128).ok().expect("stage second"));
        assert_eq!(allocated_pair(&data), (0x30, 3));
        for ino in 1..=2 {
            let pointers = fs.read_inode(ino).unwrap().i_block;
            assert_eq!(pointers, [0; 15]);
        }
        fs.sync().unwrap();
        assert!(a.finish(&mut fs, 1, &mut 128).ok().expect("free first"));
        assert!(b.finish(&mut fs, 2, &mut 128).ok().expect("free second"));
        assert_eq!(flushes.load(core::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(allocated_pair(&data), (0, 0));
        let counts = (fs.superblock.s_free_blocks_count, fs.superblock.s_free_inodes_count);
        assert_eq!(counts, (512, 128));
    }

    #[test]
    fn failed_shared_barrier_keeps_both_orphans_allocated_until_success() {
        let (mut fs, data, flushes, fail_flush) = two_orphans();
        let mut a = reclaim::Reclaim::prepare(&fs, 1).ok().expect("first orphan");
        let mut b = reclaim::Reclaim::prepare(&fs, 2).ok().expect("second orphan");
        a.stage(&fs, 1, &mut 128).ok().expect("stage first");
        b.stage(&fs, 2, &mut 128).ok().expect("stage second");
        fail_flush.store(true, core::sync::atomic::Ordering::Relaxed);
        assert!(fs.sync().is_err());
        assert!(a.finish(&mut fs, 1, &mut 128).is_err());
        assert!(b.finish(&mut fs, 2, &mut 128).is_err());
        assert_eq!(allocated_pair(&data), (0x30, 3));
        let counts = (fs.superblock.s_free_blocks_count, fs.superblock.s_free_inodes_count);
        assert_eq!(counts, (510, 126));
        fail_flush.store(false, core::sync::atomic::Ordering::Relaxed);
        assert!(a.finish(&mut fs, 1, &mut 128).ok().expect("free first after recovery"));
        assert!(b.finish(&mut fs, 2, &mut 128).ok().expect("free second after recovery"));
        assert_eq!(flushes.load(core::sync::atomic::Ordering::Relaxed), 4);
        assert_eq!(allocated_pair(&data), (0, 0));
        let counts = (fs.superblock.s_free_blocks_count, fs.superblock.s_free_inodes_count);
        assert_eq!(counts, (512, 128));
    }

    #[test]
    fn orphan_staging_spends_the_shared_budget_before_detaching_more_blocks() {
        let (mut fs, data, _, _) = two_orphans();
        let mut inode = fs.read_inode(1).unwrap();
        inode.i_block[12] = 30;
        inode.i_size = 82 * 4096;
        inode.i_blocks = 72 * 8;
        fs.write_inode(1, &inode).unwrap();
        {
            let mut bytes = data.lock().unwrap();
            for (i, block) in (40u32..110).enumerate() {
                bytes[30 * 4096 + i * 4..30 * 4096 + i * 4 + 4]
                    .copy_from_slice(&block.to_le_bytes());
            }
            for block in core::iter::once(30u32).chain(40..110) {
                bytes[3 * 4096 + block as usize / 8] |= 1 << (block % 8);
            }
        }
        fs.block_groups[0].bg_free_blocks_count = 439;
        fs.superblock.s_free_blocks_count = 439;
        let mut a = reclaim::Reclaim::prepare(&fs, 1).ok().unwrap();
        let mut b = reclaim::Reclaim::prepare(&fs, 2).ok().unwrap();
        let mut budget = 10;
        assert!(a.stage(&fs, 1, &mut budget).ok().unwrap());
        assert_eq!(budget, 0);
        assert!(!b.stage(&fs, 2, &mut budget).ok().unwrap());
        let b_pointer = fs.read_inode(2).unwrap().i_block[0];
        assert_eq!(b_pointer, 21);
        fs.sync().unwrap();
        assert!(!a.finish(&mut fs, 1, &mut 128).ok().unwrap());
        let free = fs.block_groups[0].bg_free_blocks_count;
        assert_eq!(free, 449);
        while !a.finish(&mut fs, 1, &mut 128).ok().unwrap() {}
        assert!(b.finish(&mut fs, 2, &mut 128).ok().unwrap());
        let free = fs.block_groups[0].bg_free_blocks_count;
        assert_eq!(free, 512);
    }

    #[test]
    fn pending_orphans_release_exhausted_space_for_the_next_allocation() {
        let (mut fs, data, _, _) = two_orphans();
        data.lock().unwrap()[3 * 4096..4 * 4096].fill(0xff);
        fs.block_groups[0].bg_free_blocks_count = 0;
        fs.superblock.s_free_blocks_count = 0;
        assert_eq!(allocate_block(fs.device.as_ref(), &fs.superblock, &mut fs.block_groups),
            Err("No free blocks available"));
        let mut a = reclaim::Reclaim::prepare(&fs, 1).ok().unwrap();
        let mut b = reclaim::Reclaim::prepare(&fs, 2).ok().unwrap();
        let mut staging = 128;
        a.stage(&fs, 1, &mut staging).ok().unwrap();
        b.stage(&fs, 2, &mut staging).ok().unwrap();
        fs.sync().unwrap();
        assert!(a.finish(&mut fs, 1, &mut 128).ok().unwrap());
        assert!(b.finish(&mut fs, 2, &mut 128).ok().unwrap());
        let first = allocate_block(fs.device.as_ref(), &fs.superblock, &mut fs.block_groups).unwrap();
        let second = allocate_block(fs.device.as_ref(), &fs.superblock, &mut fs.block_groups).unwrap();
        assert_eq!((first, second), (20, 21));
        let free = fs.block_groups[0].bg_free_blocks_count;
        assert_eq!(free, 0);
    }

    #[test]
    fn linked_inode_is_not_reclaimed() {
        let data = Arc::new(Mutex::new(vec![0; 512 * 4096]));
        let mut sb = Ext2Superblock::from_bytes(&[0; 1024]).unwrap();
        sb.s_blocks_count = 512;
        sb.s_inodes_count = 128;
        sb.s_log_block_size = 2;
        sb.s_blocks_per_group = 512;
        sb.s_inodes_per_group = 128;
        let mut group: Ext2BlockGroupDesc = unsafe { core::mem::zeroed() };
        group.bg_inode_table = 8;
        let mut fs = Ext2Fs {
            device: Box::new(Disk {
                data,
                fail: Arc::new(Mutex::new(None)),
            }),
            superblock: sb,
            block_groups: vec![group],
            durable_epoch: core::sync::atomic::AtomicU64::new(0),
            live_inodes: LiveInodes,
        };
        fs.write_inode(1, &Ext2Inode::new_regular_file(0o600, 0, 0))
            .unwrap();
        match reclaim::Reclaim::prepare(&fs, 1) {
            Err(reclaim::ReclaimError::Abandon(reason)) => assert_eq!(reason, "inode is linked"),
            _ => panic!("linked inode accepted"),
        }
    }
    #[test]
    fn orphan_directory_releases_inode_bitmap() {
        let data = Arc::new(Mutex::new(vec![0; 512 * 4096]));
        let mut sb = Ext2Superblock::from_bytes(&[0; 1024]).unwrap();
        sb.s_blocks_count = 512;
        sb.s_inodes_count = 128;
        sb.s_log_block_size = 2;
        sb.s_blocks_per_group = 512;
        sb.s_inodes_per_group = 128;
        let mut group: Ext2BlockGroupDesc = unsafe { core::mem::zeroed() };
        group.bg_inode_table = 8;
        group.bg_inode_bitmap = 4;
        group.bg_free_inodes_count = 127;
        group.bg_used_dirs_count = 1;
        data.lock().unwrap()[4 * 4096] = 1;
        let mut fs = Ext2Fs { device: Box::new(Disk { data: data.clone(),
            fail: Arc::new(Mutex::new(None)) }), superblock: sb, block_groups: vec![group], durable_epoch: core::sync::atomic::AtomicU64::new(0), live_inodes: LiveInodes };
        let mut inode = Ext2Inode::new_regular_file(0o600, 0, 0);
        inode.i_mode = EXT2_S_IFDIR | 0o700;
        // remove_directory commits the namespace and zero-link inode before
        // handing it to the finalizer; exercise that finalizer here.
        inode.i_links_count = 0;
        fs.write_inode(1, &inode).unwrap();
        let mut progress = reclaim::Reclaim::prepare(&fs, 1).ok().expect("prepare orphan directory");
        assert!(progress.finish(&mut fs, 1, &mut 1).ok().expect("reclaim empty directory"));
        assert_eq!(data.lock().unwrap()[4 * 4096], 0);
        let free = fs.block_groups[0].bg_free_inodes_count;
        assert_eq!(free, 128);
        let directories = fs.block_groups[0].bg_used_dirs_count;
        assert_eq!(directories, 0);
        let links = fs.read_inode(1).unwrap().i_links_count;
        assert_eq!(links, 0);
    }
    #[test]
    fn edit_only_shrink_drops_wild_child_and_rebuilds_sector_count() {
        let data = Arc::new(Mutex::new(vec![0; 512 * 4096]));
        let mut sb = Ext2Superblock::from_bytes(&[0; 1024]).unwrap();
        sb.s_blocks_count = 512;
        sb.s_inodes_count = 128;
        sb.s_log_block_size = 2;
        sb.s_blocks_per_group = 512;
        sb.s_inodes_per_group = 128;
        let mut group: Ext2BlockGroupDesc = unsafe { core::mem::zeroed() };
        group.bg_inode_table = 8;
        let mut inode = Ext2Inode::new_regular_file(0o600, 0, 0);
        inode.i_block[0] = 100;
        inode.i_block[12] = 20;
        inode.i_file_acl = 50;
        inode.i_size = 13 * 4096;
        inode.i_blocks = 99 * 8;
        {
            let mut bytes = data.lock().unwrap();
            bytes[20 * 4096..20 * 4096 + 4].copy_from_slice(&101u32.to_le_bytes());
            bytes[20 * 4096 + 4..20 * 4096 + 8].copy_from_slice(&512u32.to_le_bytes());
        }
        let mut fs = Ext2Fs { device: Box::new(Disk { data: data.clone(),
            fail: Arc::new(Mutex::new(None)) }), superblock: sb, block_groups: vec![group], durable_epoch: core::sync::atomic::AtomicU64::new(0), live_inodes: LiveInodes };
        fs.write_inode(1, &inode).unwrap();
        let mut progress = reclaim::Reclaim::shrink(inode, 13);
        inode.i_links_count = 2;
        inode.i_mtime = 42;
        fs.write_inode(1, &inode).unwrap();
        assert!(progress.finish(&mut fs, 1, &mut 128).unwrap_or(false));
        let inode = fs.read_inode(1).unwrap();
        let sectors = inode.i_blocks;
        let links = inode.i_links_count;
        let mtime = inode.i_mtime;
        assert_eq!((sectors, links, mtime), (4 * 8, 2, 42));
        assert_eq!(&data.lock().unwrap()[20 * 4096 + 4..20 * 4096 + 8], &[0; 4]);
        assert_eq!(file::get_block_num(fs.device.as_ref(), &inode, &fs.superblock, 13).unwrap(), None);
    }
    #[test]
    fn shrink_resumes_after_a_bitmap_write_lands_but_reports_error() {
        let data = Arc::new(Mutex::new(vec![0; 512 * 4096]));
        let fail = Arc::new(Mutex::new(None));
        let mut sb = Ext2Superblock::from_bytes(&[0; 1024]).unwrap();
        sb.s_blocks_count = 512;
        sb.s_inodes_count = 128;
        sb.s_log_block_size = 2;
        sb.s_blocks_per_group = 512;
        sb.s_inodes_per_group = 128;
        sb.s_free_blocks_count = 380;
        sb.s_free_inodes_count = 127;
        let mut group: Ext2BlockGroupDesc = unsafe { core::mem::zeroed() };
        group.bg_block_bitmap = 3;
        group.bg_inode_bitmap = 4;
        group.bg_inode_table = 8;
        group.bg_free_blocks_count = 380;
        group.bg_free_inodes_count = 127;
        let mut inode = Ext2Inode::new_regular_file(0o600, 0, 0);
        inode.i_block[0] = 100;
        inode.i_block[12] = 20;
        inode.i_size = 14 * 4096;
        inode.i_blocks = 132 * 8;
        {
            let mut bytes = data.lock().unwrap();
            for block in std::iter::once(20).chain(100..231) {
                bytes[3 * 4096 + block / 8] |= 1 << (block % 8);
            }
            bytes[4 * 4096] = 1;
            for i in 0..130 {
                bytes[20 * 4096 + i * 4..20 * 4096 + i * 4 + 4]
                    .copy_from_slice(&(101 + i as u32).to_le_bytes());
            }
        }
        let mut fs = Ext2Fs {
            device: Box::new(Disk {
                data: data.clone(),
                fail: fail.clone(),
            }),
            superblock: sb,
            block_groups: vec![group],
            durable_epoch: core::sync::atomic::AtomicU64::new(0),
            live_inodes: LiveInodes,
        };
        fs.write_inode(1, &inode).unwrap();
        let mut progress = reclaim::Reclaim::shrink(inode, 14);
        *fail.lock().unwrap() = Some(3 * 8);
        assert!(matches!(
            progress.finish(&mut fs, 1, &mut 7),
            Err(reclaim::ReclaimError::Retry)
        ));
        let mut passes = 0;
        while !progress.finish(&mut fs, 1, &mut 7).unwrap_or(false) {
            passes += 1;
            assert!(passes < 40);
        }
        let free = fs.superblock.s_free_blocks_count;
        assert_eq!(free, 508);
        let inode = fs.read_inode(1).unwrap();
        let sectors = inode.i_blocks;
        assert_eq!(sectors, 4 * 8);
        assert_eq!(inode.size(), 14 * 4096);
        assert_eq!(
            file::get_block_num(fs.device.as_ref(), &inode, &fs.superblock, 13).unwrap(),
            Some(102)
        );
        assert_eq!(
            file::get_block_num(fs.device.as_ref(), &inode, &fs.superblock, 14).unwrap(),
            None
        );
        let mut orphan = inode;
        orphan.i_links_count = 0;
        fs.write_inode(1, &orphan).unwrap();
        let mut progress = reclaim::Reclaim::prepare(&fs, 1).ok().unwrap();
        while !progress.finish(&mut fs, 1, &mut 2).unwrap_or(false) {}
        let free = fs.superblock.s_free_blocks_count;
        let free_inodes = fs.superblock.s_free_inodes_count;
        assert_eq!(free, 512);
        assert_eq!(free_inodes, 128);
    }
    #[test]
    fn unix_node_prepare_failure_leaves_directory_unchanged() {
        let (mut fs, data, _, _) = two_orphans();
        let mut parent = Ext2Inode::new_regular_file(0o777, 0, 0);
        parent.i_mode = 0o40777; parent.i_size = 4096; parent.i_blocks = 8; parent.i_block[0] = 21;
        fs.write_inode(2, &parent).unwrap();
        {
            let mut bytes = data.lock().unwrap();
            let dir = &mut bytes[21*4096..22*4096]; dir.fill(0);
            dir[4..6].copy_from_slice(&4096u16.to_le_bytes());
        }
        let before = fs.read_directory(&parent).unwrap();
        assert_eq!(fs.create_node_prepared(2, b"sock\xff", 0o140755, 0, 0, 0, |_, _| Err::<(), _>("No memory for inode pin")), Err("No memory for inode pin"));
        assert_eq!(fs.read_directory(&parent).unwrap(), before);
        let ino = fs.create_node_prepared(2, b"sock\xff", 0o140755, 0, 0, 0, |fs, ino| {
            assert_eq!(dir::find_inode_bytes(&fs.read_directory(&parent)?, b"sock\xff"), None);
            Ok(ino)
        }).unwrap();
        assert_eq!(dir::find_inode_bytes(&fs.read_directory(&parent).unwrap(), b"sock\xff"), Some(ino));
        let mode = fs.read_inode(ino).unwrap().i_mode;
        assert_eq!(mode, 0o140755);
    }
    /// A directory (inode 2, block 21) with room, on a disk whose writes can
    /// be made to fail after they land.
    fn directory_fixture() -> (Ext2Fs, Arc<Mutex<Vec<u8>>>, Arc<Mutex<Option<u64>>>) {
        let data = Arc::new(Mutex::new(vec![0; 512 * 4096]));
        let fail = Arc::new(Mutex::new(None));
        let mut sb = Ext2Superblock::from_bytes(&[0; 1024]).unwrap();
        sb.s_blocks_count = 512;
        sb.s_free_blocks_count = 490;
        sb.s_inodes_count = 128;
        sb.s_free_inodes_count = 126;
        sb.s_log_block_size = 2;
        sb.s_blocks_per_group = 512;
        sb.s_inodes_per_group = 128;
        let mut group: Ext2BlockGroupDesc = unsafe { core::mem::zeroed() };
        group.bg_inode_table = 8;
        group.bg_block_bitmap = 3;
        group.bg_inode_bitmap = 4;
        group.bg_free_blocks_count = 490;
        group.bg_free_inodes_count = 126;
        {
            let mut bytes = data.lock().unwrap();
            bytes[3 * 4096..3 * 4096 + 2].fill(0xff);
            bytes[3 * 4096 + 2] = 0x3f;
            bytes[4 * 4096] = 3;
            let dir = &mut bytes[21 * 4096..22 * 4096];
            dir[4..6].copy_from_slice(&4096u16.to_le_bytes());
        }
        let mut fs = Ext2Fs {
            device: Box::new(Disk { data: data.clone(), fail: fail.clone() }),
            superblock: sb,
            block_groups: vec![group],
            durable_epoch: core::sync::atomic::AtomicU64::new(0),
            live_inodes: LiveInodes,
        };
        let mut parent = Ext2Inode::new_regular_file(0o777, 0, 0);
        parent.i_mode = 0o40777;
        parent.i_links_count = 2;
        parent.i_size = 4096;
        parent.i_blocks = 8;
        parent.i_block[0] = 21;
        fs.write_inode(2, &parent).unwrap();
        fs.superblock.write_to(fs.device.as_ref()).unwrap();
        Ext2BlockGroupDesc::write_table(fs.device.as_ref(), &fs.superblock, &fs.block_groups).unwrap();
        (fs, data, fail)
    }

    #[test]
    fn create_whose_entry_write_fails_leaves_the_disk_as_it_was() {
        let (mut fs, data, fail) = directory_fixture();
        let before = data.lock().unwrap().clone();
        // The directory block's first sector lands, then reports an error.
        *fail.lock().unwrap() = Some(21 * 8);
        let created = fs.create_node(2, "f", 0o100644, 0, 0, 0);
        assert_eq!(created, Err("Failed to write new directory entry"));
        assert!(*data.lock().unwrap() == before, "a failed create changed the disk");
        let counts = (fs.superblock.s_free_inodes_count, fs.block_groups[0].bg_free_inodes_count);
        assert_eq!(counts, (126, 126));
        let ino = fs.create_node(2, "f", 0o100644, 0, 0, 0).unwrap();
        let parent = fs.read_inode(2).unwrap();
        assert_eq!(dir::find_inode_bytes(&fs.read_directory(&parent).unwrap(), b"f"), Some(ino));
    }

    #[test]
    fn create_whose_count_write_fails_takes_the_name_back_out() {
        let (mut fs, data, fail) = directory_fixture();
        let before = data.lock().unwrap().clone();
        // The superblock, the plan's last write, lands and reports an error.
        *fail.lock().unwrap() = Some(2);
        assert!(fs.create_node(2, "f", 0o100644, 0, 0, 0).is_err());
        assert!(*data.lock().unwrap() == before, "a failed create changed the disk");
        let parent = fs.read_inode(2).unwrap();
        assert_eq!(dir::find_inode_bytes(&fs.read_directory(&parent).unwrap(), b"f"), None);
    }

    #[test]
    fn unix_node_raw_directory_names_remain_distinct() {
        let mut dir = vec![0;4096]; dir[4..6].copy_from_slice(&4096u16.to_le_bytes());
        dir::add_directory_entry_bytes(&mut dir, 17, b"s\xfe", dir::EXT2_FT_SOCK).unwrap();
        dir::add_directory_entry_bytes(&mut dir, 18, b"s\xff", dir::EXT2_FT_SOCK).unwrap();
        assert_eq!(dir::find_inode_bytes(&dir, b"s\xfe"), Some(17));
        assert_eq!(dir::find_inode_bytes(&dir, b"s\xff"), Some(18));
    }

}
