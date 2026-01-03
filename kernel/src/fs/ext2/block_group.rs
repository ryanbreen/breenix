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
}
