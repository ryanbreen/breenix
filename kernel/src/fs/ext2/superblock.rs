//! ext2 superblock structures and parsing
//!
//! The superblock contains critical filesystem metadata and is always located
//! at byte offset 1024 from the start of the device.

use crate::block::{BlockDevice, BlockError};
use core::mem;

/// ext2 magic number - identifies an ext2 filesystem
const EXT2_SUPER_MAGIC: u16 = 0xEF53;

/// Superblock offset from start of device (always 1024 bytes)
const SUPERBLOCK_OFFSET: usize = 1024;

/// ext2 superblock - always at byte offset 1024 from start of device
///
/// The superblock contains all the information about the filesystem layout,
/// including block size, number of inodes, block groups, and feature flags.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext2Superblock {
    pub s_inodes_count: u32,         // Total inodes
    pub s_blocks_count: u32,         // Total blocks
    pub s_r_blocks_count: u32,       // Reserved blocks
    pub s_free_blocks_count: u32,    // Free blocks
    pub s_free_inodes_count: u32,    // Free inodes
    pub s_first_data_block: u32,     // First data block (0 for 1024+ block sizes, 1 for 1024)
    pub s_log_block_size: u32,       // Block size = 1024 << s_log_block_size
    pub s_log_frag_size: u32,        // Fragment size
    pub s_blocks_per_group: u32,     // Blocks per group
    pub s_frags_per_group: u32,      // Fragments per group
    pub s_inodes_per_group: u32,     // Inodes per group
    pub s_mtime: u32,                // Mount time
    pub s_wtime: u32,                // Write time
    pub s_mnt_count: u16,            // Mount count
    pub s_max_mnt_count: u16,        // Max mount count
    pub s_magic: u16,                // Magic number (0xEF53)
    pub s_state: u16,                // State (clean/errors)
    pub s_errors: u16,               // Error handling
    pub s_minor_rev_level: u16,      // Minor revision
    pub s_lastcheck: u32,            // Last check time
    pub s_checkinterval: u32,        // Check interval
    pub s_creator_os: u32,           // Creator OS
    pub s_rev_level: u32,            // Revision level
    pub s_def_resuid: u16,           // Default UID for reserved blocks
    pub s_def_resgid: u16,           // Default GID for reserved blocks
    // Extended superblock fields (rev 1+)
    pub s_first_ino: u32,            // First non-reserved inode
    pub s_inode_size: u16,           // Inode size
    pub s_block_group_nr: u16,       // Block group number of this superblock
    pub s_feature_compat: u32,       // Compatible features
    pub s_feature_incompat: u32,     // Incompatible features
    pub s_feature_ro_compat: u32,    // Read-only compatible features
    pub s_uuid: [u8; 16],            // UUID
    pub s_volume_name: [u8; 16],     // Volume name
    // Padding to 1024 bytes total
    _reserved: [u8; 788],
}

impl Ext2Superblock {
    /// Read superblock from block device (always at offset 1024)
    ///
    /// # Arguments
    /// * `device` - The block device to read from
    ///
    /// # Returns
    /// * `Ok(Ext2Superblock)` - Successfully read and parsed superblock
    /// * `Err(BlockError)` - I/O error or invalid superblock
    pub fn read_from<B: BlockDevice>(device: &B) -> Result<Self, BlockError> {
        // The superblock is always at byte offset 1024, which may span multiple
        // device blocks depending on the device's native block size
        let device_block_size = device.block_size();
        
        // Calculate which device blocks we need to read
        let start_block = SUPERBLOCK_OFFSET / device_block_size;
        let offset_in_block = SUPERBLOCK_OFFSET % device_block_size;
        
        // We need to read enough blocks to get 1024 bytes of superblock data
        let superblock_size = mem::size_of::<Ext2Superblock>();
        let bytes_needed = offset_in_block + superblock_size;
        let blocks_needed = (bytes_needed + device_block_size - 1) / device_block_size;
        
        // Read the necessary blocks into a buffer
        let mut buffer = [0u8; 4096]; // Large enough for typical cases
        for i in 0..blocks_needed {
            device.read_block(
                (start_block + i) as u64,
                &mut buffer[i * device_block_size..(i + 1) * device_block_size],
            )?;
        }
        
        // Extract superblock from the buffer
        let sb_bytes = &buffer[offset_in_block..offset_in_block + superblock_size];
        
        // SAFETY: We're reading from a properly aligned buffer into a packed struct
        // The buffer is guaranteed to be large enough by the calculation above
        let superblock: Ext2Superblock = unsafe {
            core::ptr::read_unaligned(sb_bytes.as_ptr() as *const Ext2Superblock)
        };
        
        // Validate the magic number
        if !superblock.is_valid() {
            return Err(BlockError::IoError);
        }
        
        Ok(superblock)
    }
    
    /// Validate magic number
    ///
    /// # Returns
    /// * `true` - Valid ext2 filesystem (magic = 0xEF53)
    /// * `false` - Invalid or corrupted filesystem
    pub fn is_valid(&self) -> bool {
        self.s_magic == EXT2_SUPER_MAGIC
    }
    
    /// Calculate block size in bytes
    ///
    /// ext2 block size is computed as: 1024 << s_log_block_size
    /// Valid values: 1024, 2048, 4096 (s_log_block_size = 0, 1, 2)
    ///
    /// # Returns
    /// Block size in bytes
    pub fn block_size(&self) -> usize {
        1024 << self.s_log_block_size
    }
    
    /// Calculate number of block groups
    ///
    /// The number of block groups is determined by dividing the total number
    /// of blocks by the blocks per group (rounding up).
    ///
    /// # Returns
    /// Number of block groups in the filesystem
    pub fn block_group_count(&self) -> u32 {
        (self.s_blocks_count + self.s_blocks_per_group - 1) / self.s_blocks_per_group
    }
    
    /// Calculate inode size (128 for rev 0, s_inode_size for rev 1+)
    ///
    /// # Returns
    /// Inode size in bytes
    pub fn inode_size(&self) -> usize {
        if self.s_rev_level == 0 {
            128 // Default for original ext2
        } else {
            self.s_inode_size as usize
        }
    }

    /// Create superblock from a byte slice
    ///
    /// This method is useful for testing and for parsing superblock data
    /// that has already been read into memory.
    ///
    /// # Arguments
    /// * `bytes` - A slice containing at least 1024 bytes of superblock data
    ///
    /// # Returns
    /// * `Some(Ext2Superblock)` - Successfully parsed superblock
    /// * `None` - Slice too small
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let superblock_size = mem::size_of::<Ext2Superblock>();
        if bytes.len() < superblock_size {
            return None;
        }

        // SAFETY: We've verified the slice is large enough.
        // The struct is packed so alignment is not a concern.
        let superblock: Ext2Superblock = unsafe {
            core::ptr::read_unaligned(bytes.as_ptr() as *const Ext2Superblock)
        };

        Some(superblock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a mock ext2 superblock with typical values
    fn create_mock_superblock_bytes() -> [u8; 1024] {
        let mut bytes = [0u8; 1024];

        // s_inodes_count (offset 0): 1024 inodes
        bytes[0..4].copy_from_slice(&1024u32.to_le_bytes());
        // s_blocks_count (offset 4): 8192 blocks
        bytes[4..8].copy_from_slice(&8192u32.to_le_bytes());
        // s_r_blocks_count (offset 8): 409 reserved blocks (~5%)
        bytes[8..12].copy_from_slice(&409u32.to_le_bytes());
        // s_free_blocks_count (offset 12): 7000 free blocks
        bytes[12..16].copy_from_slice(&7000u32.to_le_bytes());
        // s_free_inodes_count (offset 16): 1000 free inodes
        bytes[16..20].copy_from_slice(&1000u32.to_le_bytes());
        // s_first_data_block (offset 20): 0 for 4KB blocks
        bytes[20..24].copy_from_slice(&0u32.to_le_bytes());
        // s_log_block_size (offset 24): 2 means 4096 byte blocks (1024 << 2)
        bytes[24..28].copy_from_slice(&2u32.to_le_bytes());
        // s_log_frag_size (offset 28): 2
        bytes[28..32].copy_from_slice(&2u32.to_le_bytes());
        // s_blocks_per_group (offset 32): 8192 blocks per group
        bytes[32..36].copy_from_slice(&8192u32.to_le_bytes());
        // s_frags_per_group (offset 36): 8192
        bytes[36..40].copy_from_slice(&8192u32.to_le_bytes());
        // s_inodes_per_group (offset 40): 1024
        bytes[40..44].copy_from_slice(&1024u32.to_le_bytes());
        // s_mtime (offset 44): mount time
        bytes[44..48].copy_from_slice(&0u32.to_le_bytes());
        // s_wtime (offset 48): write time
        bytes[48..52].copy_from_slice(&0u32.to_le_bytes());
        // s_mnt_count (offset 52): mount count
        bytes[52..54].copy_from_slice(&1u16.to_le_bytes());
        // s_max_mnt_count (offset 54): max mount count
        bytes[54..56].copy_from_slice(&20u16.to_le_bytes());
        // s_magic (offset 56): ext2 magic number 0xEF53
        bytes[56..58].copy_from_slice(&EXT2_SUPER_MAGIC.to_le_bytes());
        // s_state (offset 58): clean state
        bytes[58..60].copy_from_slice(&1u16.to_le_bytes());
        // s_errors (offset 60): continue on errors
        bytes[60..62].copy_from_slice(&1u16.to_le_bytes());
        // s_minor_rev_level (offset 62): 0
        bytes[62..64].copy_from_slice(&0u16.to_le_bytes());
        // s_lastcheck (offset 64): last check time
        bytes[64..68].copy_from_slice(&0u32.to_le_bytes());
        // s_checkinterval (offset 68): check interval
        bytes[68..72].copy_from_slice(&0u32.to_le_bytes());
        // s_creator_os (offset 72): Linux
        bytes[72..76].copy_from_slice(&0u32.to_le_bytes());
        // s_rev_level (offset 76): revision 1
        bytes[76..80].copy_from_slice(&1u32.to_le_bytes());
        // s_def_resuid (offset 80): default reserved UID
        bytes[80..82].copy_from_slice(&0u16.to_le_bytes());
        // s_def_resgid (offset 82): default reserved GID
        bytes[82..84].copy_from_slice(&0u16.to_le_bytes());
        // s_first_ino (offset 84): first non-reserved inode (11 for ext2)
        bytes[84..88].copy_from_slice(&11u32.to_le_bytes());
        // s_inode_size (offset 88): 256 bytes per inode
        bytes[88..90].copy_from_slice(&256u16.to_le_bytes());

        bytes
    }

    #[test]
    fn test_superblock_magic_validation() {
        let bytes = create_mock_superblock_bytes();
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");

        assert!(sb.is_valid(), "Superblock should be valid with correct magic 0xEF53");
        assert_eq!(sb.s_magic, 0xEF53, "Magic number should be 0xEF53");
    }

    #[test]
    fn test_superblock_block_size() {
        let mut bytes = create_mock_superblock_bytes();

        // Test with s_log_block_size = 2 (4096 bytes)
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.block_size(), 4096, "Block size should be 4096 when log=2");

        // Test with s_log_block_size = 0 (1024 bytes)
        bytes[24..28].copy_from_slice(&0u32.to_le_bytes());
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.block_size(), 1024, "Block size should be 1024 when log=0");

        // Test with s_log_block_size = 1 (2048 bytes)
        bytes[24..28].copy_from_slice(&1u32.to_le_bytes());
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.block_size(), 2048, "Block size should be 2048 when log=1");
    }

    #[test]
    fn test_superblock_block_group_count() {
        let mut bytes = create_mock_superblock_bytes();

        // With 8192 blocks and 8192 blocks_per_group, should be 1 group
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.block_group_count(), 1, "Should have 1 block group");

        // With 16384 blocks and 8192 blocks_per_group, should be 2 groups
        bytes[4..8].copy_from_slice(&16384u32.to_le_bytes());
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.block_group_count(), 2, "Should have 2 block groups");

        // With 10000 blocks and 8192 blocks_per_group, should be 2 groups (rounds up)
        bytes[4..8].copy_from_slice(&10000u32.to_le_bytes());
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.block_group_count(), 2, "Should have 2 block groups (10000/8192 rounds up)");

        // With 1 block and 8192 blocks_per_group, should be 1 group
        bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.block_group_count(), 1, "Should have 1 block group for minimal filesystem");
    }

    #[test]
    fn test_invalid_magic() {
        let mut bytes = create_mock_superblock_bytes();

        // Set wrong magic number
        bytes[56..58].copy_from_slice(&0x0000u16.to_le_bytes());
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert!(!sb.is_valid(), "Superblock should be invalid with wrong magic 0x0000");

        // Another wrong magic
        bytes[56..58].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert!(!sb.is_valid(), "Superblock should be invalid with wrong magic 0xFFFF");

        // Almost correct magic (swapped bytes)
        bytes[56..58].copy_from_slice(&0x53EFu16.to_le_bytes());
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert!(!sb.is_valid(), "Superblock should be invalid with swapped magic bytes");
    }

    #[test]
    fn test_from_bytes_too_small() {
        let small_buffer = [0u8; 100];
        assert!(
            Ext2Superblock::from_bytes(&small_buffer).is_none(),
            "from_bytes should return None for buffer smaller than superblock size"
        );
    }

    #[test]
    fn test_inode_size() {
        let mut bytes = create_mock_superblock_bytes();

        // Rev 1 with s_inode_size = 256
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.inode_size(), 256, "Inode size should be 256 for rev 1");

        // Rev 0 should always return 128 regardless of s_inode_size field
        bytes[76..80].copy_from_slice(&0u32.to_le_bytes()); // s_rev_level = 0
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");
        assert_eq!(sb.inode_size(), 128, "Inode size should be 128 for rev 0");
    }

    #[test]
    fn test_superblock_field_values() {
        let bytes = create_mock_superblock_bytes();
        let sb = Ext2Superblock::from_bytes(&bytes).expect("Failed to parse superblock");

        assert_eq!(sb.s_inodes_count, 1024, "Inodes count should be 1024");
        assert_eq!(sb.s_blocks_count, 8192, "Blocks count should be 8192");
        assert_eq!(sb.s_blocks_per_group, 8192, "Blocks per group should be 8192");
        assert_eq!(sb.s_inodes_per_group, 1024, "Inodes per group should be 1024");
        assert_eq!(sb.s_first_ino, 11, "First non-reserved inode should be 11");
    }
}
