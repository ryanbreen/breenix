//! Write back the directory blocks an added or removed entry changed, and grow a
//! directory that has no room for a new entry.
use super::{block_group, dir, file, Ext2BlockGroupDesc, Ext2Fs, Ext2Inode, Ext2Superblock};
use crate::block::{BlockDevice, BlockError};
use alloc::vec::Vec;
use super::directory_extension::Extension;
/// A copy of a directory's records, taken before an entry is added or removed,
/// so that only the blocks the change reaches are written back.
pub(super) fn snapshot(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(data.len())
        .map_err(|_| "No memory for directory data")?;
    copy.extend_from_slice(data);
    Ok(copy)
}

/// The device block of each block of directory `inode` whose records differ
/// between `before`, as on disk, and `after`, with the block's index. Adding or
/// removing an entry changes one block, so this is what a creation or removal
/// writes instead of the whole directory.
pub(super) fn plan<B: BlockDevice + ?Sized>(
    device: &B,
    inode: &Ext2Inode,
    sb: &Ext2Superblock,
    before: &[u8],
    after: &[u8],
) -> Result<Vec<(usize, u32)>, &'static str> {
    let size = check(inode, sb, before, after)?;
    let mut blocks = Vec::new();
    for (logical, _) in changed(size, before, after) {
        blocks.try_reserve(1)
            .map_err(|_| "No memory for directory write plan")?;
        blocks.push((logical, block_of(device, inode, sb, logical)?));
    }
    Ok(blocks)
}

/// Write `data`'s records for each block of `blocks`.
pub(super) fn write<B: BlockDevice + ?Sized>(
    device: &B,
    size: usize,
    blocks: &[(usize, u32)],
    data: &[u8],
) -> Result<(), &'static str> {
    if size == 0 || size > 4096 {
        return Err("Invalid directory write plan");
    }
    for &(logical, block) in blocks {
        let records = data
            .get(logical * size..)
            .map(|rest| &rest[..rest.len().min(size)])
            .filter(|records| !records.is_empty())
            .ok_or("Invalid directory write plan")?;
        write_block(device, size, block, records)?;
    }
    Ok(())
}

/// Write each block of directory `inode` whose records differ between `before`,
/// as on disk, and `after`, mapping each as it goes, so it allocates nothing.
pub(super) fn write_changes<B: BlockDevice + ?Sized>(
    device: &B,
    inode: &Ext2Inode,
    sb: &Ext2Superblock,
    before: &[u8],
    after: &[u8],
) -> Result<(), &'static str> {
    let size = check(inode, sb, before, after)?;
    for (logical, records) in changed(size, before, after) {
        write_block(device, size, block_of(device, inode, sb, logical)?, records)?;
    }
    Ok(())
}

fn check(inode: &Ext2Inode, sb: &Ext2Superblock, before: &[u8], after: &[u8]) -> Result<usize, &'static str> {
    if !inode.is_dir() {
        return Err("Not a directory");
    }
    let size = sb.block_size();
    if size == 0 || size > 4096 || before.len() != after.len() {
        return Err("Invalid directory write plan");
    }
    if after.len() as u64 > inode.size() {
        return Err("No space for directory data");
    }
    Ok(size)
}

/// Each block of `after` whose records differ from `before`, with its index.
fn changed<'a>(size: usize, before: &'a [u8], after: &'a [u8]) -> impl Iterator<Item = (usize, &'a [u8])> {
    before
        .chunks(size)
        .zip(after.chunks(size))
        .enumerate()
        .filter(|(_, (old, new))| old != new)
        .map(|(logical, (_, new))| (logical, new))
}

fn block_of<B: BlockDevice + ?Sized>(
    device: &B,
    inode: &Ext2Inode,
    sb: &Ext2Superblock,
    logical: usize,
) -> Result<u32, &'static str> {
    file::get_block_num(device, inode, sb, logical as u32)
        .map_err(|e| {
            if e == BlockError::OutOfMemory {
                "No memory for directory addressing"
            } else {
                "Failed to read directory addressing"
            }
        })?
        .ok_or("No space for directory data")
}

fn write_block<B: BlockDevice + ?Sized>(
    device: &B,
    size: usize,
    block: u32,
    records: &[u8],
) -> Result<(), &'static str> {
    let mut bytes = [0u8; 4096];
    bytes[..records.len()].copy_from_slice(records);
    file::write_ext2_block(device, block, size, &bytes[..size])
        .map_err(|_| "Failed to write directory block")
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
        let order = transaction.block_order(grown.i_block, logical, block)
            .map_err(|_| "Failed to verify directory addressing")?;
        if let Err(restored) = transaction.publish(&order) {
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
