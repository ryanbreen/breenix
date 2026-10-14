//! Prepare allocated directory blocks before publishing namespace bytes, and grow a
//! directory that has no room for a new entry.
use super::{block_group, dir, file, Ext2BlockGroupDesc, Ext2Fs, Ext2Inode, Ext2Superblock};
use crate::block::{BlockDevice, BlockError};
use alloc::vec::Vec;
use super::directory_extension::Extension;
pub(super) fn prepare<B: BlockDevice + ?Sized>(
    device: &B,
    inode: &Ext2Inode,
    sb: &Ext2Superblock,
    len: usize,
) -> Result<Vec<u32>, &'static str> {
    if !inode.is_dir() {
        return Err("Not a directory");
    }
    if len as u64 > inode.size() {
        return Err("No space for directory data");
    }
    let count = len.div_ceil(sb.block_size());
    let mut blocks = Vec::new();
    blocks
        .try_reserve_exact(count)
        .map_err(|_| "No memory for directory write plan")?;
    for logical in 0..count {
        let block = file::get_block_num(device, inode, sb, logical as u32)
            .map_err(|e| {
                if e == BlockError::OutOfMemory {
                    "No memory for directory addressing"
                } else {
                    "Failed to read directory addressing"
                }
            })?
            .ok_or("No space for directory data")?;
        blocks.push(block);
    }
    Ok(blocks)
}
pub(super) fn write<B: BlockDevice + ?Sized>(
    device: &B,
    size: usize,
    blocks: &[u32],
    data: &[u8],
) -> Result<(), &'static str> {
    if size == 0 || size > 4096 || blocks.len() != data.len().div_ceil(size) {
        return Err("Invalid directory write plan");
    }
    for (&block, chunk) in blocks.iter().zip(data.chunks(size)) {
        let mut bytes = [0u8; 4096];
        bytes[..chunk.len()].copy_from_slice(chunk);
        file::write_ext2_block(device, block, size, &bytes[..size])
            .map_err(|_| "Failed to write directory block")?;
    }
    Ok(())
}

impl Ext2Fs {
    /// Make room in the directory `dir_ino` for an entry with a `name_len`-byte name.
    /// `data` holds the directory's records and `inode` is the caller's copy of its inode.
    /// When no record has room, the directory gets one more block holding a single empty
    /// record: the block is written first, then linked at the directory's end with the
    /// size grown, so the directory on disk is whole at every step and stays valid whether
    /// or not the caller's insertion then succeeds. `data` and `inode` are updated to match.
    pub(super) fn make_directory_room(
        &mut self,
        dir_ino: u32,
        inode: &mut Ext2Inode,
        data: &mut Vec<u8>,
        name_len: usize,
    ) -> Result<(), &'static str> {
        if dir::has_room(data, name_len)? {
            return Ok(());
        }
        let size = self.superblock.block_size();
        if size > 4096 || data.len() as u64 != inode.size() || data.len() % size != 0 {
            return Err("Corrupt directory size");
        }
        let logical = u32::try_from(data.len() / size).map_err(|_| "No space in directory")?;
        let new_size = u32::try_from(data.len() + size).map_err(|_| "No space in directory")?;
        data.try_reserve_exact(size)
            .map_err(|_| "No memory for directory data")?;
        // One empty record spanning the block: inode 0, rec_len the block size.
        let mut record = [0u8; 4096];
        record[4..6].copy_from_slice(&(size as u16).to_le_bytes());

        let mut groups = Vec::new();
        groups.try_reserve_exact(self.block_groups.len()).map_err(|_| "No memory for directory transaction")?;
        groups.extend_from_slice(&self.block_groups);
        let transaction = Extension::new(self.device.as_ref(), &self.superblock, &groups)?;
        let block = block_group::allocate_data_block(&transaction, &self.superblock, &mut groups)
            .map_err(|e| if e == "No free blocks available" { "No space for another directory block" } else { e })?;
        let mut grown = *inode;
        file::write_ext2_block(&transaction, block, size, &record[..size])
            .map_err(|_| "Failed to write directory block")?;
        file::set_block_num(&transaction, &mut grown, &self.superblock, &mut groups, logical, block)
            .map_err(|e| if e == BlockError::NoSpace { "No space for directory addressing" } else { "Failed to link directory block" })?;
        grown.i_blocks += (size / 512) as u32;
        grown.i_size = new_size;
        grown.write_to(&transaction, dir_ino, &self.superblock, &groups)
            .map_err(|_| "Failed to write inode")?;
        let mut sb = self.superblock;
        sb.s_free_blocks_count = groups.iter().map(|group| group.bg_free_blocks_count as u32).sum();
        sb.write_to(&transaction).map_err(|_| "Failed to write superblock")?;
        Ext2BlockGroupDesc::write_table(&transaction, &sb, &groups)
            .map_err(|_| "Failed to write block group descriptors")?;
        if let Err(restored) = transaction.publish() {
            if !restored {
                self.block_groups = groups;
                self.superblock = sb;
                return Err("Failed to restore directory extension; allocations retained");
            }
            return Err("Failed to publish directory extension");
        }
        self.block_groups = groups;
        self.superblock = sb;
        if let Some(object) = self.live_inodes.get(dir_ino) {
            object.map.note_disk_write(self.durable_epoch());
        }
        *inode = grown;
        data.extend_from_slice(&record[..size]);
        Ok(())
    }
}
