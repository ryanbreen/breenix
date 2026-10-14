//! Prepare allocated directory blocks before publishing namespace bytes, and grow a
//! directory that has no room for a new entry.
use super::{block_group, dir, file, Ext2BlockGroupDesc, Ext2Fs, Ext2Inode, Ext2Superblock};
use crate::block::{BlockDevice, BlockError};
use alloc::vec::Vec;
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

        let device = self.device.as_ref();
        let block = block_group::allocate_data_block(device, &self.superblock, &mut self.block_groups)
            .map_err(|e| if e == "No free blocks available" { "No space for another directory block" } else { e })?;
        let mut grown = *inode;
        let linked = file::write_ext2_block(device, block, size, &record[..size])
            .map_err(|_| "Failed to write directory block")
            .and_then(|_| {
                file::set_block_num(device, &mut grown, &self.superblock, &mut self.block_groups, logical, block)
                    .map_err(|_| "Failed to link directory block")
            });
        if let Err(error) = linked {
            let mut attempted = false;
            block_group::release_orphan_block(device, block, &self.superblock, &mut self.block_groups, &mut attempted)?;
            return Err(error);
        }
        grown.i_blocks += (size / 512) as u32;
        grown.i_size = new_size;
        self.write_inode(dir_ino, &grown)?;
        *inode = grown;
        data.extend_from_slice(&record[..size]);

        // The bitmap holds the allocation; publish the free count the groups now carry.
        self.superblock.s_free_blocks_count =
            self.block_groups.iter().map(|group| group.bg_free_blocks_count as u32).sum();
        self.superblock
            .write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;
        Ext2BlockGroupDesc::write_table(self.device.as_ref(), &self.superblock, &self.block_groups)
            .map_err(|_| "Failed to write block group descriptors")
    }
}
