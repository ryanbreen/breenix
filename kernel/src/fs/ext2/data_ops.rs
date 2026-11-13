//! Disk-level file reads, size limits and resize publication.
use super::{content_privilege, file, reclaim, Ext2Fs, Ext2Inode};
use alloc::vec::Vec;

impl Ext2Fs {
    /// Read a range of file content from an inode
    pub(crate) fn read_file_range(
        &self,
        inode: &Ext2Inode,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, &'static str> {
        file::read_file_range(
            self.device.as_ref(),
            inode,
            &self.superblock,
            offset,
            length,
        )
        .map_err(|_| "Failed to read file range")
    }

    /// Bound addressing by the sector counter and LARGE_FILE feature.
    pub fn max_file_size(&self) -> u64 {
        let block_size = self.superblock.block_size() as u64;
        let fanout = block_size / 4;
        let addressable = 12 + fanout + fanout.pow(2) + fanout.pow(3);
        // Reserve room for pointer blocks and an external ACL block.
        let sector_limit = u32::MAX as u64 / (block_size / 512);
        let pointer_blocks = 3 + 2 * fanout + fanout.pow(2);
        let limit = addressable.min(sector_limit.saturating_sub(pointer_blocks + 1)) * block_size;
        if self.superblock.s_rev_level == 0 || self.superblock.s_feature_ro_compat & 0x2 == 0 {
            limit.min(i32::MAX as u64)
        } else {
            limit
        }
    }

    pub(super) fn resize_file_disk(
        &mut self,
        inode_num: u32,
        length: u64,
        unprivileged: bool,
    ) -> Result<Option<reclaim::Reclaim>, &'static str> {
        self.check_shrink(inode_num)?;
        let mut inode = self.read_inode(inode_num)?;
        if !inode.is_file() || length > self.max_file_size() {
            return Err("Invalid file size or type");
        }
        if unprivileged {
            inode.i_mode &= !content_privilege(inode.i_mode);
        }
        if length < inode.size() {
            // Publish EOF before modifying any content that used to be visible.
            let mut smaller = inode;
            smaller.i_size = length as u32;
            smaller.i_dir_acl = (length >> 32) as u32;
            smaller.update_timestamps(false, true, true);
            self.write_inode(inode_num, &smaller)?;
            self.live_inodes.publish_size(inode_num, length);
        }
        let shrinking = length < inode.size();
        file::resize_file(self.device.as_ref(), &mut inode, &self.superblock, length)
            .map_err(|_| "Failed to resize file")?;
        self.write_inode(inode_num, &inode)?;
        self.live_inodes.publish_size(inode_num, length);
        if shrinking {
            let progress = reclaim::Reclaim::shrink(
                inode,
                length.div_ceil(self.superblock.block_size() as u64),
            );
            return Ok(Some(progress));
        }
        Ok(None)
    }
}
