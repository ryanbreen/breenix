//! ext2 block group descriptor structures
//!
//! Block groups divide the filesystem into manageable chunks, each with its own
//! inode and block bitmaps, inode table, and data blocks.

use crate::block::{BlockDevice, BlockError};
use crate::fs::ext2::Ext2Superblock;
use crate::fs::ext2::file::{read_ext2_block, write_ext2_block};
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
        // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
        let bitmap_block = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_block_bitmap))
        };
        let mut bitmap_buf = [0u8; 4096]; // Max block size
        read_ext2_block(device, bitmap_block, block_size, &mut bitmap_buf[..block_size])
            .map_err(|_| "Failed to read block bitmap")?;

        // Search for a free block in this group
        // s_first_data_block is the first data block in the filesystem (usually 1 for 1KB blocks)
        let first_data_block = superblock.s_first_data_block;
        for local_block in 0..blocks_per_group {
            // Calculate the global block number
            // Bitmap bit N corresponds to block (s_first_data_block + bg_index * blocks_per_group + N)
            let global_block = first_data_block + bg_index as u32 * blocks_per_group + local_block;

            // Check if this block is free (bit = 0)
            let byte_index = (local_block / 8) as usize;
            let bit_index = (local_block % 8) as u8;

            if byte_index >= block_size {
                break; // Bitmap doesn't cover this block
            }

            if (bitmap_buf[byte_index] & (1 << bit_index)) == 0 {
                // Found a free block - mark it as used
                bitmap_buf[byte_index] |= 1 << bit_index;

                // Write the updated bitmap back to disk
                write_ext2_block(device, bitmap_block, block_size, &bitmap_buf[..block_size])
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
                let zero_buf = [0u8; 4096]; // Max block size
                write_ext2_block(device, global_block, block_size, &zero_buf[..block_size])
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
    let first_data_block = superblock.s_first_data_block;

    // Block number must be >= s_first_data_block
    if block_num < first_data_block {
        return Err("Invalid block number (below first data block)");
    }

    // Calculate which block group contains this block
    // Subtract s_first_data_block before dividing/modding
    let adjusted_block = block_num - first_data_block;
    let bg_index = (adjusted_block / blocks_per_group) as usize;
    let local_block = adjusted_block % blocks_per_group;

    if bg_index >= block_groups.len() {
        return Err("Block number out of range");
    }

    let bg = &mut block_groups[bg_index];

    // Read the block bitmap block
    // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
    let bitmap_block = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_block_bitmap))
    };
    let mut bitmap_buf = [0u8; 4096]; // Max block size
    read_ext2_block(device, bitmap_block, block_size, &mut bitmap_buf[..block_size])
        .map_err(|_| "Failed to read block bitmap")?;

    // Clear the bit for this block
    let byte_index = (local_block / 8) as usize;
    let bit_index = (local_block % 8) as u8;

    if byte_index < block_size {
        bitmap_buf[byte_index] &= !(1 << bit_index);
    }

    // Write the updated bitmap back
    write_ext2_block(device, bitmap_block, block_size, &bitmap_buf[..block_size])
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use spin::Mutex;

    /// Mock block device for testing
    /// Stores data in memory with controllable bitmap content
    struct MockBlockDevice {
        data: Mutex<Vec<u8>>,
        device_block_size: usize,
    }

    impl MockBlockDevice {
        fn new(size: usize, device_block_size: usize) -> Self {
            Self {
                data: Mutex::new(vec![0u8; size]),
                device_block_size,
            }
        }

        /// Set a byte in the device data
        fn set_byte(&self, offset: usize, value: u8) {
            let mut data = self.data.lock();
            if offset < data.len() {
                data[offset] = value;
            }
        }

        /// Get a byte from the device data
        fn get_byte(&self, offset: usize) -> u8 {
            let data = self.data.lock();
            if offset < data.len() {
                data[offset]
            } else {
                0
            }
        }
    }

    impl BlockDevice for MockBlockDevice {
        fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
            let offset = block_num as usize * self.device_block_size;
            let data = self.data.lock();
            let end = offset + buf.len().min(self.device_block_size);
            if end <= data.len() {
                buf[..end - offset].copy_from_slice(&data[offset..end]);
                Ok(())
            } else {
                Err(BlockError::OutOfBounds)
            }
        }

        fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
            let offset = block_num as usize * self.device_block_size;
            let mut data = self.data.lock();
            let len = buf.len().min(self.device_block_size);
            let end = offset + len;
            if end <= data.len() {
                data[offset..end].copy_from_slice(&buf[..len]);
                Ok(())
            } else {
                Err(BlockError::OutOfBounds)
            }
        }

        fn block_size(&self) -> usize {
            self.device_block_size
        }

        fn num_blocks(&self) -> u64 {
            let data = self.data.lock();
            (data.len() / self.device_block_size) as u64
        }

        fn flush(&self) -> Result<(), BlockError> {
            Ok(())
        }
    }

    /// Create a mock superblock for 1KB blocks (s_first_data_block = 1)
    fn create_1kb_superblock() -> Ext2Superblock {
        let mut bytes = [0u8; 1024];

        // s_inodes_count (offset 0): 256
        bytes[0..4].copy_from_slice(&256u32.to_le_bytes());
        // s_blocks_count (offset 4): 1024
        bytes[4..8].copy_from_slice(&1024u32.to_le_bytes());
        // s_free_blocks_count (offset 12): 900
        bytes[12..16].copy_from_slice(&900u32.to_le_bytes());
        // s_free_inodes_count (offset 16): 200
        bytes[16..20].copy_from_slice(&200u32.to_le_bytes());
        // s_first_data_block (offset 20): 1 for 1KB blocks
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        // s_log_block_size (offset 24): 0 means 1024 byte blocks
        bytes[24..28].copy_from_slice(&0u32.to_le_bytes());
        // s_blocks_per_group (offset 32): 8192
        bytes[32..36].copy_from_slice(&8192u32.to_le_bytes());
        // s_inodes_per_group (offset 40): 256
        bytes[40..44].copy_from_slice(&256u32.to_le_bytes());
        // s_magic (offset 56): ext2 magic
        bytes[56..58].copy_from_slice(&0xEF53u16.to_le_bytes());
        // s_rev_level (offset 76): 1
        bytes[76..80].copy_from_slice(&1u32.to_le_bytes());
        // s_inode_size (offset 88): 128
        bytes[88..90].copy_from_slice(&128u16.to_le_bytes());

        Ext2Superblock::from_bytes(&bytes).unwrap()
    }

    /// Create a mock superblock for 4KB blocks (s_first_data_block = 0)
    fn create_4kb_superblock() -> Ext2Superblock {
        let mut bytes = [0u8; 1024];

        // s_inodes_count
        bytes[0..4].copy_from_slice(&256u32.to_le_bytes());
        // s_blocks_count
        bytes[4..8].copy_from_slice(&1024u32.to_le_bytes());
        // s_free_blocks_count
        bytes[12..16].copy_from_slice(&900u32.to_le_bytes());
        // s_free_inodes_count
        bytes[16..20].copy_from_slice(&200u32.to_le_bytes());
        // s_first_data_block: 0 for 4KB blocks
        bytes[20..24].copy_from_slice(&0u32.to_le_bytes());
        // s_log_block_size: 2 means 4096 byte blocks
        bytes[24..28].copy_from_slice(&2u32.to_le_bytes());
        // s_blocks_per_group
        bytes[32..36].copy_from_slice(&8192u32.to_le_bytes());
        // s_inodes_per_group
        bytes[40..44].copy_from_slice(&256u32.to_le_bytes());
        // s_magic
        bytes[56..58].copy_from_slice(&0xEF53u16.to_le_bytes());
        // s_rev_level
        bytes[76..80].copy_from_slice(&1u32.to_le_bytes());
        // s_inode_size
        bytes[88..90].copy_from_slice(&128u16.to_le_bytes());

        Ext2Superblock::from_bytes(&bytes).unwrap()
    }

    /// Create a block group descriptor with specified bitmap block and free count
    fn create_block_group(bitmap_block: u32, free_blocks: u16) -> Ext2BlockGroupDesc {
        let mut bg = Ext2BlockGroupDesc {
            bg_block_bitmap: bitmap_block,
            bg_inode_bitmap: 0,
            bg_inode_table: 0,
            bg_free_blocks_count: free_blocks,
            bg_free_inodes_count: 0,
            bg_used_dirs_count: 0,
            bg_pad: 0,
            _reserved: [0u8; 12],
        };
        // Write to packed struct safely
        unsafe {
            core::ptr::write_unaligned(
                core::ptr::addr_of_mut!(bg.bg_block_bitmap),
                bitmap_block,
            );
            core::ptr::write_unaligned(
                core::ptr::addr_of_mut!(bg.bg_free_blocks_count),
                free_blocks,
            );
        }
        bg
    }

    // =============================================================
    // CRITICAL: Block allocation arithmetic tests
    // These tests verify the s_first_data_block offset fix in d190da5
    // =============================================================

    #[test]
    fn test_allocate_block_1kb_first_data_block_offset() {
        // For ext2 with 1KB blocks, s_first_data_block = 1
        // Bitmap bit 0 represents block 1, not block 0
        // This test verifies the fix in commit d190da5

        let superblock = create_1kb_superblock();
        assert_eq!(superblock.s_first_data_block, 1, "1KB blocks should have s_first_data_block = 1");

        // Create a mock device with:
        // - 1KB device blocks
        // - Bitmap at ext2 block 3 (bytes 3072-4095)
        // - All bitmap bits initially free (0)
        let device = MockBlockDevice::new(32 * 1024, 1024); // 32KB total

        // Block group descriptor with bitmap at block 3
        let mut block_groups = vec![create_block_group(3, 100)];

        // Allocate a block - should return block 1 (s_first_data_block + 0)
        // NOT block 0 (which would be wrong)
        let result = allocate_block(&device, &superblock, &mut block_groups);
        assert!(result.is_ok(), "allocate_block should succeed");

        let allocated = result.unwrap();
        assert_eq!(
            allocated, 1,
            "First allocated block should be 1 (s_first_data_block), not 0. \
             Bitmap bit 0 maps to block 1 for 1KB filesystems."
        );
    }

    #[test]
    fn test_allocate_block_4kb_no_offset() {
        // For ext2 with 4KB blocks, s_first_data_block = 0
        // Bitmap bit 0 represents block 0
        // This is the simpler case

        let superblock = create_4kb_superblock();
        assert_eq!(superblock.s_first_data_block, 0, "4KB blocks should have s_first_data_block = 0");

        // Create a mock device with 4KB blocks
        let device = MockBlockDevice::new(128 * 1024, 4096); // 128KB total

        // Block group descriptor with bitmap at block 1
        let mut block_groups = vec![create_block_group(1, 100)];

        let result = allocate_block(&device, &superblock, &mut block_groups);
        assert!(result.is_ok(), "allocate_block should succeed");

        let allocated = result.unwrap();
        assert_eq!(
            allocated, 0,
            "First allocated block should be 0 for 4KB filesystems (s_first_data_block = 0)"
        );
    }

    #[test]
    fn test_allocate_block_with_some_bits_used() {
        // Test allocation when first few blocks are already allocated
        // For 1KB blocks: if bitmap bits 0-4 are used, next allocation should be block 6
        // (s_first_data_block=1 + local_block=5 = 6)

        let superblock = create_1kb_superblock();
        let device = MockBlockDevice::new(32 * 1024, 1024);

        // Set bitmap byte 0 = 0x1F (bits 0-4 set, meaning blocks 1-5 are used)
        // Bitmap is at ext2 block 3, which starts at byte 3072
        device.set_byte(3072, 0x1F);

        let mut block_groups = vec![create_block_group(3, 95)];

        let result = allocate_block(&device, &superblock, &mut block_groups);
        assert!(result.is_ok(), "allocate_block should succeed");

        let allocated = result.unwrap();
        assert_eq!(
            allocated, 6,
            "With bits 0-4 used, next block should be 6 (s_first_data_block=1 + bit_index=5)"
        );
    }

    #[test]
    fn test_allocate_block_bit_295_returns_block_296() {
        // This test specifically targets the bug scenario from the handoff:
        // For 1KB blocks, when bitmap bit 295 is free, allocate_block should
        // return block 296 (not 295)

        let superblock = create_1kb_superblock();
        let device = MockBlockDevice::new(64 * 1024, 1024);

        // Set all bitmap bytes 0-36 to 0xFF (bits 0-295 used)
        // Bitmap at ext2 block 3 starts at byte 3072
        for i in 0..37 {
            device.set_byte(3072 + i, 0xFF);
        }
        // Byte 36 covers bits 288-295, byte 37 covers bits 296-303
        // Clear bit 295 in byte 36 (bit 7): 0xFF -> 0x7F
        device.set_byte(3072 + 36, 0x7F);

        let mut block_groups = vec![create_block_group(3, 1)];

        let result = allocate_block(&device, &superblock, &mut block_groups);
        assert!(result.is_ok(), "allocate_block should succeed");

        let allocated = result.unwrap();
        assert_eq!(
            allocated, 296,
            "Bitmap bit 295 should map to block 296 (s_first_data_block=1 + 295). \
             This was the bug: it incorrectly returned 295."
        );
    }

    // =============================================================
    // CRITICAL: Block free arithmetic tests
    // These tests verify the s_first_data_block offset fix in free_block
    // =============================================================

    #[test]
    fn test_free_block_1kb_offset() {
        // When freeing block 296 on a 1KB filesystem, we should clear
        // bitmap bit 295 (not bit 296)

        let superblock = create_1kb_superblock();
        let device = MockBlockDevice::new(64 * 1024, 1024);

        // Set all bitmap bits as used initially
        for i in 0..128 {
            device.set_byte(3072 + i, 0xFF);
        }

        let mut block_groups = vec![create_block_group(3, 0)];

        // Free block 296
        let result = free_block(&device, 296, &superblock, &mut block_groups);
        assert!(result.is_ok(), "free_block should succeed");

        // Check that bitmap bit 295 is now clear (block 296 = s_first_data_block + 295)
        // Bit 295 is in byte 36, bit 7
        let bitmap_byte = device.get_byte(3072 + 36);
        assert_eq!(
            bitmap_byte & 0x80, 0,
            "Bitmap bit 295 (byte 36, bit 7) should be clear after freeing block 296. \
             Block 296 - s_first_data_block(1) = 295"
        );

        // Verify other bits are still set
        assert_eq!(
            bitmap_byte & 0x7F, 0x7F,
            "Other bits in byte 36 should remain set"
        );
    }

    #[test]
    fn test_free_block_4kb_no_offset() {
        // For 4KB blocks (s_first_data_block = 0), freeing block 5
        // should clear bitmap bit 5

        let superblock = create_4kb_superblock();
        let device = MockBlockDevice::new(128 * 1024, 4096);

        // Bitmap at block 1 (bytes 4096-8191 for 4KB blocks)
        // Set first bitmap byte to 0xFF (all bits used)
        device.set_byte(4096, 0xFF);

        let mut block_groups = vec![create_block_group(1, 0)];

        // Free block 5
        let result = free_block(&device, 5, &superblock, &mut block_groups);
        assert!(result.is_ok(), "free_block should succeed");

        // Check that bitmap bit 5 is now clear
        let bitmap_byte = device.get_byte(4096);
        assert_eq!(
            bitmap_byte & (1 << 5), 0,
            "Bitmap bit 5 should be clear after freeing block 5 (s_first_data_block=0)"
        );
        assert_eq!(
            bitmap_byte & !(1 << 5), 0xFF & !(1 << 5),
            "Other bits should remain set"
        );
    }

    #[test]
    fn test_free_block_below_first_data_block_fails() {
        // Freeing a block below s_first_data_block should fail

        let superblock = create_1kb_superblock();
        let device = MockBlockDevice::new(32 * 1024, 1024);
        let mut block_groups = vec![create_block_group(3, 100)];

        // Try to free block 0 (which is below s_first_data_block = 1)
        let result = free_block(&device, 0, &superblock, &mut block_groups);
        assert!(result.is_err(), "Freeing block 0 should fail when s_first_data_block = 1");
    }

    // =============================================================
    // Allocate-free round-trip tests
    // Verify that allocating then freeing returns to the original state
    // =============================================================

    #[test]
    fn test_allocate_free_roundtrip() {
        // Allocate a block, free it, allocate again - should get same block

        let superblock = create_1kb_superblock();
        let device = MockBlockDevice::new(32 * 1024, 1024);
        let mut block_groups = vec![create_block_group(3, 100)];

        // First allocation
        let block1 = allocate_block(&device, &superblock, &mut block_groups)
            .expect("First allocation should succeed");

        // Free it
        free_block(&device, block1, &superblock, &mut block_groups)
            .expect("Free should succeed");

        // Allocate again - should get the same block back
        let block2 = allocate_block(&device, &superblock, &mut block_groups)
            .expect("Second allocation should succeed");

        assert_eq!(
            block1, block2,
            "After freeing, next allocation should return the same block"
        );
    }

    #[test]
    fn test_free_block_count_updates() {
        // Verify that bg_free_blocks_count is updated correctly

        let superblock = create_1kb_superblock();
        let device = MockBlockDevice::new(32 * 1024, 1024);
        let mut block_groups = vec![create_block_group(3, 100)];

        let initial_free = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(block_groups[0].bg_free_blocks_count))
        };

        // Allocate a block
        let block = allocate_block(&device, &superblock, &mut block_groups)
            .expect("Allocation should succeed");

        let after_alloc = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(block_groups[0].bg_free_blocks_count))
        };
        assert_eq!(
            after_alloc, initial_free - 1,
            "Free count should decrease by 1 after allocation"
        );

        // Free the block
        free_block(&device, block, &superblock, &mut block_groups)
            .expect("Free should succeed");

        let after_free = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(block_groups[0].bg_free_blocks_count))
        };
        assert_eq!(
            after_free, initial_free,
            "Free count should return to initial value after freeing"
        );
    }

    // =============================================================
    // CRITICAL: Truncate block freeing test
    // This test verifies that freeing multiple blocks (as truncate_file does)
    // actually increases bg_free_blocks_count - catching the bug where
    // truncate_file only cleared i_blocks but didn't call free_block()
    // =============================================================

    #[test]
    fn test_truncate_frees_blocks_to_bitmap() {
        // This test simulates what truncate_file should do:
        // 1. A file has multiple allocated blocks (direct blocks 0-2)
        // 2. truncate_file calls free_block for each allocated block
        // 3. bg_free_blocks_count should increase by the number of blocks freed
        //
        // If truncate_file only sets i_blocks=0 without calling free_block,
        // the bg_free_blocks_count would NOT increase, and this test would fail.

        let superblock = create_1kb_superblock();
        let device = MockBlockDevice::new(32 * 1024, 1024);

        // Start with 100 free blocks
        let mut block_groups = vec![create_block_group(3, 100)];

        // Simulate file creation: allocate 3 blocks (like a ~3KB file)
        let block1 = allocate_block(&device, &superblock, &mut block_groups)
            .expect("Allocate block 1 should succeed");
        let block2 = allocate_block(&device, &superblock, &mut block_groups)
            .expect("Allocate block 2 should succeed");
        let block3 = allocate_block(&device, &superblock, &mut block_groups)
            .expect("Allocate block 3 should succeed");

        // After allocating 3 blocks, free count should be 97
        let after_alloc = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(block_groups[0].bg_free_blocks_count))
        };
        assert_eq!(after_alloc, 97, "Should have 97 free blocks after allocating 3");

        // Now simulate truncate_file: free all 3 blocks
        // This is what truncate_file MUST do - if it only sets i_blocks=0,
        // the blocks remain allocated in the bitmap and this count won't change
        free_block(&device, block1, &superblock, &mut block_groups)
            .expect("Free block 1 should succeed");
        free_block(&device, block2, &superblock, &mut block_groups)
            .expect("Free block 2 should succeed");
        free_block(&device, block3, &superblock, &mut block_groups)
            .expect("Free block 3 should succeed");

        // After freeing 3 blocks, free count should be back to 100
        let after_truncate = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(block_groups[0].bg_free_blocks_count))
        };
        assert_eq!(
            after_truncate, 100,
            "After truncate frees blocks, bg_free_blocks_count should return to 100. \
             If this fails, truncate_file is not calling free_block() for allocated blocks."
        );

        // Also verify the bitmap bits are actually cleared (blocks are reusable)
        // Allocating again should return the same blocks
        let realloc1 = allocate_block(&device, &superblock, &mut block_groups)
            .expect("Re-allocate should succeed");
        let realloc2 = allocate_block(&device, &superblock, &mut block_groups)
            .expect("Re-allocate should succeed");
        let realloc3 = allocate_block(&device, &superblock, &mut block_groups)
            .expect("Re-allocate should succeed");

        // The freed blocks should be reused (order may vary based on bitmap scan)
        let realloc_set: [u32; 3] = [realloc1, realloc2, realloc3];
        assert!(
            realloc_set.contains(&block1) && realloc_set.contains(&block2) && realloc_set.contains(&block3),
            "Freed blocks should be reallocated. Got {:?}, expected {:?}",
            realloc_set, [block1, block2, block3]
        );
    }
}
