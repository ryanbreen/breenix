//! Prepare allocated directory blocks before publishing namespace bytes.
use super::{file, Ext2Inode, Ext2Superblock};
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
