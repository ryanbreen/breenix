//! ext2 filesystem implementation
//!
//! The Second Extended Filesystem (ext2) is a classic Linux filesystem.
//! This module provides structures and functions for parsing ext2 filesystems.

pub mod superblock;
pub mod block_group;
pub mod dir;
pub mod inode;
pub mod file;

pub use superblock::*;
pub use block_group::*;
pub use dir::*;
pub use inode::*;
pub use file::*;

use crate::block::virtio::VirtioBlockWrapper;
use crate::block::BlockDevice;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// A mounted ext2 filesystem instance
///
/// Holds the superblock, block group descriptors, and a reference
/// to the underlying block device for filesystem operations.
pub struct Ext2Fs {
    /// The filesystem superblock
    pub superblock: Ext2Superblock,
    /// Block group descriptors
    pub block_groups: Vec<Ext2BlockGroupDesc>,
    /// The underlying block device
    pub device: Arc<VirtioBlockWrapper>,
    /// Mount ID for VFS integration
    pub mount_id: usize,
}

impl Ext2Fs {
    /// Create a new ext2 filesystem instance from a block device
    ///
    /// Reads and validates the superblock and block group descriptors.
    pub fn new(device: Arc<VirtioBlockWrapper>, mount_id: usize) -> Result<Self, &'static str> {
        // Read the superblock
        let superblock = Ext2Superblock::read_from(device.as_ref())
            .map_err(|_| "Failed to read ext2 superblock")?;

        if !superblock.is_valid() {
            return Err("Invalid ext2 magic number");
        }

        // Read block group descriptors
        let block_groups = Ext2BlockGroupDesc::read_table(device.as_ref(), &superblock)
            .map_err(|_| "Failed to read block group descriptors")?;

        Ok(Self {
            superblock,
            block_groups,
            device,
            mount_id,
        })
    }

    /// Read an inode from the filesystem
    pub fn read_inode(&self, inode_num: u32) -> Result<Ext2Inode, &'static str> {
        Ext2Inode::read_from(
            self.device.as_ref(),
            inode_num,
            &self.superblock,
            &self.block_groups,
        )
        .map_err(|_| "Failed to read inode")
    }

    /// Read directory entries from an inode
    ///
    /// Returns the raw directory data for parsing with DirReader.
    pub fn read_directory(&self, inode: &Ext2Inode) -> Result<Vec<u8>, &'static str> {
        if !inode.is_dir() {
            return Err("Not a directory");
        }
        read_file(self.device.as_ref(), inode, &self.superblock)
            .map_err(|_| "Failed to read directory data")
    }

    /// Look up a path component in a directory
    ///
    /// Returns the inode number of the matching entry, or None if not found.
    pub fn lookup_in_dir(&self, dir_inode: &Ext2Inode, name: &str) -> Result<Option<u32>, &'static str> {
        let dir_data = self.read_directory(dir_inode)?;
        Ok(find_entry(&dir_data, name).map(|entry| entry.inode))
    }

    /// Resolve a path to an inode number
    ///
    /// Walks the directory tree from root, looking up each path component.
    /// Supports absolute paths starting with "/".
    pub fn resolve_path(&self, path: &str) -> Result<u32, &'static str> {
        // Must start with "/"
        if !path.starts_with('/') {
            return Err("Path must be absolute");
        }

        // Start at root inode (always inode 2 in ext2)
        let mut current_inode_num = EXT2_ROOT_INO;

        // Split path into components, skipping empty parts
        for component in path.split('/').filter(|s| !s.is_empty()) {
            // Read the current directory inode
            let current_inode = self.read_inode(current_inode_num)?;

            // Make sure it's a directory
            if !current_inode.is_dir() {
                return Err("Not a directory in path");
            }

            // Look up the component in this directory
            match self.lookup_in_dir(&current_inode, component)? {
                Some(inode_num) => {
                    current_inode_num = inode_num;
                }
                None => {
                    return Err("Path component not found");
                }
            }
        }

        Ok(current_inode_num)
    }

    /// Read file content from an inode
    pub fn read_file_content(&self, inode: &Ext2Inode) -> Result<Vec<u8>, &'static str> {
        read_file(self.device.as_ref(), inode, &self.superblock)
            .map_err(|_| "Failed to read file content")
    }

    /// Read a range of file content from an inode
    pub fn read_file_range(
        &self,
        inode: &Ext2Inode,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, &'static str> {
        read_file_range(self.device.as_ref(), inode, &self.superblock, offset, length)
            .map_err(|_| "Failed to read file range")
    }

    /// Write data to a file at the specified offset
    ///
    /// # Arguments
    /// * `inode_num` - The inode number of the file to write to
    /// * `offset` - Starting byte offset within the file
    /// * `data` - Data to write
    ///
    /// # Returns
    /// * `Ok(bytes_written)` - Number of bytes written
    /// * `Err(msg)` - Error message if write failed
    pub fn write_file_range(
        &self,
        inode_num: u32,
        offset: u64,
        data: &[u8],
    ) -> Result<usize, &'static str> {
        if data.is_empty() {
            return Ok(0);
        }

        // Read the inode
        let mut inode = self.read_inode(inode_num)?;

        // Verify it's a regular file
        if !inode.is_file() {
            return Err("Not a regular file");
        }

        // Write the data
        write_file_range(self.device.as_ref(), &mut inode, &self.superblock, offset, data)
            .map_err(|_| "Failed to write file data")?;

        // Write the modified inode back to disk
        inode.write_to(self.device.as_ref(), inode_num, &self.superblock, &self.block_groups)
            .map_err(|_| "Failed to write inode")?;

        Ok(data.len())
    }

    /// Create a new file in the filesystem
    ///
    /// # Arguments
    /// * `parent_inode_num` - Inode number of the parent directory
    /// * `name` - Name of the new file
    /// * `mode` - File permission bits (0o644, 0o755, etc.)
    ///
    /// # Returns
    /// * `Ok(inode_num)` - The inode number of the newly created file
    /// * `Err(msg)` - Error message if creation failed
    pub fn create_file(&mut self, parent_inode_num: u32, name: &str, mode: u16) -> Result<u32, &'static str> {
        // Validate name
        if name.is_empty() || name.len() > 255 {
            return Err("Invalid filename length");
        }
        if name.contains('/') || name == "." || name == ".." {
            return Err("Invalid filename");
        }

        // Read the parent directory inode
        let parent_inode = self.read_inode(parent_inode_num)?;
        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Read the parent directory data
        let mut dir_data = self.read_directory(&parent_inode)?;

        // Check if the file already exists
        if find_entry(&dir_data, name).is_some() {
            return Err("File already exists");
        }

        // Allocate a new inode
        let new_inode_num = allocate_inode(
            self.device.as_ref(),
            &self.superblock,
            &mut self.block_groups,
        )?;

        // Create the new inode structure
        let new_inode = Ext2Inode::new_regular_file(mode);

        // Write the new inode to disk
        new_inode.write_to(
            self.device.as_ref(),
            new_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write new inode")?;

        // Add directory entry
        add_directory_entry(&mut dir_data, new_inode_num, name, EXT2_FT_REG_FILE)?;

        // Write the modified directory data back
        self.write_directory_data(parent_inode_num, &dir_data)?;

        // Update superblock with new free inode count
        self.superblock.decrement_free_inodes();
        self.superblock.write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;

        // Write updated block group descriptors
        Ext2BlockGroupDesc::write_table(
            self.device.as_ref(),
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write block group descriptors")?;

        log::debug!("ext2: created file '{}' with inode {}", name, new_inode_num);
        Ok(new_inode_num)
    }

    /// Truncate a file to zero length
    ///
    /// Frees all data blocks and sets the file size to 0.
    ///
    /// # Arguments
    /// * `inode_num` - Inode number of the file to truncate
    ///
    /// # Returns
    /// * `Ok(())` - File was successfully truncated
    /// * `Err(msg)` - Error message if truncation failed
    pub fn truncate_file(&mut self, inode_num: u32) -> Result<(), &'static str> {
        // Read the inode
        let mut inode = self.read_inode(inode_num)?;

        // Verify it's a regular file
        if !inode.is_file() {
            return Err("Not a regular file");
        }

        // For now, just set size to 0 and clear block pointers
        // A full implementation would also free the data blocks in the block bitmap
        inode.i_size = 0;
        inode.i_dir_acl = 0; // Clear high bits of size
        inode.i_blocks = 0;

        // Clear all block pointers
        inode.i_block = [0; 15];

        // Update modification time
        inode.i_mtime = crate::time::current_unix_time() as u32;

        // Write the modified inode back
        inode.write_to(
            self.device.as_ref(),
            inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write truncated inode")?;

        log::debug!("ext2: truncated inode {} to zero length", inode_num);
        Ok(())
    }

    /// Unlink (delete) a file from the filesystem
    ///
    /// This removes the directory entry and decrements the inode's link count.
    /// If the link count reaches 0, the inode and its data blocks are freed.
    ///
    /// # Arguments
    /// * `path` - Absolute path to the file to unlink
    ///
    /// # Returns
    /// * `Ok(())` - File was successfully unlinked
    /// * `Err(msg)` - Error message
    pub fn unlink_file(&mut self, path: &str) -> Result<(), &'static str> {
        // Must start with "/"
        if !path.starts_with('/') {
            return Err("Path must be absolute");
        }

        // Split path into parent directory and filename
        let (parent_path, filename) = match path.rfind('/') {
            Some(0) => ("/", &path[1..]), // File in root directory
            Some(idx) => (&path[..idx], &path[idx + 1..]),
            None => return Err("Invalid path"),
        };

        // Filename cannot be empty or contain special names
        if filename.is_empty() || filename == "." || filename == ".." {
            return Err("Invalid filename");
        }

        // Resolve parent directory
        let parent_inode_num = self.resolve_path(parent_path)?;
        let parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Read the parent directory data
        let mut dir_data = self.read_directory(&parent_inode)?;

        // Find the entry to verify it exists and get its inode
        let entry = find_entry(&dir_data, filename).ok_or("File not found")?;
        let target_inode_num = entry.inode;

        // Check that we're not unlinking a directory (use rmdir for that)
        let target_inode = self.read_inode(target_inode_num)?;
        if target_inode.is_dir() {
            return Err("Cannot unlink directory (use rmdir)");
        }

        // Remove the directory entry
        remove_entry(&mut dir_data, filename)?;

        // Write the modified directory data back
        self.write_directory_data(parent_inode_num, &dir_data)?;

        // Decrement the inode link count (may free the inode if it reaches 0)
        decrement_inode_links(
            self.device.as_ref(),
            target_inode_num,
            &self.superblock,
            &mut self.block_groups,
        )?;

        log::debug!("ext2: unlinked {} (inode {})", path, target_inode_num);
        Ok(())
    }

    /// Write directory data back to disk
    ///
    /// Helper function to write modified directory contents back to the directory's data blocks.
    fn write_directory_data(&self, dir_inode_num: u32, data: &[u8]) -> Result<(), &'static str> {
        // Read the directory inode
        let inode = self.read_inode(dir_inode_num)?;

        if !inode.is_dir() {
            return Err("Not a directory");
        }

        // Get the direct block pointers
        let i_block = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block))
        };

        let block_size = self.superblock.block_size();
        let mut offset = 0usize;

        // Write to each direct block
        for i in 0..12 {
            if offset >= data.len() {
                break;
            }

            let block_num = i_block[i];
            if block_num == 0 {
                break;
            }

            // Calculate how much data to write to this block
            let bytes_to_write = core::cmp::min(block_size, data.len() - offset);

            // Prepare block buffer
            let mut block_buf = alloc::vec![0u8; block_size];
            block_buf[..bytes_to_write].copy_from_slice(&data[offset..offset + bytes_to_write]);

            // Write the block
            self.device.write_block(block_num as u64, &block_buf)
                .map_err(|_| "Failed to write directory block")?;

            offset += bytes_to_write;
        }

        Ok(())
    }
}

/// Global mounted ext2 root filesystem
static ROOT_EXT2: Mutex<Option<Ext2Fs>> = Mutex::new(None);

/// Initialize the root ext2 filesystem
///
/// Mounts the primary VirtIO block device as the root filesystem.
/// This should be called during kernel initialization after VirtIO
/// block device initialization.
pub fn init_root_fs() -> Result<(), &'static str> {
    // Get the primary VirtIO block device
    let device = VirtioBlockWrapper::primary()
        .ok_or("No VirtIO block device available")?;
    let device = Arc::new(device);

    // Register with VFS mount system
    let mount_id = crate::fs::vfs::mount("/", "ext2");

    // Create the ext2 filesystem instance
    let fs = Ext2Fs::new(device, mount_id)?;

    // Read packed struct fields safely before logging
    let blocks_count = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(fs.superblock.s_blocks_count))
    };
    let inodes_count = unsafe {
        core::ptr::read_unaligned(core::ptr::addr_of!(fs.superblock.s_inodes_count))
    };
    log::info!(
        "ext2: Mounted root filesystem - {} blocks, {} inodes, block size {}",
        blocks_count,
        inodes_count,
        fs.superblock.block_size()
    );

    // Store globally
    *ROOT_EXT2.lock() = Some(fs);

    Ok(())
}

/// Access the root ext2 filesystem
///
/// Returns None if the filesystem hasn't been initialized yet.
pub fn root_fs() -> spin::MutexGuard<'static, Option<Ext2Fs>> {
    ROOT_EXT2.lock()
}

/// Check if the root filesystem is mounted
pub fn is_mounted() -> bool {
    ROOT_EXT2.lock().is_some()
}
