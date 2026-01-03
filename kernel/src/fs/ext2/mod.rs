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
