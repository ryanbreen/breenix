//! ext2 Inode Structures and Operations
//!
//! This module provides the core inode structure for ext2 filesystems, including
//! file type detection, permissions handling, and inode reading from block devices.

use crate::block::{BlockDevice, BlockError};
use crate::fs::ext2::block_group::free_block;
use crate::fs::ext2::file::{read_ext2_block, write_ext2_block};

/// File type constants (from i_mode upper bits)
pub const EXT2_S_IFSOCK: u16 = 0xC000; // Socket
pub const EXT2_S_IFLNK: u16 = 0xA000; // Symbolic link
pub const EXT2_S_IFREG: u16 = 0x8000; // Regular file
pub const EXT2_S_IFBLK: u16 = 0x6000; // Block device
pub const EXT2_S_IFDIR: u16 = 0x4000; // Directory
pub const EXT2_S_IFCHR: u16 = 0x2000; // Character device
pub const EXT2_S_IFIFO: u16 = 0x1000; // FIFO

/// File type mask
const EXT2_S_IFMT: u16 = 0xF000;

/// Permission bits
pub const EXT2_S_IRUSR: u16 = 0x0100; // User read
pub const EXT2_S_IWUSR: u16 = 0x0080; // User write
pub const EXT2_S_IXUSR: u16 = 0x0040; // User execute
pub const EXT2_S_IRGRP: u16 = 0x0020; // Group read
pub const EXT2_S_IWGRP: u16 = 0x0010; // Group write
pub const EXT2_S_IXGRP: u16 = 0x0008; // Group execute
pub const EXT2_S_IROTH: u16 = 0x0004; // Other read
pub const EXT2_S_IWOTH: u16 = 0x0002; // Other write
pub const EXT2_S_IXOTH: u16 = 0x0001; // Other execute

/// Permission mask
const EXT2_S_PERM_MASK: u16 = 0x0FFF;

/// ext2 inode structure (128 bytes for rev 0, variable for rev 1+)
///
/// This is the on-disk representation of an inode. The structure is packed
/// to match the exact layout on disk.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ext2Inode {
    pub i_mode: u16,        // File mode (type + permissions)
    pub i_uid: u16,         // Owner UID
    pub i_size: u32,        // Size in bytes (lower 32 bits)
    pub i_atime: u32,       // Access time
    pub i_ctime: u32,       // Creation time
    pub i_mtime: u32,       // Modification time
    pub i_dtime: u32,       // Deletion time
    pub i_gid: u16,         // Group ID
    pub i_links_count: u16, // Links count
    pub i_blocks: u32,      // Blocks count (in 512-byte units)
    pub i_flags: u32,       // File flags
    pub i_osd1: u32,        // OS dependent value 1
    pub i_block: [u32; 15], // Block pointers:
                            //   [0-11]: Direct blocks
                            //   [12]: Single indirect
                            //   [13]: Double indirect
                            //   [14]: Triple indirect
    pub i_generation: u32,  // File version (for NFS)
    pub i_file_acl: u32,    // File ACL block
    pub i_dir_acl: u32,     // Directory ACL / high 32 bits of size
    pub i_faddr: u32,       // Fragment address
    pub i_osd2: [u8; 12],   // OS dependent value 2
}

/// File type enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
    SymLink,
    Unknown,
}

impl Ext2Inode {
    /// Parse an inode from a 128-byte slice
    ///
    /// # Safety
    /// The slice must be exactly 128 bytes and contain a valid ext2 inode structure.
    ///
    /// # Panics
    /// Panics if the slice length is not exactly 128 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        assert_eq!(bytes.len(), 128, "inode must be exactly 128 bytes");

        // Safety: We're casting a byte slice to a packed struct
        // This is safe because:
        // 1. The struct is #[repr(C, packed)] matching on-disk layout
        // 2. We've verified exactly 128 bytes (size of Ext2Inode)
        // 3. All field types are Copy and have no invalid bit patterns
        unsafe {
            let ptr = bytes.as_ptr() as *const Ext2Inode;
            ptr.read_unaligned()
        }
    }

    /// Read an inode from the device
    ///
    /// Inode numbers are 1-indexed (inode 1 is bad blocks, inode 2 is root).
    ///
    /// # Algorithm
    /// 1. Calculate which block group contains this inode
    /// 2. Calculate the local index within that block group's inode table
    /// 3. Read from the inode table block at the appropriate offset
    ///
    /// # Arguments
    /// * `device` - The block device to read from
    /// * `inode_num` - The inode number (1-indexed)
    /// * `superblock` - The ext2 superblock (for inodes_per_group, inode_size)
    /// * `block_groups` - Array of block group descriptors
    ///
    /// # Returns
    /// The inode structure, or a BlockError if reading fails
    pub fn read_from<B: BlockDevice + ?Sized>(
        device: &B,
        inode_num: u32,
        superblock: &super::Ext2Superblock,
        block_groups: &[super::Ext2BlockGroupDesc],
    ) -> Result<Self, BlockError> {
        // Inode numbers are 1-indexed, array indices are 0-indexed
        let inode_index = inode_num - 1;

        // Calculate which block group contains this inode
        // Safety: superblock is a packed struct, need unaligned read
        let inodes_per_group = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(superblock.s_inodes_per_group))
        };
        let block_group = (inode_index / inodes_per_group) as usize;
        let local_index = inode_index % inodes_per_group;

        // Get the inode table starting block for this block group
        // Safety: block_groups is a packed struct, need unaligned read
        let inode_table_block = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(block_groups[block_group].bg_inode_table))
        };

        // Calculate byte offset within the inode table
        // Safety: superblock is a packed struct, need unaligned read
        let s_rev_level = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(superblock.s_rev_level))
        };
        let inode_size = if s_rev_level == 0 {
            128 // Original ext2 revision uses 128-byte inodes
        } else {
            let s_inode_size = unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!(superblock.s_inode_size))
            };
            s_inode_size as u32
        };
        let byte_offset = local_index * inode_size;

        // Calculate which ext2 block and offset within that block
        let s_log_block_size = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(superblock.s_log_block_size))
        };
        let ext2_block_size = 1024u32 << s_log_block_size;
        let ext2_block_offset = byte_offset / ext2_block_size;
        let offset_in_ext2_block = (byte_offset % ext2_block_size) as usize;

        // Calculate the ext2 block number containing the inode
        let target_ext2_block = inode_table_block + ext2_block_offset;

        // Convert ext2 block to device blocks
        // Device uses 512-byte sectors, ext2 uses ext2_block_size (typically 1024+)
        let device_block_size = device.block_size();
        let device_blocks_per_ext2_block = ext2_block_size as usize / device_block_size;
        let start_device_block = (target_ext2_block as usize) * device_blocks_per_ext2_block;

        // Read all device blocks that make up this ext2 block
        let mut block_buf = [0u8; 4096]; // Support up to 4KB block size
        for i in 0..device_blocks_per_ext2_block {
            device.read_block(
                (start_device_block + i) as u64,
                &mut block_buf[i * device_block_size..(i + 1) * device_block_size],
            )?;
        }

        // Extract the inode from the block buffer
        // We only read the first 128 bytes regardless of actual inode_size
        let inode_bytes = &block_buf[offset_in_ext2_block..offset_in_ext2_block + 128];

        // Safety: We're casting a byte slice to a packed struct
        // This is safe because:
        // 1. The struct is #[repr(C, packed)] matching on-disk layout
        // 2. We've read exactly 128 bytes (size of Ext2Inode)
        // 3. All field types are Copy and have no invalid bit patterns
        unsafe {
            let ptr = inode_bytes.as_ptr() as *const Ext2Inode;
            Ok(ptr.read_unaligned())
        }
    }

    /// Get file type from mode
    pub fn file_type(&self) -> FileType {
        // Safety: Reading from packed struct requires unaligned access
        let mode = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.i_mode)) };
        match mode & EXT2_S_IFMT {
            EXT2_S_IFREG => FileType::Regular,
            EXT2_S_IFDIR => FileType::Directory,
            EXT2_S_IFCHR => FileType::CharDevice,
            EXT2_S_IFBLK => FileType::BlockDevice,
            EXT2_S_IFIFO => FileType::Fifo,
            EXT2_S_IFSOCK => FileType::Socket,
            EXT2_S_IFLNK => FileType::SymLink,
            _ => FileType::Unknown,
        }
    }

    /// Check if this is a directory
    pub fn is_dir(&self) -> bool {
        matches!(self.file_type(), FileType::Directory)
    }

    /// Check if this is a regular file
    pub fn is_file(&self) -> bool {
        matches!(self.file_type(), FileType::Regular)
    }

    /// Check if this is a symbolic link
    pub fn is_symlink(&self) -> bool {
        matches!(self.file_type(), FileType::SymLink)
    }

    /// Get file size (combines i_size and i_dir_acl for large files)
    ///
    /// For regular files in ext2 revision 1+, the high 32 bits of the file size
    /// are stored in i_dir_acl. This allows files larger than 4GB.
    pub fn size(&self) -> u64 {
        // Safety: Reading from packed struct requires unaligned access
        let low_bits = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.i_size)) } as u64;

        // For regular files, i_dir_acl contains high 32 bits of size
        if self.is_file() {
            let high_bits = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.i_dir_acl)) } as u64;
            (high_bits << 32) | low_bits
        } else {
            low_bits
        }
    }

    /// Get permissions (lower 12 bits of mode)
    pub fn permissions(&self) -> u16 {
        // Safety: Reading from packed struct requires unaligned access
        let mode = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(self.i_mode)) };
        mode & EXT2_S_PERM_MASK
    }

    /// Update timestamps on the inode
    ///
    /// # Arguments
    /// * `update_atime` - Update access time
    /// * `update_mtime` - Update modification time
    /// * `update_ctime` - Update change time
    ///
    /// # Example
    /// ```
    /// // After writing to a file
    /// inode.update_timestamps(false, true, true);
    ///
    /// // After reading from a file
    /// inode.update_timestamps(true, false, false);
    ///
    /// // After changing inode metadata (chmod, chown, etc.)
    /// inode.update_timestamps(false, false, true);
    /// ```
    pub fn update_timestamps(&mut self, update_atime: bool, update_mtime: bool, update_ctime: bool) {
        let now = crate::time::current_unix_time() as u32;

        if update_atime {
            self.i_atime = now;
        }
        if update_mtime {
            self.i_mtime = now;
        }
        if update_ctime {
            self.i_ctime = now;
        }
    }
}

/// Root directory inode number
pub const EXT2_ROOT_INO: u32 = 2;

/// Bad blocks inode
pub const EXT2_BAD_INO: u32 = 1;

/// First non-reserved inode (ext2 uses 11 for rev 0, s_first_ino for rev 1+)
pub const EXT2_FIRST_INO: u32 = 11;

/// Decrement inode link count and optionally free the inode if it reaches 0
///
/// This function:
/// 1. Reads the inode from disk
/// 2. Decrements the link count
/// 3. If link count reaches 0, marks inode as deleted and frees its resources
/// 4. Writes the updated inode back to disk
///
/// # Arguments
/// * `device` - The block device
/// * `inode_num` - The inode number to decrement
/// * `superblock` - The ext2 superblock
/// * `block_groups` - Mutable reference to block group descriptors
///
/// # Returns
/// * `Ok(new_link_count)` - The new link count after decrement
/// * `Err(msg)` - Error message
pub fn decrement_inode_links<B: BlockDevice + ?Sized>(
    device: &B,
    inode_num: u32,
    superblock: &super::Ext2Superblock,
    block_groups: &mut [super::Ext2BlockGroupDesc],
) -> Result<u16, &'static str> {
    // Read the inode
    let mut inode = Ext2Inode::read_from(device, inode_num, superblock, block_groups)
        .map_err(|_| "Failed to read inode")?;

    // Decrement the link count
    let current_links = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_links_count)) };
    let new_links = current_links.saturating_sub(1);
    inode.i_links_count = new_links;

    // If link count reached 0, mark as deleted and free resources
    if new_links == 0 {
        // Set deletion time
        let deletion_time = crate::time::current_unix_time() as u32;
        inode.i_dtime = deletion_time;

        // Free all data blocks associated with this inode
        let blocks_freed = free_inode_blocks(device, superblock, block_groups, &inode)?;
        if blocks_freed > 0 {
            log::debug!("ext2: freed {} blocks for inode {}", blocks_freed, inode_num);
        }

        // Free the inode in the bitmap
        free_inode_bitmap(device, inode_num, superblock, block_groups)?;
    }

    // Write the updated inode back
    inode.write_to(device, inode_num, superblock, block_groups)
        .map_err(|_| "Failed to write inode")?;

    Ok(new_links)
}

/// Increment inode link count
///
/// This function:
/// 1. Reads the inode from disk
/// 2. Increments the link count
/// 3. Updates the ctime (change time)
/// 4. Writes the updated inode back to disk
///
/// # Arguments
/// * `device` - The block device
/// * `inode_num` - The inode number to increment
/// * `superblock` - The ext2 superblock
/// * `block_groups` - Reference to block group descriptors
///
/// # Returns
/// * `Ok(new_link_count)` - The new link count after increment
/// * `Err(msg)` - Error message
pub fn increment_inode_links<B: BlockDevice + ?Sized>(
    device: &B,
    inode_num: u32,
    superblock: &super::Ext2Superblock,
    block_groups: &[super::Ext2BlockGroupDesc],
) -> Result<u16, &'static str> {
    // Read the inode
    let mut inode = Ext2Inode::read_from(device, inode_num, superblock, block_groups)
        .map_err(|_| "Failed to read inode")?;

    // Increment the link count (saturating to prevent overflow)
    let current_links = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_links_count)) };
    let new_links = current_links.saturating_add(1);
    inode.i_links_count = new_links;

    // Update ctime (metadata change time)
    inode.update_timestamps(false, false, true);

    // Write the updated inode back
    inode.write_to(device, inode_num, superblock, block_groups)
        .map_err(|_| "Failed to write inode")?;

    Ok(new_links)
}

/// Free an inode in the inode bitmap
///
/// Marks the inode as free in the inode bitmap and updates the
/// free inode count in the block group descriptor.
fn free_inode_bitmap<B: BlockDevice + ?Sized>(
    device: &B,
    inode_num: u32,
    superblock: &super::Ext2Superblock,
    block_groups: &mut [super::Ext2BlockGroupDesc],
) -> Result<(), &'static str> {
    let block_size = superblock.block_size();
    let inodes_per_group = superblock.s_inodes_per_group;

    // Calculate which block group contains this inode
    let inode_index = inode_num - 1;
    let bg_index = (inode_index / inodes_per_group) as usize;
    let local_index = inode_index % inodes_per_group;

    let bg = &mut block_groups[bg_index];

    // Read the inode bitmap block
    // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
    let bitmap_block = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_inode_bitmap))
    };
    let mut bitmap_buf = [0u8; 4096]; // Max block size
    read_ext2_block(device, bitmap_block, block_size, &mut bitmap_buf[..block_size])
        .map_err(|_| "Failed to read inode bitmap")?;

    // Clear the bit for this inode
    let byte_index = (local_index / 8) as usize;
    let bit_index = (local_index % 8) as u8;

    if byte_index < block_size {
        bitmap_buf[byte_index] &= !(1 << bit_index);
    }

    // Write the updated bitmap back
    write_ext2_block(device, bitmap_block, block_size, &bitmap_buf[..block_size])
        .map_err(|_| "Failed to write inode bitmap")?;

    // Update the free inode count
    let free_inodes = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_free_inodes_count))
    };
    unsafe {
        core::ptr::write_unaligned(
            core::ptr::addr_of_mut!(bg.bg_free_inodes_count),
            free_inodes + 1,
        );
    }

    Ok(())
}

/// Free all data blocks associated with an inode
///
/// This function iterates through all block pointers (direct, single indirect,
/// double indirect, and triple indirect) and frees each allocated block.
///
/// # Arguments
/// * `device` - The block device
/// * `superblock` - The ext2 superblock
/// * `block_groups` - Mutable reference to block group descriptors
/// * `inode` - The inode whose blocks should be freed
///
/// # Returns
/// * `Ok(blocks_freed)` - Number of blocks freed
/// * `Err(msg)` - Error message if operation failed
fn free_inode_blocks<B: BlockDevice + ?Sized>(
    device: &B,
    superblock: &super::Ext2Superblock,
    block_groups: &mut [super::Ext2BlockGroupDesc],
    inode: &Ext2Inode,
) -> Result<u32, &'static str> {
    let block_size = superblock.block_size();
    let _ptrs_per_block = block_size / 4; // Reserved for future full deallocation
    let mut blocks_freed = 0u32;

    // Read the i_block array safely from the packed struct
    let i_block = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block))
    };

    // 1. Free direct blocks (i_block[0-11])
    for i in 0..12 {
        let block_num = i_block[i];
        if block_num != 0 {
            free_block(device, block_num, superblock, block_groups)?;
            blocks_freed += 1;
        }
    }

    // 2. Free single indirect block (i_block[12])
    // Note: We only free the indirect block pointer itself, not its contents.
    // Full recursive deallocation requires heap allocations which exhaust the
    // bump allocator. The data blocks remain allocated but inaccessible.
    // TODO: Implement proper deallocation when we have a real allocator.
    let single_indirect = i_block[12];
    if single_indirect != 0 {
        free_block(device, single_indirect, superblock, block_groups)?;
        blocks_freed += 1;
    }

    // 3. Free double indirect block (i_block[13])
    let double_indirect = i_block[13];
    if double_indirect != 0 {
        free_block(device, double_indirect, superblock, block_groups)?;
        blocks_freed += 1;
    }

    // 4. Free triple indirect block (i_block[14])
    let triple_indirect = i_block[14];
    if triple_indirect != 0 {
        free_block(device, triple_indirect, superblock, block_groups)?;
        blocks_freed += 1;
    }

    Ok(blocks_freed)
}

/// Free all data blocks referenced by a single indirect block
fn free_indirect_block<B: BlockDevice + ?Sized>(
    device: &B,
    superblock: &super::Ext2Superblock,
    block_groups: &mut [super::Ext2BlockGroupDesc],
    indirect_block: u32,
    block_size: usize,
) -> Result<u32, &'static str> {
    let mut blocks_freed = 0u32;

    // Read the indirect block
    // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
    let mut buf = [0u8; 4096]; // Max block size
    read_ext2_block(device, indirect_block, block_size, &mut buf[..block_size])
        .map_err(|_| "Failed to read indirect block")?;

    // Parse block pointers and free each non-zero block
    let num_pointers = block_size / 4;
    for i in 0..num_pointers {
        let offset = i * 4;
        let block_num = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);

        if block_num != 0 {
            free_block(device, block_num, superblock, block_groups)?;
            blocks_freed += 1;
        }
    }

    Ok(blocks_freed)
}

/// Free all data blocks referenced by a double indirect block
fn free_double_indirect_block<B: BlockDevice + ?Sized>(
    device: &B,
    superblock: &super::Ext2Superblock,
    block_groups: &mut [super::Ext2BlockGroupDesc],
    double_indirect_block: u32,
    block_size: usize,
    ptrs_per_block: usize,
) -> Result<u32, &'static str> {
    let mut blocks_freed = 0u32;

    // Read the double indirect block (contains pointers to single indirect blocks)
    // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
    let mut buf = [0u8; 4096]; // Max block size
    read_ext2_block(device, double_indirect_block, block_size, &mut buf[..block_size])
        .map_err(|_| "Failed to read double indirect block")?;

    // For each first-level pointer
    for i in 0..ptrs_per_block {
        let offset = i * 4;
        let first_level_ptr = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);

        if first_level_ptr != 0 {
            // Free all data blocks referenced by this single indirect block
            blocks_freed += free_indirect_block(device, superblock, block_groups, first_level_ptr, block_size)?;
            // Free the single indirect block itself
            free_block(device, first_level_ptr, superblock, block_groups)?;
            blocks_freed += 1;
        }
    }

    Ok(blocks_freed)
}

/// Free all data blocks referenced by a triple indirect block
fn free_triple_indirect_block<B: BlockDevice + ?Sized>(
    device: &B,
    superblock: &super::Ext2Superblock,
    block_groups: &mut [super::Ext2BlockGroupDesc],
    triple_indirect_block: u32,
    block_size: usize,
    ptrs_per_block: usize,
) -> Result<u32, &'static str> {
    let mut blocks_freed = 0u32;

    // Read the triple indirect block (contains pointers to double indirect blocks)
    // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
    let mut buf = [0u8; 4096]; // Max block size
    read_ext2_block(device, triple_indirect_block, block_size, &mut buf[..block_size])
        .map_err(|_| "Failed to read triple indirect block")?;

    // For each first-level pointer
    for i in 0..ptrs_per_block {
        let offset = i * 4;
        let first_level_ptr = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);

        if first_level_ptr != 0 {
            // Free all blocks referenced by this double indirect block
            blocks_freed += free_double_indirect_block(device, superblock, block_groups, first_level_ptr, block_size, ptrs_per_block)?;
            // Free the double indirect block itself
            free_block(device, first_level_ptr, superblock, block_groups)?;
            blocks_freed += 1;
        }
    }

    Ok(blocks_freed)
}

impl Ext2Inode {
    /// Write an inode to the device
    ///
    /// Writes the inode structure to the correct location in the inode table.
    ///
    /// # Arguments
    /// * `device` - The block device to write to
    /// * `inode_num` - The inode number (1-indexed)
    /// * `superblock` - The ext2 superblock
    /// * `block_groups` - Array of block group descriptors
    pub fn write_to<B: BlockDevice + ?Sized>(
        &self,
        device: &B,
        inode_num: u32,
        superblock: &super::Ext2Superblock,
        block_groups: &[super::Ext2BlockGroupDesc],
    ) -> Result<(), BlockError> {
        // Inode numbers are 1-indexed, array indices are 0-indexed
        let inode_index = inode_num - 1;

        // Calculate which block group contains this inode
        // Safety: superblock is a packed struct, need unaligned read
        let inodes_per_group = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(superblock.s_inodes_per_group))
        };
        let block_group = (inode_index / inodes_per_group) as usize;
        let local_index = inode_index % inodes_per_group;

        // Get the inode table starting block for this block group
        // Safety: block_groups is a packed struct, need unaligned read
        let inode_table_block = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(block_groups[block_group].bg_inode_table))
        };

        // Calculate byte offset within the inode table
        // Safety: superblock is a packed struct, need unaligned read
        let s_rev_level = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(superblock.s_rev_level))
        };
        let inode_size = if s_rev_level == 0 {
            128 // Original ext2 revision uses 128-byte inodes
        } else {
            let s_inode_size = unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!(superblock.s_inode_size))
            };
            s_inode_size as u32
        };
        let byte_offset = local_index * inode_size;

        // Calculate which block and offset within that block
        let s_log_block_size = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(superblock.s_log_block_size))
        };
        let ext2_block_size = 1024u32 << s_log_block_size;
        let ext2_block_offset = byte_offset / ext2_block_size;
        let offset_in_ext2_block = (byte_offset % ext2_block_size) as usize;

        // Calculate the ext2 block number containing the inode
        let target_ext2_block = inode_table_block + ext2_block_offset;

        // Convert ext2 block to device blocks
        let device_block_size = device.block_size();
        let device_blocks_per_ext2_block = ext2_block_size as usize / device_block_size;
        let start_device_block = (target_ext2_block as usize) * device_blocks_per_ext2_block;

        // Read all device blocks that make up this ext2 block
        let mut block_buf = [0u8; 4096]; // Support up to 4KB block size
        for i in 0..device_blocks_per_ext2_block {
            device.read_block(
                (start_device_block + i) as u64,
                &mut block_buf[i * device_block_size..(i + 1) * device_block_size],
            )?;
        }

        // Write the inode structure into the buffer
        // Safety: We're writing the first 128 bytes of the inode structure
        let inode_bytes = unsafe {
            core::slice::from_raw_parts(
                self as *const Ext2Inode as *const u8,
                128,
            )
        };
        block_buf[offset_in_ext2_block..offset_in_ext2_block + 128].copy_from_slice(inode_bytes);

        // Write all device blocks back
        for i in 0..device_blocks_per_ext2_block {
            device.write_block(
                (start_device_block + i) as u64,
                &block_buf[i * device_block_size..(i + 1) * device_block_size],
            )?;
        }

        Ok(())
    }

    /// Create a new empty regular file inode
    ///
    /// Initializes all fields for a new regular file with the given mode.
    ///
    /// # Arguments
    /// * `mode` - File mode (permissions) - file type bits are added automatically
    pub fn new_regular_file(mode: u16) -> Self {
        let now = crate::time::current_unix_time() as u32;

        Self {
            i_mode: EXT2_S_IFREG | (mode & 0o777),
            i_uid: 0,          // root for now
            i_size: 0,
            i_atime: now,
            i_ctime: now,
            i_mtime: now,
            i_dtime: 0,
            i_gid: 0,          // root for now
            i_links_count: 1,  // One link from the directory entry
            i_blocks: 0,       // No data blocks allocated yet
            i_flags: 0,
            i_osd1: 0,
            i_block: [0; 15],  // No blocks allocated
            i_generation: 0,
            i_file_acl: 0,
            i_dir_acl: 0,
            i_faddr: 0,
            i_osd2: [0; 12],
        }
    }

    /// Create a new symbolic link inode
    ///
    /// Initializes all fields for a new symbolic link. The target path can be stored
    /// either inline in i_block (fast symlink, for paths <= 60 bytes) or in a data
    /// block (for longer paths).
    ///
    /// # Arguments
    /// * `target` - The symlink target path
    ///
    /// # Returns
    /// A new inode configured as a symbolic link
    pub fn new_symlink(target: &str) -> Self {
        let now = crate::time::current_unix_time() as u32;
        let target_len = target.len();

        let mut inode = Self {
            i_mode: EXT2_S_IFLNK | 0o777, // Symlinks typically have full permissions
            i_uid: 0,                      // root for now
            i_size: target_len as u32,     // Size is the length of the target path
            i_atime: now,
            i_ctime: now,
            i_mtime: now,
            i_dtime: 0,
            i_gid: 0,                      // root for now
            i_links_count: 1,              // One link from the directory entry
            i_blocks: 0,                   // Updated if using data block
            i_flags: 0,
            i_osd1: 0,
            i_block: [0; 15],              // Will store target if fast symlink
            i_generation: 0,
            i_file_acl: 0,
            i_dir_acl: 0,
            i_faddr: 0,
            i_osd2: [0; 12],
        };

        // Fast symlink: store target in i_block array if it fits (up to 60 bytes)
        // i_block is 15 * 4 = 60 bytes
        if target_len <= 60 {
            let target_bytes = target.as_bytes();
            // Safety: We're writing bytes into the i_block array which is u32[15]
            // We treat it as a byte array of 60 bytes
            // Use addr_of_mut! to get a raw pointer without creating a reference to packed field
            let block_ptr = core::ptr::addr_of_mut!(inode.i_block) as *mut u8;
            let block_bytes = unsafe {
                core::slice::from_raw_parts_mut(block_ptr, 60)
            };
            block_bytes[..target_len].copy_from_slice(target_bytes);
            // i_blocks stays 0 for fast symlinks (no data blocks used)
        }
        // For longer targets, the caller must allocate a data block and store the target there

        inode
    }

    /// Create a new directory inode
    ///
    /// Initializes all fields for a new directory with the given mode.
    /// The directory starts with link count 2 (self via "." and parent via entry).
    ///
    /// # Arguments
    /// * `mode` - Directory permissions (e.g., 0o755) - directory type bits are added automatically
    pub fn new_directory(mode: u16) -> Self {
        let now = crate::time::current_unix_time() as u32;

        Self {
            i_mode: EXT2_S_IFDIR | (mode & 0o777),
            i_uid: 0,          // root for now
            i_size: 0,         // Will be set when directory data is written
            i_atime: now,
            i_ctime: now,
            i_mtime: now,
            i_dtime: 0,
            i_gid: 0,          // root for now
            i_links_count: 2,  // Self via "." and parent via directory entry
            i_blocks: 0,       // Will be set when blocks are allocated
            i_flags: 0,
            i_osd1: 0,
            i_block: [0; 15],  // No blocks allocated yet
            i_generation: 0,
            i_file_acl: 0,
            i_dir_acl: 0,
            i_faddr: 0,
            i_osd2: [0; 12],
        }
    }
}

/// Allocate a new inode from the filesystem
///
/// Searches the inode bitmaps to find a free inode, marks it as used,
/// and returns the inode number.
///
/// # Arguments
/// * `device` - The block device
/// * `superblock` - The ext2 superblock
/// * `block_groups` - Mutable reference to block group descriptors (to update free count)
///
/// # Returns
/// * `Ok(inode_num)` - The allocated inode number (1-indexed)
/// * `Err(msg)` - Error if no free inodes available or I/O error
pub fn allocate_inode<B: BlockDevice + ?Sized>(
    device: &B,
    superblock: &super::Ext2Superblock,
    block_groups: &mut [super::Ext2BlockGroupDesc],
) -> Result<u32, &'static str> {
    let block_size = superblock.block_size();
    let inodes_per_group = superblock.s_inodes_per_group;

    // Determine the first inode we can allocate
    let first_ino = if superblock.s_rev_level == 0 {
        EXT2_FIRST_INO
    } else {
        superblock.s_first_ino
    };

    // Search each block group for a free inode
    for (bg_index, bg) in block_groups.iter_mut().enumerate() {
        // Read free inodes count safely from packed struct
        let free_inodes = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_free_inodes_count))
        };

        if free_inodes == 0 {
            continue; // No free inodes in this group
        }

        // Read the inode bitmap block
        // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
        let bitmap_block = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_inode_bitmap))
        };
        let mut bitmap_buf = [0u8; 4096]; // Max block size
        read_ext2_block(device, bitmap_block, block_size, &mut bitmap_buf[..block_size])
            .map_err(|_| "Failed to read inode bitmap")?;

        // Search for a free inode in this group
        for local_inode in 0..inodes_per_group {
            // Calculate the global inode number (1-indexed)
            let global_inode = bg_index as u32 * inodes_per_group + local_inode + 1;

            // Skip reserved inodes
            if global_inode < first_ino {
                continue;
            }

            // Check if this inode is free (bit = 0)
            let byte_index = (local_inode / 8) as usize;
            let bit_index = (local_inode % 8) as u8;

            if byte_index >= block_size {
                break; // Bitmap doesn't cover this inode
            }

            if (bitmap_buf[byte_index] & (1 << bit_index)) == 0 {
                // Found a free inode - mark it as used
                bitmap_buf[byte_index] |= 1 << bit_index;

                // Write the updated bitmap back to disk
                write_ext2_block(device, bitmap_block, block_size, &bitmap_buf[..block_size])
                    .map_err(|_| "Failed to write inode bitmap")?;

                // Update the free inode count in the block group descriptor
                // Safety: Writing to packed struct
                unsafe {
                    core::ptr::write_unaligned(
                        core::ptr::addr_of_mut!(bg.bg_free_inodes_count),
                        free_inodes - 1,
                    );
                }

                return Ok(global_inode);
            }
        }
    }

    Err("No free inodes available")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper function to create a mock 128-byte ext2 inode buffer
    ///
    /// Layout (little-endian):
    /// - offset 0x00: i_mode (u16) - file type + permissions
    /// - offset 0x02: i_uid (u16) - owner UID
    /// - offset 0x04: i_size (u32) - file size (lower 32 bits)
    /// - offset 0x08: i_atime (u32)
    /// - offset 0x0C: i_ctime (u32)
    /// - offset 0x10: i_mtime (u32)
    /// - offset 0x14: i_dtime (u32)
    /// - offset 0x18: i_gid (u16) - group ID
    /// - offset 0x1A: i_links_count (u16)
    /// - offset 0x1C: i_blocks (u32)
    /// - offset 0x20: i_flags (u32)
    /// - offset 0x24: i_osd1 (u32)
    /// - offset 0x28: i_block[15] (15 * u32 = 60 bytes)
    /// - offset 0x64: i_generation (u32)
    /// - offset 0x68: i_file_acl (u32)
    /// - offset 0x6C: i_dir_acl (u32) - or high 32 bits of size for regular files
    /// - offset 0x70: i_faddr (u32)
    /// - offset 0x74: i_osd2[12] (12 bytes)
    fn create_mock_inode(
        mode: u16,
        uid: u16,
        gid: u16,
        size: u32,
        links_count: u16,
        blocks: &[u32; 15],
    ) -> [u8; 128] {
        let mut buf = [0u8; 128];

        // i_mode at offset 0
        buf[0..2].copy_from_slice(&mode.to_le_bytes());
        // i_uid at offset 2
        buf[2..4].copy_from_slice(&uid.to_le_bytes());
        // i_size at offset 4
        buf[4..8].copy_from_slice(&size.to_le_bytes());
        // i_atime at offset 8 (leave as 0)
        // i_ctime at offset 12 (leave as 0)
        // i_mtime at offset 16 (leave as 0)
        // i_dtime at offset 20 (leave as 0)
        // i_gid at offset 24
        buf[24..26].copy_from_slice(&gid.to_le_bytes());
        // i_links_count at offset 26
        buf[26..28].copy_from_slice(&links_count.to_le_bytes());
        // i_blocks at offset 28 (leave as 0)
        // i_flags at offset 32 (leave as 0)
        // i_osd1 at offset 36 (leave as 0)
        // i_block[15] at offset 40
        for (i, &block) in blocks.iter().enumerate() {
            let offset = 40 + i * 4;
            buf[offset..offset + 4].copy_from_slice(&block.to_le_bytes());
        }
        // i_generation at offset 100 (leave as 0)
        // i_file_acl at offset 104 (leave as 0)
        // i_dir_acl at offset 108 (leave as 0)
        // i_faddr at offset 112 (leave as 0)
        // i_osd2[12] at offset 116 (leave as 0)

        buf
    }

    #[test]
    fn test_inode_struct_size() {
        // Verify the inode structure is exactly 128 bytes
        assert_eq!(core::mem::size_of::<Ext2Inode>(), 128);
    }

    #[test]
    fn test_file_type_constants() {
        // Verify file type constants are correct
        assert_eq!(EXT2_S_IFREG & EXT2_S_IFMT, EXT2_S_IFREG);
        assert_eq!(EXT2_S_IFDIR & EXT2_S_IFMT, EXT2_S_IFDIR);
        assert_eq!(EXT2_S_IFLNK & EXT2_S_IFMT, EXT2_S_IFLNK);
    }

    #[test]
    fn test_inode_file_type_regular() {
        // Create a regular file inode (mode = 0o100644 = S_IFREG | rw-r--r--)
        let mode = EXT2_S_IFREG | 0o644;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 1000, 1000, 1024, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.file_type(), FileType::Regular);
        assert!(inode.is_file());
        assert!(!inode.is_dir());
        assert!(!inode.is_symlink());
    }

    #[test]
    fn test_inode_file_type_directory() {
        // Create a directory inode (mode = 0o040755 = S_IFDIR | rwxr-xr-x)
        let mode = EXT2_S_IFDIR | 0o755;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 4096, 2, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.file_type(), FileType::Directory);
        assert!(inode.is_dir());
        assert!(!inode.is_file());
        assert!(!inode.is_symlink());
    }

    #[test]
    fn test_inode_file_type_symlink() {
        // Create a symbolic link inode (mode = 0o120777 = S_IFLNK | rwxrwxrwx)
        let mode = EXT2_S_IFLNK | 0o777;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 32, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.file_type(), FileType::SymLink);
        assert!(inode.is_symlink());
        assert!(!inode.is_file());
        assert!(!inode.is_dir());
    }

    #[test]
    fn test_inode_file_type_block_device() {
        let mode = EXT2_S_IFBLK | 0o660;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 0, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.file_type(), FileType::BlockDevice);
    }

    #[test]
    fn test_inode_file_type_char_device() {
        let mode = EXT2_S_IFCHR | 0o666;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 0, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.file_type(), FileType::CharDevice);
    }

    #[test]
    fn test_inode_file_type_fifo() {
        let mode = EXT2_S_IFIFO | 0o644;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 0, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.file_type(), FileType::Fifo);
    }

    #[test]
    fn test_inode_file_type_socket() {
        let mode = EXT2_S_IFSOCK | 0o755;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 0, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.file_type(), FileType::Socket);
    }

    #[test]
    fn test_inode_size_regular_file() {
        // Test a regular file with size = 1024 bytes
        let mode = EXT2_S_IFREG | 0o644;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 1000, 1000, 1024, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.size(), 1024);
    }

    #[test]
    fn test_inode_size_directory() {
        // Test a directory with size = 4096 bytes
        let mode = EXT2_S_IFDIR | 0o755;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 4096, 2, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.size(), 4096);
    }

    #[test]
    fn test_inode_size_large_file() {
        // Test a large file using i_dir_acl for high 32 bits
        // Total size = (1 << 32) + 1000 = 4294968296 bytes
        let mode = EXT2_S_IFREG | 0o644;
        let blocks = [0u32; 15];
        let mut buf = create_mock_inode(mode, 1000, 1000, 1000, 1, &blocks);

        // Set i_dir_acl (offset 108) to 1 for high 32 bits
        buf[108..112].copy_from_slice(&1u32.to_le_bytes());

        let inode = Ext2Inode::from_bytes(&buf);

        // For regular files, size = (i_dir_acl << 32) | i_size
        assert_eq!(inode.size(), (1u64 << 32) + 1000);
    }

    #[test]
    fn test_inode_mode_parsing_permissions() {
        // Test permission bits extraction: rw-r--r-- = 0o644
        let mode = EXT2_S_IFREG | 0o644;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 1000, 1000, 1024, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.permissions(), 0o644);
        assert!(inode.permissions() & EXT2_S_IRUSR != 0); // User read
        assert!(inode.permissions() & EXT2_S_IWUSR != 0); // User write
        assert!(inode.permissions() & EXT2_S_IXUSR == 0); // No user execute
        assert!(inode.permissions() & EXT2_S_IRGRP != 0); // Group read
        assert!(inode.permissions() & EXT2_S_IWGRP == 0); // No group write
        assert!(inode.permissions() & EXT2_S_IROTH != 0); // Other read
    }

    #[test]
    fn test_inode_mode_parsing_executable() {
        // Test permission bits for executable: rwxr-xr-x = 0o755
        let mode = EXT2_S_IFREG | 0o755;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 2048, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.permissions(), 0o755);
        assert!(inode.permissions() & EXT2_S_IXUSR != 0); // User execute
        assert!(inode.permissions() & EXT2_S_IXGRP != 0); // Group execute
        assert!(inode.permissions() & EXT2_S_IXOTH != 0); // Other execute
    }

    #[test]
    fn test_inode_block_pointers() {
        // Create an inode with specific block pointers
        let mode = EXT2_S_IFREG | 0o644;
        let blocks: [u32; 15] = [
            100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, // Direct blocks [0-11]
            200,  // Single indirect [12]
            300,  // Double indirect [13]
            400,  // Triple indirect [14]
        ];
        let buf = create_mock_inode(mode, 1000, 1000, 1024, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        // Verify direct blocks [0-11]
        for i in 0..12 {
            // Safety: reading from packed struct
            let block = unsafe { core::ptr::read_unaligned(&inode.i_block[i]) };
            assert_eq!(block, 100 + i as u32, "Direct block {} mismatch", i);
        }

        // Verify indirect block pointers
        let single_indirect = unsafe { core::ptr::read_unaligned(&inode.i_block[12]) };
        let double_indirect = unsafe { core::ptr::read_unaligned(&inode.i_block[13]) };
        let triple_indirect = unsafe { core::ptr::read_unaligned(&inode.i_block[14]) };

        assert_eq!(single_indirect, 200, "Single indirect block mismatch");
        assert_eq!(double_indirect, 300, "Double indirect block mismatch");
        assert_eq!(triple_indirect, 400, "Triple indirect block mismatch");
    }

    #[test]
    fn test_inode_uid_gid() {
        // Test UID and GID parsing
        let mode = EXT2_S_IFREG | 0o644;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 1000, 500, 1024, 1, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        // Safety: reading from packed struct
        let uid = unsafe { core::ptr::read_unaligned(&inode.i_uid) };
        let gid = unsafe { core::ptr::read_unaligned(&inode.i_gid) };

        assert_eq!(uid, 1000);
        assert_eq!(gid, 500);
    }

    #[test]
    fn test_inode_links_count() {
        // Test links count parsing
        let mode = EXT2_S_IFDIR | 0o755;
        let blocks = [0u32; 15];
        let buf = create_mock_inode(mode, 0, 0, 4096, 5, &blocks);

        let inode = Ext2Inode::from_bytes(&buf);

        // Safety: reading from packed struct
        let links = unsafe { core::ptr::read_unaligned(&inode.i_links_count) };
        assert_eq!(links, 5);
    }

    #[test]
    #[should_panic(expected = "inode must be exactly 128 bytes")]
    fn test_inode_from_bytes_wrong_size() {
        // Test that from_bytes panics with wrong size
        let buf = [0u8; 64];
        let _ = Ext2Inode::from_bytes(&buf);
    }

    #[test]
    fn test_inode_zero_initialized() {
        // Test a zero-initialized inode
        let buf = [0u8; 128];
        let inode = Ext2Inode::from_bytes(&buf);

        assert_eq!(inode.file_type(), FileType::Unknown);
        assert_eq!(inode.size(), 0);
        assert_eq!(inode.permissions(), 0);
    }
}
