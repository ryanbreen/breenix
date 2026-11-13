//! File data writes and publication of their allocation metadata.

use super::{content_privilege, file, Ext2BlockGroupDesc, Ext2Fs};

impl Ext2Fs {
    pub(super) fn write_file_range_disk(
        &mut self,
        inode_num: u32,
        offset: u64,
        data: &[u8],
        unprivileged: bool,
        mapped: bool,
    ) -> Result<usize, &'static str> {
        self.check_shrink(inode_num)?;
        if data.is_empty() {
            return Ok(0);
        }

        if offset
            .checked_add(data.len() as u64)
            .map_or(true, |end| end > self.max_file_size())
        {
            return Err("File too large");
        }

        // Read the inode
        let mut inode = self.read_inode(inode_num)?;

        // Verify it's a regular file
        if !inode.is_file() {
            return Err("Not a regular file");
        }

        let mode_before = inode.i_mode;
        let times_before = (inode.i_mtime, inode.i_ctime);
        if unprivileged {
            inode.i_mode &= !content_privilege(inode.i_mode);
        }

        // Allocation updates the group counters. Publish their aggregate and
        // the descriptors once per write, including partial allocation failure.
        let free_before: u32 = self
            .block_groups
            .iter()
            .map(|group| group.bg_free_blocks_count as u32)
            .sum();
        let object = self.live_inodes.get(inode_num);
        let previous_epoch = object
            .as_ref()
            .map(|object| object.map.note_disk_write(self.durable_epoch()));
        let written = if mapped {
            file::write_mapped_file_range(
                self.device.as_ref(),
                &mut inode,
                &self.superblock,
                &mut self.block_groups,
                offset,
                data,
            )
        } else {
            file::write_file_range_tracked(
                self.device.as_ref(),
                &mut inode,
                &self.superblock,
                &mut self.block_groups,
                offset,
                data,
            )
        };
        if written == Err(crate::block::BlockError::MetadataPublishFailed) {
            // The bitmap may have landed only in part. Do not publish inode
            // pointers or build another allocation on an uncertain bitmap.
            self.fail_writes();
            return Err("Failed to publish allocation metadata");
        }
        let free_after: u32 = self
            .block_groups
            .iter()
            .map(|group| group.bg_free_blocks_count as u32)
            .sum();
        let mut persisted = Ok(());
        let counts_changed =
            free_before != free_after || self.superblock.s_free_blocks_count != free_after;
        if counts_changed {
            self.superblock.s_free_blocks_count = free_after;
            persisted = self
                .superblock
                .write_to(self.device.as_ref())
                .map_err(|_| "Failed to persist allocation count")
                .and_then(|_| {
                    Ext2BlockGroupDesc::write_table(
                        self.device.as_ref(),
                        &self.superblock,
                        &self.block_groups,
                    )
                    .map_err(|_| "Failed to persist allocation groups")
                });
        }

        // The bitmaps record the blocks allocated by this write, and the
        // inode links them even when the write failed part way (its size and
        // times change only on success). Persist those links on either
        // outcome, so a retry reuses the blocks instead of leaking them.
        let linked = inode
            .write_to(
                self.device.as_ref(),
                inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write inode");
        let changed = written.map_err(|error| match error {
            crate::block::BlockError::NoSpace => "No free blocks available",
            crate::block::BlockError::OutOfMemory => "No memory for file write",
            _ => "Failed to write file data",
        })?;
        persisted?;
        linked?;

        if !changed
            && mode_before == inode.i_mode
            && times_before == (inode.i_mtime, inode.i_ctime)
            && !counts_changed
        {
            if let (Some(object), Some(epoch)) = (object, previous_epoch) {
                object.map.restore_disk_epoch(epoch);
            }
        }
        self.live_inodes.publish_size(inode_num, inode.size());
        Ok(data.len())
    }

}
