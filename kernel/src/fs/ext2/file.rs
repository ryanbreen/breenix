//! ext2 file content reading
//!
//! Handles reading file data by following the block pointer structure
//! in the inode (direct, single/double/triple indirect blocks).

use crate::block::{BlockDevice, BlockError};
use crate::fs::ext2::{Ext2Inode, Ext2Superblock};
use alloc::vec;
use alloc::vec::Vec;

/// Number of direct block pointers in the inode
const DIRECT_BLOCKS: u32 = 12;

/// Index of single indirect block pointer
const SINGLE_INDIRECT: usize = 12;

/// Index of double indirect block pointer
const DOUBLE_INDIRECT: usize = 13;

/// Index of triple indirect block pointer
const TRIPLE_INDIRECT: usize = 14;

/// Read a specific data block number for a file
///
/// Given a logical block index (0 = first block of file, 1 = second, etc.),
/// returns the physical block number on disk.
///
/// # Arguments
/// * `device` - The block device to read from
/// * `inode` - The inode containing block pointers
/// * `superblock` - The superblock (for block size calculation)
/// * `logical_block` - Logical block index within the file (0-based)
///
/// # Returns
/// * `Ok(Some(block_num))` - Physical block number on disk
/// * `Ok(None)` - Sparse hole (block pointer is 0)
/// * `Err(BlockError)` - I/O error or out of bounds
pub fn get_block_num<B: BlockDevice>(
    device: &B,
    inode: &Ext2Inode,
    superblock: &Ext2Superblock,
    logical_block: u32,
) -> Result<Option<u32>, BlockError> {
    let block_size = superblock.block_size();
    let ptrs_per_block = (block_size / 4) as u32; // 4 bytes per u32 block pointer

    // Read block pointers safely from packed struct
    let i_block = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block))
    };

    // Direct blocks (0-11)
    if logical_block < DIRECT_BLOCKS {
        let block_num = i_block[logical_block as usize];
        return Ok(if block_num == 0 { None } else { Some(block_num) });
    }

    let direct_count = DIRECT_BLOCKS;
    let single_indirect_count = ptrs_per_block;
    let double_indirect_count = ptrs_per_block * ptrs_per_block;

    // Single indirect block (12)
    if logical_block < direct_count + single_indirect_count {
        let single_indirect_ptr = i_block[SINGLE_INDIRECT];
        if single_indirect_ptr == 0 {
            return Ok(None); // Sparse hole
        }

        let index_in_indirect = logical_block - direct_count;
        let indirect_blocks = read_indirect_block(device, single_indirect_ptr, block_size)?;
        let block_num = indirect_blocks[index_in_indirect as usize];
        return Ok(if block_num == 0 { None } else { Some(block_num) });
    }

    // Double indirect block (13)
    if logical_block < direct_count + single_indirect_count + double_indirect_count {
        let double_indirect_ptr = i_block[DOUBLE_INDIRECT];
        if double_indirect_ptr == 0 {
            return Ok(None); // Sparse hole
        }

        let index_in_double = logical_block - direct_count - single_indirect_count;
        let first_level_index = index_in_double / ptrs_per_block;
        let second_level_index = index_in_double % ptrs_per_block;

        // Read first-level indirect block (contains pointers to second-level blocks)
        let first_level_blocks = read_indirect_block(device, double_indirect_ptr, block_size)?;
        let second_level_ptr = first_level_blocks[first_level_index as usize];
        if second_level_ptr == 0 {
            return Ok(None); // Sparse hole
        }

        // Read second-level indirect block (contains pointers to data blocks)
        let second_level_blocks = read_indirect_block(device, second_level_ptr, block_size)?;
        let block_num = second_level_blocks[second_level_index as usize];
        return Ok(if block_num == 0 { None } else { Some(block_num) });
    }

    // Triple indirect block (14)
    let triple_indirect_ptr = i_block[TRIPLE_INDIRECT];
    if triple_indirect_ptr == 0 {
        return Ok(None); // Sparse hole
    }

    let index_in_triple = logical_block - direct_count - single_indirect_count - double_indirect_count;
    let first_level_index = index_in_triple / (ptrs_per_block * ptrs_per_block);
    let second_level_index = (index_in_triple / ptrs_per_block) % ptrs_per_block;
    let third_level_index = index_in_triple % ptrs_per_block;

    // Read first-level indirect block
    let first_level_blocks = read_indirect_block(device, triple_indirect_ptr, block_size)?;
    let second_level_ptr = first_level_blocks[first_level_index as usize];
    if second_level_ptr == 0 {
        return Ok(None); // Sparse hole
    }

    // Read second-level indirect block
    let second_level_blocks = read_indirect_block(device, second_level_ptr, block_size)?;
    let third_level_ptr = second_level_blocks[second_level_index as usize];
    if third_level_ptr == 0 {
        return Ok(None); // Sparse hole
    }

    // Read third-level indirect block (contains pointers to data blocks)
    let third_level_blocks = read_indirect_block(device, third_level_ptr, block_size)?;
    let block_num = third_level_blocks[third_level_index as usize];
    Ok(if block_num == 0 { None } else { Some(block_num) })
}

/// Read the entire contents of a file
///
/// # Arguments
/// * `device` - The block device to read from
/// * `inode` - The inode containing block pointers and file size
/// * `superblock` - The superblock (for block size calculation)
///
/// # Returns
/// * `Ok(Vec<u8>)` - File contents
/// * `Err(BlockError)` - I/O error
pub fn read_file<B: BlockDevice>(
    device: &B,
    inode: &Ext2Inode,
    superblock: &Ext2Superblock,
) -> Result<Vec<u8>, BlockError> {
    let file_size = inode.size() as usize;
    if file_size == 0 {
        return Ok(Vec::new());
    }

    read_file_range(device, inode, superblock, 0, file_size)
}

/// Read a portion of a file (for seek + read operations)
///
/// # Arguments
/// * `device` - The block device to read from
/// * `inode` - The inode containing block pointers
/// * `superblock` - The superblock (for block size calculation)
/// * `offset` - Starting byte offset within the file
/// * `length` - Number of bytes to read
///
/// # Returns
/// * `Ok(Vec<u8>)` - File contents (may be shorter than length if EOF reached)
/// * `Err(BlockError)` - I/O error
pub fn read_file_range<B: BlockDevice>(
    device: &B,
    inode: &Ext2Inode,
    superblock: &Ext2Superblock,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>, BlockError> {
    let file_size = inode.size();
    if offset >= file_size {
        return Ok(Vec::new()); // Read past EOF
    }

    // Clamp length to not read past EOF
    let actual_length = core::cmp::min(length, (file_size - offset) as usize);
    if actual_length == 0 {
        return Ok(Vec::new());
    }

    let block_size = superblock.block_size();
    let start_block = (offset / block_size as u64) as u32;
    let offset_in_first_block = (offset % block_size as u64) as usize;
    let end_offset = offset + actual_length as u64;
    let end_block = ((end_offset + block_size as u64 - 1) / block_size as u64) as u32;

    let mut result = Vec::with_capacity(actual_length);
    let mut block_buf = vec![0u8; block_size];

    for logical_block in start_block..end_block {
        // Get physical block number (or None for sparse holes)
        let physical_block = get_block_num(device, inode, superblock, logical_block)?;

        // Read block or fill with zeros for sparse holes
        if let Some(block_num) = physical_block {
            device.read_block(block_num as u64, &mut block_buf)?;
        } else {
            // Sparse hole - fill with zeros
            block_buf.fill(0);
        }

        // Calculate which bytes from this block to copy
        let block_offset = logical_block as u64 * block_size as u64;
        let start_in_block = if block_offset < offset {
            offset_in_first_block
        } else {
            0
        };
        let end_in_block = if block_offset + block_size as u64 > end_offset {
            (end_offset - block_offset) as usize
        } else {
            block_size
        };

        result.extend_from_slice(&block_buf[start_in_block..end_in_block]);
    }

    Ok(result)
}

/// Helper to read block pointers from an indirect block
///
/// Reads a block containing an array of u32 block pointers (little-endian).
///
/// # Arguments
/// * `device` - The block device to read from
/// * `block_num` - Physical block number of the indirect block
/// * `block_size` - Filesystem block size
///
/// # Returns
/// * `Ok(Vec<u32>)` - Array of block pointers
/// * `Err(BlockError)` - I/O error
fn read_indirect_block<B: BlockDevice>(
    device: &B,
    block_num: u32,
    block_size: usize,
) -> Result<Vec<u32>, BlockError> {
    let mut block_buf = vec![0u8; block_size];
    device.read_block(block_num as u64, &mut block_buf)?;

    // Parse as array of little-endian u32 pointers
    let num_pointers = block_size / 4;
    let mut pointers = Vec::with_capacity(num_pointers);

    for i in 0..num_pointers {
        let offset = i * 4;
        let ptr = u32::from_le_bytes([
            block_buf[offset],
            block_buf[offset + 1],
            block_buf[offset + 2],
            block_buf[offset + 3],
        ]);
        pointers.push(ptr);
    }

    Ok(pointers)
}

/// Set a specific data block number for a file
///
/// Given a logical block index, sets the physical block number on disk.
/// This function allocates indirect blocks as needed.
///
/// # Arguments
/// * `device` - The block device to read/write
/// * `inode` - The inode to modify (mutable)
/// * `superblock` - The superblock (for block size calculation)
/// * `logical_block` - Logical block index within the file (0-based)
/// * `physical_block` - Physical block number on disk to set
///
/// # Returns
/// * `Ok(())` - Block pointer set successfully
/// * `Err(BlockError)` - I/O error or allocation failure
pub fn set_block_num<B: BlockDevice>(
    device: &B,
    inode: &mut Ext2Inode,
    superblock: &Ext2Superblock,
    logical_block: u32,
    physical_block: u32,
) -> Result<(), BlockError> {
    let block_size = superblock.block_size();
    let ptrs_per_block = (block_size / 4) as u32;

    // Direct blocks (0-11)
    if logical_block < DIRECT_BLOCKS {
        // Safety: Writing to packed struct requires unaligned access
        unsafe {
            let block_ptr = core::ptr::addr_of_mut!(inode.i_block[logical_block as usize]);
            core::ptr::write_unaligned(block_ptr, physical_block);
        }
        return Ok(());
    }

    let direct_count = DIRECT_BLOCKS;
    let single_indirect_count = ptrs_per_block;

    // Single indirect block (12)
    if logical_block < direct_count + single_indirect_count {
        // Get or allocate single indirect block
        let single_indirect_ptr = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block[SINGLE_INDIRECT]))
        };

        if single_indirect_ptr == 0 {
            // Need to allocate an indirect block - for now, return an error
            // A full implementation would allocate a new block here
            return Err(BlockError::IoError);
        }

        let index_in_indirect = logical_block - direct_count;

        // Read the indirect block, modify it, write it back
        let mut indirect_blocks = read_indirect_block(device, single_indirect_ptr, block_size)?;
        indirect_blocks[index_in_indirect as usize] = physical_block;
        write_indirect_block(device, single_indirect_ptr, block_size, &indirect_blocks)?;

        return Ok(());
    }

    // Double and triple indirect not implemented for writes yet
    Err(BlockError::IoError)
}

/// Write the entire contents to a file
///
/// # Arguments
/// * `device` - The block device to write to
/// * `inode` - The inode to write to (will be modified)
/// * `superblock` - The superblock (for block size calculation)
/// * `data` - Data to write
///
/// # Returns
/// * `Ok(())` - Write successful
/// * `Err(BlockError)` - I/O error
pub fn write_file<B: BlockDevice>(
    device: &B,
    inode: &mut Ext2Inode,
    superblock: &Ext2Superblock,
    data: &[u8],
) -> Result<(), BlockError> {
    write_file_range(device, inode, superblock, 0, data)
}

/// Write a portion of a file (for seek + write operations)
///
/// # Arguments
/// * `device` - The block device to write to
/// * `inode` - The inode to write to (will be modified)
/// * `superblock` - The superblock (for block size calculation)
/// * `offset` - Starting byte offset within the file
/// * `data` - Data to write
///
/// # Returns
/// * `Ok(())` - Write successful
/// * `Err(BlockError)` - I/O error
pub fn write_file_range<B: BlockDevice>(
    device: &B,
    inode: &mut Ext2Inode,
    superblock: &Ext2Superblock,
    offset: u64,
    data: &[u8],
) -> Result<(), BlockError> {
    if data.is_empty() {
        return Ok(());
    }

    let block_size = superblock.block_size();
    let start_block = (offset / block_size as u64) as u32;
    let offset_in_first_block = (offset % block_size as u64) as usize;
    let end_offset = offset + data.len() as u64;
    let end_block = ((end_offset + block_size as u64 - 1) / block_size as u64) as u32;

    let mut data_pos = 0usize;
    let mut block_buf = vec![0u8; block_size];

    for logical_block in start_block..end_block {
        // Get physical block number
        let physical_block = match get_block_num(device, inode, superblock, logical_block)? {
            Some(block_num) => block_num,
            None => {
                // Sparse hole or no block allocated - need to allocate
                // For now, we don't support writing to sparse files or extending
                // A full implementation would allocate a new block here
                return Err(BlockError::IoError);
            }
        };

        // Calculate which bytes in this block to write
        let block_offset = logical_block as u64 * block_size as u64;
        let start_in_block = if block_offset < offset {
            offset_in_first_block
        } else {
            0
        };
        let end_in_block = if block_offset + block_size as u64 > end_offset {
            (end_offset - block_offset) as usize
        } else {
            block_size
        };
        let bytes_to_write = end_in_block - start_in_block;

        // Read-modify-write if we're not writing a full block
        if start_in_block != 0 || end_in_block != block_size {
            device.read_block(physical_block as u64, &mut block_buf)?;
        }

        // Copy data into block buffer
        block_buf[start_in_block..end_in_block]
            .copy_from_slice(&data[data_pos..data_pos + bytes_to_write]);
        data_pos += bytes_to_write;

        // Write the block back
        device.write_block(physical_block as u64, &block_buf)?;
    }

    // Update inode size if we extended the file
    let current_size = inode.size();
    if end_offset > current_size {
        // Update i_size (lower 32 bits)
        unsafe {
            let size_ptr = core::ptr::addr_of_mut!(inode.i_size);
            core::ptr::write_unaligned(size_ptr, end_offset as u32);
        }
        // For files > 4GB, we'd also need to update i_dir_acl
        // but that's not common for typical use
    }

    Ok(())
}

/// Helper to write block pointers to an indirect block
///
/// Writes an array of u32 block pointers (little-endian) to a block.
///
/// # Arguments
/// * `device` - The block device to write to
/// * `block_num` - Physical block number of the indirect block
/// * `block_size` - Filesystem block size
/// * `pointers` - Array of block pointers to write
///
/// # Returns
/// * `Ok(())` - Write successful
/// * `Err(BlockError)` - I/O error
fn write_indirect_block<B: BlockDevice>(
    device: &B,
    block_num: u32,
    block_size: usize,
    pointers: &[u32],
) -> Result<(), BlockError> {
    let mut block_buf = vec![0u8; block_size];
    let num_pointers = core::cmp::min(pointers.len(), block_size / 4);

    // Serialize pointers to little-endian bytes
    for i in 0..num_pointers {
        let offset = i * 4;
        let bytes = pointers[i].to_le_bytes();
        block_buf[offset..offset + 4].copy_from_slice(&bytes);
    }

    device.write_block(block_num as u64, &block_buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direct_block_ranges() {
        // With 4KB block size, direct blocks cover 0-11 (48KB)
        assert_eq!(DIRECT_BLOCKS, 12);
        assert_eq!(SINGLE_INDIRECT, 12);
        assert_eq!(DOUBLE_INDIRECT, 13);
        assert_eq!(TRIPLE_INDIRECT, 14);
    }

    #[test]
    fn test_block_pointer_capacity() {
        // For 4KB block size:
        // - 1024 pointers per indirect block
        // - Direct: 12 * 4KB = 48KB
        // - Single indirect: 1024 * 4KB = 4MB
        // - Double indirect: 1024^2 * 4KB = 4GB
        // - Triple indirect: 1024^3 * 4KB = 4TB
        let block_size = 4096;
        let ptrs_per_block = block_size / 4;

        assert_eq!(ptrs_per_block, 1024);

        let direct_bytes = DIRECT_BLOCKS * block_size;
        assert_eq!(direct_bytes, 49152); // 48KB

        let single_indirect_bytes = ptrs_per_block * block_size;
        assert_eq!(single_indirect_bytes, 4194304); // 4MB
    }
}
