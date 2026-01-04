//! ext2 block group descriptor structures
//!
//! Block groups divide the filesystem into manageable chunks, each with its own
//! inode and block bitmaps, inode table, and data blocks.

use crate::block::{BlockDevice, BlockError};
use crate::fs::ext2::Ext2Superblock;
use alloc::vec::Vec;
use core::mem;

/// Block group descriptor (32 bytes)
///
/// Each block group has a descriptor that points to its bitmaps, inode table,
/// and tracks free space statistics.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext2BlockGroupDesc {
    pub bg_block_bitmap: u32,        // Block bitmap block
    pub bg_inode_bitmap: u32,        // Inode bitmap block
    pub bg_inode_table: u32,         // Inode table start block
    pub bg_free_blocks_count: u16,   // Free blocks in group
    pub bg_free_inodes_count: u16,   // Free inodes in group
    pub bg_used_dirs_count: u16,     // Directories in group
    pub bg_pad: u16,
    _reserved: [u8; 12],
}

impl Ext2BlockGroupDesc {
    /// Read all block group descriptors from device
    ///
    /// The block group descriptor table starts immediately after the superblock.
    /// For 1024-byte blocks: superblock is in block 1, BGDT starts in block 2
    /// For 2048+ byte blocks: superblock is in block 0 (bytes 1024-2047), BGDT starts in block 1
    ///
    /// # Arguments
    /// * `device` - The block device to read from
    /// * `superblock` - The filesystem superblock (for calculating BGDT location)
    ///
    /// # Returns
    /// * `Ok(Vec<Ext2BlockGroupDesc>)` - Successfully read all block group descriptors
    /// * `Err(BlockError)` - I/O error during read
    pub fn read_table<B: BlockDevice>(
        device: &B,
        superblock: &Ext2Superblock,
    ) -> Result<Vec<Self>, BlockError> {
        let block_size = superblock.block_size();
        let bg_count = superblock.block_group_count() as usize;
        let descriptor_size = mem::size_of::<Ext2BlockGroupDesc>();
        
        // Calculate which ext2 block contains the BGDT
        // The BGDT starts right after the superblock
        let bgdt_block = if block_size == 1024 {
            // For 1024-byte blocks, superblock is in block 1, BGDT in block 2
            2
        } else {
            // For larger blocks, superblock is in block 0, BGDT in block 1
            1
        };
        
        // Calculate how many ext2 blocks we need to read for all descriptors
        let bytes_needed = bg_count * descriptor_size;
        let ext2_blocks_needed = (bytes_needed + block_size - 1) / block_size;
        
        // Read the necessary ext2 blocks
        // We need to convert ext2 blocks to device blocks
        let device_block_size = device.block_size();
        let mut buffer = Vec::new();
        buffer.resize(ext2_blocks_needed * block_size, 0u8);
        
        for i in 0..ext2_blocks_needed {
            let ext2_block_num = bgdt_block + i;
            
            // Convert ext2 block to device block(s)
            let device_blocks_per_ext2_block = block_size / device_block_size;
            let start_device_block = ext2_block_num * device_blocks_per_ext2_block;
            
            for j in 0..device_blocks_per_ext2_block {
                device.read_block(
                    (start_device_block + j) as u64,
                    &mut buffer[(i * block_size + j * device_block_size)
                        ..(i * block_size + (j + 1) * device_block_size)],
                )?;
            }
        }
        
        // Parse descriptors from buffer
        let mut descriptors = Vec::new();
        for i in 0..bg_count {
            let offset = i * descriptor_size;
            
            // SAFETY: Reading from aligned buffer into packed struct
            let desc: Ext2BlockGroupDesc = unsafe {
                core::ptr::read_unaligned(
                    buffer[offset..offset + descriptor_size].as_ptr() 
                        as *const Ext2BlockGroupDesc
                )
            };
            
            descriptors.push(desc);
        }
        
        Ok(descriptors)
    }

    /// Write all block group descriptors to device
    ///
    /// Writes the block group descriptor table back to disk after modifications.
    ///
    /// # Arguments
    /// * `device` - The block device to write to
    /// * `superblock` - The filesystem superblock (for calculating BGDT location)
    /// * `descriptors` - The block group descriptors to write
    ///
    /// # Returns
    /// * `Ok(())` - Successfully wrote all block group descriptors
    /// * `Err(BlockError)` - I/O error during write
    pub fn write_table<B: BlockDevice>(
        device: &B,
        superblock: &Ext2Superblock,
        descriptors: &[Self],
    ) -> Result<(), BlockError> {
        let block_size = superblock.block_size();
        let descriptor_size = mem::size_of::<Ext2BlockGroupDesc>();

        // Calculate which ext2 block contains the BGDT
        let bgdt_block = if block_size == 1024 {
            2
        } else {
            1
        };

        // Calculate how many ext2 blocks we need
        let bytes_needed = descriptors.len() * descriptor_size;
        let ext2_blocks_needed = (bytes_needed + block_size - 1) / block_size;

        // Build the buffer with all descriptors
        let device_block_size = device.block_size();
        let mut buffer = alloc::vec![0u8; ext2_blocks_needed * block_size];

        for (i, desc) in descriptors.iter().enumerate() {
            let offset = i * descriptor_size;

            // SAFETY: Writing packed struct as raw bytes
            let desc_bytes = unsafe {
                core::slice::from_raw_parts(
                    desc as *const Ext2BlockGroupDesc as *const u8,
                    descriptor_size,
                )
            };
            buffer[offset..offset + descriptor_size].copy_from_slice(desc_bytes);
        }

        // Write the buffer back to device
        for i in 0..ext2_blocks_needed {
            let ext2_block_num = bgdt_block + i;

            // Convert ext2 block to device block(s)
            let device_blocks_per_ext2_block = block_size / device_block_size;
            let start_device_block = ext2_block_num * device_blocks_per_ext2_block;

            for j in 0..device_blocks_per_ext2_block {
                device.write_block(
                    (start_device_block + j) as u64,
                    &buffer[(i * block_size + j * device_block_size)
                        ..(i * block_size + (j + 1) * device_block_size)],
                )?;
            }
        }

        Ok(())
    }
}

/// Allocate a new data block from the filesystem
///
/// Searches the block bitmaps to find a free block, marks it as used,
/// and returns the block number.
///
/// # Arguments
/// * `device` - The block device
/// * `superblock` - The ext2 superblock
/// * `block_groups` - Mutable reference to block group descriptors (to update free count)
///
/// # Returns
/// * `Ok(block_num)` - The allocated block number
/// * `Err(msg)` - Error if no free blocks available or I/O error
pub fn allocate_block<B: BlockDevice>(
    device: &B,
    superblock: &Ext2Superblock,
    block_groups: &mut [Ext2BlockGroupDesc],
) -> Result<u32, &'static str> {
    let block_size = superblock.block_size();
    let blocks_per_group = superblock.s_blocks_per_group;

    // Search each block group for a free block
    for (bg_index, bg) in block_groups.iter_mut().enumerate() {
        // Read free blocks count safely from packed struct
        let free_blocks = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_free_blocks_count))
        };

        if free_blocks == 0 {
            continue; // No free blocks in this group
        }

        // Read the block bitmap block
        let bitmap_block = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_block_bitmap))
        };
        let mut bitmap_buf = alloc::vec![0u8; block_size];
        device.read_block(bitmap_block as u64, &mut bitmap_buf)
            .map_err(|_| "Failed to read block bitmap")?;

        // Search for a free block in this group
        for local_block in 0..blocks_per_group {
            // Calculate the global block number
            let global_block = bg_index as u32 * blocks_per_group + local_block;

            // Check if this block is free (bit = 0)
            let byte_index = (local_block / 8) as usize;
            let bit_index = (local_block % 8) as u8;

            if byte_index >= bitmap_buf.len() {
                break; // Bitmap doesn't cover this block
            }

            if (bitmap_buf[byte_index] & (1 << bit_index)) == 0 {
                // Found a free block - mark it as used
                bitmap_buf[byte_index] |= 1 << bit_index;

                // Write the updated bitmap back to disk
                device.write_block(bitmap_block as u64, &bitmap_buf)
                    .map_err(|_| "Failed to write block bitmap")?;

                // Update the free block count in the block group descriptor
                // Safety: Writing to packed struct
                unsafe {
                    core::ptr::write_unaligned(
                        core::ptr::addr_of_mut!(bg.bg_free_blocks_count),
                        free_blocks - 1,
                    );
                }

                // Zero out the newly allocated block
                let zero_buf = alloc::vec![0u8; block_size];
                device.write_block(global_block as u64, &zero_buf)
                    .map_err(|_| "Failed to zero allocated block")?;

                return Ok(global_block);
            }
        }
    }

    Err("No free blocks available")
}

/// Free a data block in the block bitmap
///
/// Marks the block as free in the block bitmap and updates the
/// free block count in the block group descriptor.
///
/// # Arguments
/// * `device` - The block device
/// * `block_num` - Block number to free
/// * `superblock` - The ext2 superblock
/// * `block_groups` - Mutable reference to block group descriptors
///
/// # Returns
/// * `Ok(())` - Block was successfully freed
/// * `Err(msg)` - Error message if operation failed
pub fn free_block<B: BlockDevice>(
    device: &B,
    block_num: u32,
    superblock: &Ext2Superblock,
    block_groups: &mut [Ext2BlockGroupDesc],
) -> Result<(), &'static str> {
    let block_size = superblock.block_size();
    let blocks_per_group = superblock.s_blocks_per_group;

    // Calculate which block group contains this block
    let bg_index = (block_num / blocks_per_group) as usize;
    let local_block = block_num % blocks_per_group;

    if bg_index >= block_groups.len() {
        return Err("Block number out of range");
    }

    let bg = &mut block_groups[bg_index];

    // Read the block bitmap block
    let bitmap_block = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_block_bitmap))
    };
    let mut bitmap_buf = alloc::vec![0u8; block_size];
    device.read_block(bitmap_block as u64, &mut bitmap_buf)
        .map_err(|_| "Failed to read block bitmap")?;

    // Clear the bit for this block
    let byte_index = (local_block / 8) as usize;
    let bit_index = (local_block % 8) as u8;

    if byte_index < bitmap_buf.len() {
        bitmap_buf[byte_index] &= !(1 << bit_index);
    }

    // Write the updated bitmap back
    device.write_block(bitmap_block as u64, &bitmap_buf)
        .map_err(|_| "Failed to write block bitmap")?;

    // Update the free block count
    let free_blocks = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_free_blocks_count))
    };
    unsafe {
        core::ptr::write_unaligned(
            core::ptr::addr_of_mut!(bg.bg_free_blocks_count),
            free_blocks + 1,
        );
    }

    Ok(())
}
