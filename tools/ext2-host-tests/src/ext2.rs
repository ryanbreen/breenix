#[path = "../../../kernel/src/fs/ext2/block_group.rs"]
pub mod block_group;
#[path = "../../../kernel/src/fs/ext2/file.rs"]
pub mod file;
#[path = "../../../kernel/src/fs/ext2/inode.rs"]
pub mod inode;
#[path = "../../../kernel/src/fs/ext2/reclaim.rs"]
pub mod reclaim;
#[path = "../../../kernel/src/fs/ext2/superblock.rs"]
pub mod superblock;
pub use block_group::*;
pub use inode::*;
pub use superblock::*;
pub struct Ext2Fs {
    pub device: Box<dyn crate::block::BlockDevice>,
    pub superblock: Ext2Superblock,
    pub block_groups: Vec<Ext2BlockGroupDesc>,
}
impl Ext2Fs {
    pub fn read_inode(&self, ino: u32) -> Result<Ext2Inode, &'static str> {
        Ext2Inode::read_from(
            self.device.as_ref(),
            ino,
            &self.superblock,
            &self.block_groups,
        )
        .map_err(|_| "read")
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
        };
        fs.write_inode(1, &Ext2Inode::new_regular_file(0o600, 0, 0))
            .unwrap();
        match reclaim::Reclaim::prepare(&fs, 1) {
            Err(reclaim::ReclaimError::Abandon(reason)) => assert_eq!(reason, "inode is linked"),
            _ => panic!("linked inode accepted"),
        }
    }
    #[test]
    fn directory_last_link_releases_inode_bitmap() {
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
        data.lock().unwrap()[4 * 4096] = 1;
        let mut fs = Ext2Fs { device: Box::new(Disk { data: data.clone(),
            fail: Arc::new(Mutex::new(None)) }), superblock: sb, block_groups: vec![group] };
        let mut inode = Ext2Inode::new_regular_file(0o600, 0, 0);
        inode.i_mode = EXT2_S_IFDIR | 0o700;
        fs.write_inode(1, &inode).unwrap();
        assert_eq!(inode::reclaim_directory_inode(fs.device.as_ref(), 1,
            &fs.superblock, &mut fs.block_groups).unwrap(), 0);
        assert_eq!(data.lock().unwrap()[4 * 4096], 0);
        let free = fs.block_groups[0].bg_free_inodes_count;
        assert_eq!(free, 128);
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
            fail: Arc::new(Mutex::new(None)) }), superblock: sb, block_groups: vec![group] };
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
}
