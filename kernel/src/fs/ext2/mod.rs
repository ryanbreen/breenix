//! ext2 filesystem implementation
//!
//! The Second Extended Filesystem (ext2) is a classic Linux filesystem.
//! This module provides structures and functions for parsing ext2 filesystems.

pub mod block_group;
pub mod dir;
pub mod file;
pub mod inode;
pub mod superblock;

pub use block_group::*;
pub use dir::*;
pub use file::*;
pub use inode::*;
pub use superblock::*;

use crate::block::BlockDevice;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

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
    pub device: alloc::boxed::Box<dyn BlockDevice>,
    /// Mount ID for VFS integration
    pub mount_id: usize,
}

impl Ext2Fs {
    /// Create a new ext2 filesystem instance from a block device
    ///
    /// Reads and validates the superblock and block group descriptors.
    pub fn new(
        device: alloc::boxed::Box<dyn BlockDevice>,
        mount_id: usize,
    ) -> Result<Self, &'static str> {
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
    pub fn lookup_in_dir(
        &self,
        dir_inode: &Ext2Inode,
        name: &str,
    ) -> Result<Option<u32>, &'static str> {
        let dir_data = self.read_directory(dir_inode)?;
        Ok(find_entry(&dir_data, name).map(|entry| entry.inode))
    }

    /// Resolve a path to an inode number, following symlinks
    ///
    /// Walks the directory tree from root, looking up each path component.
    /// Supports absolute paths starting with "/".
    /// Symlinks are followed transparently (both intermediate and final components).
    pub fn resolve_path(&self, path: &str) -> Result<u32, &'static str> {
        self.resolve_path_impl(path, true, 0)
    }

    /// Resolve a path to an inode number without following the final symlink
    ///
    /// Used by readlink() and lstat() which need the symlink inode itself.
    pub fn resolve_path_no_follow(&self, path: &str) -> Result<u32, &'static str> {
        self.resolve_path_impl(path, false, 0)
    }

    /// Internal path resolution with symlink following and depth limiting
    fn resolve_path_impl(
        &self,
        path: &str,
        follow_final: bool,
        depth: u32,
    ) -> Result<u32, &'static str> {
        const MAX_SYMLINK_DEPTH: u32 = 8;
        if depth > MAX_SYMLINK_DEPTH {
            return Err("Too many levels of symbolic links");
        }

        // Must start with "/"
        if !path.starts_with('/') {
            return Err("Path must be absolute");
        }

        // Start at root inode (always inode 2 in ext2)
        let mut current_inode_num = EXT2_ROOT_INO;

        // Collect components so we can detect the final one
        let components: alloc::vec::Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        for (i, component) in components.iter().enumerate() {
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

                    // Check if this resolved to a symlink
                    let resolved_inode = self.read_inode(inode_num)?;
                    let is_final = i == components.len() - 1;

                    if resolved_inode.is_symlink() && (follow_final || !is_final) {
                        // Read symlink target
                        let target = self.read_symlink(inode_num)?;

                        // Build the remaining path (components after this one)
                        let remaining = if is_final {
                            alloc::string::String::new()
                        } else {
                            let mut r = alloc::string::String::from("/");
                            for (j, c) in components[i + 1..].iter().enumerate() {
                                if j > 0 {
                                    r.push('/');
                                }
                                r.push_str(c);
                            }
                            r
                        };

                        if target.starts_with('/') {
                            // Absolute symlink target
                            let mut full_path = target;
                            if !remaining.is_empty() {
                                full_path.push_str(&remaining);
                            }
                            return self.resolve_path_impl(&full_path, follow_final, depth + 1);
                        } else {
                            // Relative symlink - resolve relative to parent directory
                            let mut parent = alloc::string::String::from("/");
                            for (j, c) in components[..i].iter().enumerate() {
                                if j > 0 {
                                    parent.push('/');
                                }
                                parent.push_str(c);
                            }
                            let mut full_path = parent;
                            full_path.push('/');
                            full_path.push_str(&target);
                            if !remaining.is_empty() {
                                full_path.push_str(&remaining);
                            }
                            return self.resolve_path_impl(&full_path, follow_final, depth + 1);
                        }
                    }
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
        read_file_range(
            self.device.as_ref(),
            inode,
            &self.superblock,
            offset,
            length,
        )
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
        &mut self,
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
        if let Err(_) = write_file_range(
            self.device.as_ref(),
            &mut inode,
            &self.superblock,
            &mut self.block_groups,
            offset,
            data,
        ) {
            return Err("Failed to write file data");
        }

        // Write the modified inode back to disk
        if let Err(_) = inode.write_to(
            self.device.as_ref(),
            inode_num,
            &self.superblock,
            &self.block_groups,
        ) {
            return Err("Failed to write inode");
        }

        Ok(data.len())
    }

    /// Write a modified inode back to disk
    ///
    /// # Arguments
    /// * `inode_num` - The inode number to write
    /// * `inode` - The modified inode data
    pub fn write_inode(&mut self, inode_num: u32, inode: &Ext2Inode) -> Result<(), &'static str> {
        inode
            .write_to(
                self.device.as_ref(),
                inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write inode")
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
    pub fn create_file(
        &mut self,
        parent_inode_num: u32,
        name: &str,
        mode: u16,
    ) -> Result<u32, &'static str> {
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
        new_inode
            .write_to(
                self.device.as_ref(),
                new_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write new inode")?;

        // Add directory entry
        add_directory_entry(&mut dir_data, new_inode_num, name, EXT2_FT_REG_FILE)?;

        // Update parent directory timestamps (mtime and ctime)
        let mut parent_inode_mut = parent_inode;
        parent_inode_mut.update_timestamps(false, true, true);

        // Write the modified directory data back
        self.write_directory_data(parent_inode_num, &dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut
            .write_to(
                self.device.as_ref(),
                parent_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write parent inode")?;

        // Update superblock with new free inode count
        self.superblock.decrement_free_inodes();
        self.superblock
            .write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;

        // Write updated block group descriptors
        Ext2BlockGroupDesc::write_table(self.device.as_ref(), &self.superblock, &self.block_groups)
            .map_err(|_| "Failed to write block group descriptors")?;

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

        // Free all allocated data blocks before clearing pointers
        // This prevents block leaks where blocks remain marked "in use" but are unreachable
        let i_block = inode.i_block;

        // Free direct blocks (0-11) and count how many were freed
        let mut blocks_freed: u32 = 0;
        for i in 0..12 {
            if i_block[i] != 0 {
                if block_group::free_block(
                    self.device.as_ref(),
                    i_block[i],
                    &self.superblock,
                    &mut self.block_groups,
                )
                .is_ok()
                {
                    blocks_freed += 1;
                }
            }
        }

        // TODO: Free indirect blocks (single, double, triple) for large files
        // For now, just handle direct blocks which covers files up to 12KB (1KB blocks)
        // or 48KB (4KB blocks)

        inode.i_size = 0;
        inode.i_dir_acl = 0; // Clear high bits of size
        inode.i_blocks = 0;

        // Clear all block pointers
        inode.i_block = [0; 15];

        // Update modification and change timestamps
        inode.update_timestamps(false, true, true);

        // Write the modified inode back
        inode
            .write_to(
                self.device.as_ref(),
                inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write truncated inode")?;

        // Update superblock free block count so freed blocks can be reused
        if blocks_freed > 0 {
            self.superblock.increment_free_blocks(blocks_freed);
            self.superblock
                .write_to(self.device.as_ref())
                .map_err(|_| "Failed to write superblock after truncate")?;
        }

        log::debug!(
            "ext2: truncated inode {} to zero length, freed {} blocks",
            inode_num,
            blocks_freed
        );
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

        // Get the link count to determine if we'll be freeing the inode
        let link_count =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(target_inode.i_links_count)) };

        // Calculate how many blocks this file uses (if we're about to free it)
        // i_blocks is in 512-byte sectors, so divide by (block_size / 512)
        let blocks_to_free = if link_count == 1 {
            let block_size = self.superblock.block_size();
            let sectors_per_block = (block_size / 512) as u32;
            let i_blocks =
                unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(target_inode.i_blocks)) };
            i_blocks / sectors_per_block
        } else {
            0
        };

        // Remove the directory entry
        remove_entry(&mut dir_data, filename)?;

        // Update parent directory timestamps (mtime and ctime)
        let mut parent_inode_mut = parent_inode;
        parent_inode_mut.update_timestamps(false, true, true);

        // Write the modified directory data back
        self.write_directory_data(parent_inode_num, &dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut
            .write_to(
                self.device.as_ref(),
                parent_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write parent inode")?;

        // Decrement the inode link count (may free the inode and blocks if it reaches 0)
        let new_links = decrement_inode_links(
            self.device.as_ref(),
            target_inode_num,
            &self.superblock,
            &mut self.block_groups,
        )?;

        // If the inode was freed, update the superblock's free counts
        if new_links == 0 {
            // Update superblock free inode count
            self.superblock.increment_free_inodes();

            // Update superblock free block count
            if blocks_to_free > 0 {
                self.superblock.increment_free_blocks(blocks_to_free);
            }

            // Write the updated superblock
            self.superblock
                .write_to(self.device.as_ref())
                .map_err(|_| "Failed to write superblock")?;

            // Write updated block group descriptors
            Ext2BlockGroupDesc::write_table(
                self.device.as_ref(),
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write block group descriptors")?;
        }

        log::debug!("ext2: unlinked {} (inode {})", path, target_inode_num);
        Ok(())
    }

    /// Rename/move a file or directory
    ///
    /// Renames or moves a file/directory from oldpath to newpath.
    /// If newpath exists and is a file, it is replaced. If newpath is a directory,
    /// the operation fails.
    ///
    /// # Arguments
    /// * `oldpath` - Current absolute path
    /// * `newpath` - New absolute path
    ///
    /// # Returns
    /// * `Ok(())` - Rename was successful
    /// * `Err(msg)` - Error message
    pub fn rename_file(&mut self, oldpath: &str, newpath: &str) -> Result<(), &'static str> {
        // Both paths must be absolute
        if !oldpath.starts_with('/') || !newpath.starts_with('/') {
            return Err("Paths must be absolute");
        }

        // Cannot rename . or ..
        if oldpath.ends_with("/.") || oldpath.ends_with("/..") {
            return Err("Cannot rename . or ..");
        }

        // Split both paths into parent and filename
        let (old_parent_path, old_filename) = match oldpath.rfind('/') {
            Some(0) => ("/", &oldpath[1..]),
            Some(idx) => (&oldpath[..idx], &oldpath[idx + 1..]),
            None => return Err("Invalid oldpath"),
        };

        let (new_parent_path, new_filename) = match newpath.rfind('/') {
            Some(0) => ("/", &newpath[1..]),
            Some(idx) => (&newpath[..idx], &newpath[idx + 1..]),
            None => return Err("Invalid newpath"),
        };

        // Validate filenames
        if old_filename.is_empty() || new_filename.is_empty() {
            return Err("Invalid filename");
        }
        if old_filename == "."
            || old_filename == ".."
            || new_filename == "."
            || new_filename == ".."
        {
            return Err("Cannot rename . or ..");
        }

        // If old and new paths are the same, it's a no-op - just return success
        if oldpath == newpath {
            log::debug!("ext2: rename {} to same path (no-op)", oldpath);
            return Ok(());
        }

        // Resolve source file/directory
        let source_inode_num = self.resolve_path(oldpath)?;
        let source_inode = self.read_inode(source_inode_num)?;
        let source_is_dir = source_inode.is_dir();
        let source_file_type = if source_is_dir {
            EXT2_FT_DIR
        } else {
            EXT2_FT_REG_FILE
        };

        // Resolve parent directories
        let old_parent_num = self.resolve_path(old_parent_path)?;
        let new_parent_num = self.resolve_path(new_parent_path)?;

        let old_parent_inode = self.read_inode(old_parent_num)?;
        let new_parent_inode = self.read_inode(new_parent_num)?;

        if !old_parent_inode.is_dir() || !new_parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Check if destination exists
        let dest_exists = self.resolve_path(newpath).is_ok();

        if dest_exists {
            // Destination exists - check if we can replace it
            let dest_inode_num = self.resolve_path(newpath)?;
            let dest_inode = self.read_inode(dest_inode_num)?;

            if dest_inode.is_dir() {
                if !source_is_dir {
                    // Cannot replace directory with non-directory
                    return Err("Destination is a directory");
                } else {
                    // For directory rename, destination must be empty
                    // (we don't support this yet - would need to check if directory is empty)
                    return Err("Destination directory exists");
                }
            } else if source_is_dir {
                // Cannot replace file with directory
                return Err("Destination is a file but source is a directory");
            }

            // Destination is a file and source is a file - we'll replace it
            // First, unlink the destination
            self.unlink_file(newpath)?;
        }

        // Now perform the rename
        // Read both parent directories
        let mut old_parent_data = self.read_directory(&old_parent_inode)?;
        let mut new_parent_data = if old_parent_num == new_parent_num {
            // Same directory - use the same data buffer
            old_parent_data.clone()
        } else {
            self.read_directory(&new_parent_inode)?
        };

        // Remove entry from old parent
        remove_entry(&mut old_parent_data, old_filename)?;

        // Add entry to new parent
        if old_parent_num == new_parent_num {
            // Same directory - work with the modified buffer
            add_directory_entry(
                &mut old_parent_data,
                source_inode_num,
                new_filename,
                source_file_type,
            )?;

            // Update parent directory timestamps
            let mut parent_inode_mut = old_parent_inode;
            parent_inode_mut.update_timestamps(false, true, true);

            // Write back once
            self.write_directory_data(old_parent_num, &old_parent_data)?;

            // Write the updated parent directory inode
            parent_inode_mut
                .write_to(
                    self.device.as_ref(),
                    old_parent_num,
                    &self.superblock,
                    &self.block_groups,
                )
                .map_err(|_| "Failed to write parent inode")?;
        } else {
            // Different directories
            add_directory_entry(
                &mut new_parent_data,
                source_inode_num,
                new_filename,
                source_file_type,
            )?;

            // Update timestamps for both parent directories
            let mut old_parent_mut = old_parent_inode;
            let mut new_parent_mut = new_parent_inode;
            old_parent_mut.update_timestamps(false, true, true);
            new_parent_mut.update_timestamps(false, true, true);

            // Write both directories back
            self.write_directory_data(old_parent_num, &old_parent_data)?;
            self.write_directory_data(new_parent_num, &new_parent_data)?;

            // Write the updated parent directory inodes
            old_parent_mut
                .write_to(
                    self.device.as_ref(),
                    old_parent_num,
                    &self.superblock,
                    &self.block_groups,
                )
                .map_err(|_| "Failed to write old parent inode")?;

            new_parent_mut
                .write_to(
                    self.device.as_ref(),
                    new_parent_num,
                    &self.superblock,
                    &self.block_groups,
                )
                .map_err(|_| "Failed to write new parent inode")?;

            // If moving a directory, update its ".." entry to point to new parent
            if source_is_dir {
                let mut source_dir_data = self.read_directory(&source_inode)?;
                // Find and update the ".." entry
                update_directory_entry(&mut source_dir_data, "..", new_parent_num)?;
                self.write_directory_data(source_inode_num, &source_dir_data)?;
            }
        }

        log::debug!("ext2: renamed {} to {}", oldpath, newpath);
        Ok(())
    }

    /// Create a new directory in the filesystem
    ///
    /// Creates a new directory with the specified name in the parent directory.
    /// The new directory will have "." and ".." entries initialized.
    ///
    /// # Arguments
    /// * `path` - Absolute path for the new directory
    /// * `mode` - Directory permission bits (e.g., 0o755)
    ///
    /// # Returns
    /// * `Ok(inode_num)` - The inode number of the newly created directory
    /// * `Err(msg)` - Error message if creation failed
    pub fn create_directory(&mut self, path: &str, mode: u16) -> Result<u32, &'static str> {
        // Must be an absolute path
        if !path.starts_with('/') {
            return Err("Path must be absolute");
        }

        // Split path into parent directory and new directory name
        let (parent_path, dirname) = match path.rfind('/') {
            Some(0) => ("/", &path[1..]), // Directory in root
            Some(idx) => (&path[..idx], &path[idx + 1..]),
            None => return Err("Invalid path"),
        };

        // Validate name
        if dirname.is_empty() || dirname.len() > 255 {
            return Err("Invalid directory name length");
        }
        if dirname.contains('/') || dirname == "." || dirname == ".." {
            return Err("Invalid directory name");
        }

        // Resolve parent directory
        let parent_inode_num = self.resolve_path(parent_path)?;
        let parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Read the parent directory data
        let mut parent_dir_data = self.read_directory(&parent_inode)?;

        // Check if the directory already exists
        if find_entry(&parent_dir_data, dirname).is_some() {
            return Err("Directory already exists");
        }

        // Allocate a new inode for the directory
        let new_inode_num = allocate_inode(
            self.device.as_ref(),
            &self.superblock,
            &mut self.block_groups,
        )?;

        // Allocate a data block for the new directory's contents (. and .. entries)
        let new_block = allocate_block(
            self.device.as_ref(),
            &self.superblock,
            &mut self.block_groups,
        )?;

        // Create the new directory inode
        let mut new_inode = Ext2Inode::new_directory(mode);

        // Set the data block pointer
        new_inode.i_block[0] = new_block;

        // Set size to one block (for . and .. entries)
        let block_size = self.superblock.block_size();
        new_inode.i_size = block_size as u32;

        // Set block count (in 512-byte sectors)
        new_inode.i_blocks = (block_size / 512) as u32;

        // Initialize directory contents with "." and ".." entries
        // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
        let mut dir_data = [0u8; 4096]; // Max block size

        // Write "." entry (points to self)
        // inode (4) + rec_len (2) + name_len (1) + file_type (1) + name (1) = 9, aligned to 12
        let dot_rec_len = 12u16;
        dir_data[0..4].copy_from_slice(&new_inode_num.to_le_bytes()); // inode
        dir_data[4..6].copy_from_slice(&dot_rec_len.to_le_bytes()); // rec_len
        dir_data[6] = 1; // name_len
        dir_data[7] = EXT2_FT_DIR; // file_type
        dir_data[8] = b'.'; // name

        // Write ".." entry (points to parent)
        // This entry takes up the rest of the block
        let dotdot_offset = 12usize;
        let dotdot_rec_len = (block_size - 12) as u16;
        dir_data[dotdot_offset..dotdot_offset + 4].copy_from_slice(&parent_inode_num.to_le_bytes()); // inode
        dir_data[dotdot_offset + 4..dotdot_offset + 6]
            .copy_from_slice(&dotdot_rec_len.to_le_bytes()); // rec_len
        dir_data[dotdot_offset + 6] = 2; // name_len
        dir_data[dotdot_offset + 7] = EXT2_FT_DIR; // file_type
        dir_data[dotdot_offset + 8] = b'.'; // name[0]
        dir_data[dotdot_offset + 9] = b'.'; // name[1]

        // Write the directory data block
        file::write_ext2_block(
            self.device.as_ref(),
            new_block,
            block_size,
            &dir_data[..block_size],
        )
        .map_err(|_| "Failed to write directory data block")?;

        // Write the new inode to disk
        new_inode
            .write_to(
                self.device.as_ref(),
                new_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write new directory inode")?;

        // Add directory entry to parent directory
        add_directory_entry(&mut parent_dir_data, new_inode_num, dirname, EXT2_FT_DIR)?;

        // Increment parent's link count (for the ".." entry in the new directory)
        let mut parent_inode_mut = parent_inode;
        let current_links = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(parent_inode_mut.i_links_count))
        };
        parent_inode_mut.i_links_count = current_links + 1;

        // Update parent directory timestamps
        parent_inode_mut.update_timestamps(false, true, true);

        // Write the modified parent directory data back
        self.write_directory_data(parent_inode_num, &parent_dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut
            .write_to(
                self.device.as_ref(),
                parent_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write parent inode")?;

        // Update superblock with new free inode and block counts
        self.superblock.decrement_free_inodes();
        self.superblock.decrement_free_blocks();
        self.superblock
            .write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;

        // Update block group used directories count
        let inodes_per_group = self.superblock.s_inodes_per_group;
        let bg_index = ((new_inode_num - 1) / inodes_per_group) as usize;
        if bg_index < self.block_groups.len() {
            let bg = &mut self.block_groups[bg_index];
            let used_dirs =
                unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_used_dirs_count)) };
            unsafe {
                core::ptr::write_unaligned(
                    core::ptr::addr_of_mut!(bg.bg_used_dirs_count),
                    used_dirs + 1,
                );
            }
        }

        // Write updated block group descriptors
        Ext2BlockGroupDesc::write_table(self.device.as_ref(), &self.superblock, &self.block_groups)
            .map_err(|_| "Failed to write block group descriptors")?;

        log::debug!(
            "ext2: created directory '{}' with inode {}",
            path,
            new_inode_num
        );
        Ok(new_inode_num)
    }

    /// Remove an empty directory from the filesystem
    ///
    /// This removes the directory if it is empty (contains only "." and "..").
    /// The directory's inode is freed and the entry is removed from the parent.
    ///
    /// # Arguments
    /// * `path` - Absolute path to the directory to remove
    ///
    /// # Returns
    /// * `Ok(())` - Directory was successfully removed
    /// * `Err(msg)` - Error message
    ///
    /// # Errors
    /// * "Path must be absolute" - Path doesn't start with "/"
    /// * "Cannot remove root directory" - Tried to remove "/"
    /// * "Not a directory" - Path refers to a non-directory
    /// * "Directory not empty" - Directory contains entries other than "." and ".."
    /// * "Path component not found" - Part of the path doesn't exist
    pub fn remove_directory(&mut self, path: &str) -> Result<(), &'static str> {
        // Must start with "/"
        if !path.starts_with('/') {
            return Err("Path must be absolute");
        }

        // Cannot remove root directory
        if path == "/" {
            return Err("Cannot remove root directory");
        }

        // Split path into parent directory and directory name
        let (parent_path, dir_name) = match path.rfind('/') {
            Some(0) => ("/", &path[1..]), // Directory in root
            Some(idx) => (&path[..idx], &path[idx + 1..]),
            None => return Err("Invalid path"),
        };

        // Directory name cannot be empty or special
        if dir_name.is_empty() || dir_name == "." || dir_name == ".." {
            return Err("Invalid directory name");
        }

        // Resolve the target directory
        let target_inode_num = self.resolve_path(path)?;
        let target_inode = self.read_inode(target_inode_num)?;

        // Verify it's a directory
        if !target_inode.is_dir() {
            return Err("Not a directory");
        }

        // Read directory contents and check if empty
        let dir_data = self.read_directory(&target_inode)?;
        if !is_directory_empty(&dir_data) {
            return Err("Directory not empty");
        }

        // Resolve parent directory
        let parent_inode_num = self.resolve_path(parent_path)?;
        let parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Read the parent directory data
        let mut parent_dir_data = self.read_directory(&parent_inode)?;

        // Remove the directory entry from parent
        remove_entry(&mut parent_dir_data, dir_name)?;

        // Update parent directory timestamps (mtime and ctime)
        let mut parent_inode_mut = parent_inode;
        parent_inode_mut.update_timestamps(false, true, true);

        // Decrement parent's link count (for the ".." entry that pointed to it)
        let parent_links = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(parent_inode_mut.i_links_count))
        };
        parent_inode_mut.i_links_count = parent_links.saturating_sub(1);

        // Write the modified parent directory data back
        self.write_directory_data(parent_inode_num, &parent_dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut
            .write_to(
                self.device.as_ref(),
                parent_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write parent inode")?;

        // Free the directory's data blocks
        let i_block =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(target_inode.i_block)) };
        for block_num in i_block.iter().take(12) {
            if *block_num != 0 {
                free_block(
                    self.device.as_ref(),
                    *block_num,
                    &self.superblock,
                    &mut self.block_groups,
                )?;
            }
        }

        // Decrement the directory's inode link count (which frees the inode)
        decrement_inode_links(
            self.device.as_ref(),
            target_inode_num,
            &self.superblock,
            &mut self.block_groups,
        )?;

        // Update superblock with new free inode/block counts
        self.superblock.increment_free_inodes();
        self.superblock
            .write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;

        // Write updated block group descriptors
        Ext2BlockGroupDesc::write_table(self.device.as_ref(), &self.superblock, &self.block_groups)
            .map_err(|_| "Failed to write block group descriptors")?;

        log::debug!(
            "ext2: removed directory '{}' (inode {})",
            path,
            target_inode_num
        );
        Ok(())
    }

    /// Create a hard link to an existing file
    ///
    /// Creates a new directory entry pointing to an existing inode,
    /// incrementing the inode's link count.
    ///
    /// # Arguments
    /// * `oldpath` - Absolute path to the existing file
    /// * `newpath` - Absolute path for the new link
    ///
    /// # Returns
    /// * `Ok(())` - Hard link was created successfully
    /// * `Err(msg)` - Error message
    ///
    /// # Errors
    /// * Path not absolute
    /// * Source file not found
    /// * Source is a directory (hard links to directories not allowed)
    /// * Destination already exists
    /// * Destination parent directory not found
    /// * No space in destination directory
    pub fn create_hard_link(&mut self, oldpath: &str, newpath: &str) -> Result<(), &'static str> {
        // Both paths must be absolute
        if !oldpath.starts_with('/') || !newpath.starts_with('/') {
            return Err("Paths must be absolute");
        }

        // Resolve the source path to get the inode
        let source_inode_num = self.resolve_path(oldpath)?;
        let source_inode = self.read_inode(source_inode_num)?;

        // Hard links to directories are not allowed (prevents cycles in filesystem)
        if source_inode.is_dir() {
            return Err("Cannot create hard link to directory");
        }

        // Parse newpath to get parent directory and new name
        let (new_parent_path, new_filename) = match newpath.rfind('/') {
            Some(0) => ("/", &newpath[1..]), // File in root directory
            Some(idx) => (&newpath[..idx], &newpath[idx + 1..]),
            None => return Err("Invalid newpath"),
        };

        // Validate the new filename
        if new_filename.is_empty() || new_filename.len() > 255 {
            return Err("Invalid filename length");
        }
        if new_filename.contains('/') || new_filename == "." || new_filename == ".." {
            return Err("Invalid filename");
        }

        // Resolve the parent directory for the new link
        let new_parent_inode_num = self.resolve_path(new_parent_path)?;
        let new_parent_inode = self.read_inode(new_parent_inode_num)?;

        if !new_parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Check if the destination already exists
        if self.resolve_path(newpath).is_ok() {
            return Err("Destination already exists");
        }

        // Read the parent directory data
        let mut dir_data = self.read_directory(&new_parent_inode)?;

        // Add a new directory entry pointing to the source inode
        add_directory_entry(
            &mut dir_data,
            source_inode_num,
            new_filename,
            EXT2_FT_REG_FILE,
        )?;

        // Update parent directory timestamps
        let mut parent_inode_mut = new_parent_inode;
        parent_inode_mut.update_timestamps(false, true, true);

        // Write the modified directory data back
        self.write_directory_data(new_parent_inode_num, &dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut
            .write_to(
                self.device.as_ref(),
                new_parent_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write parent inode")?;

        // Increment the source inode's link count
        increment_inode_links(
            self.device.as_ref(),
            source_inode_num,
            &self.superblock,
            &self.block_groups,
        )?;

        log::debug!(
            "ext2: created hard link {} -> {} (inode {})",
            newpath,
            oldpath,
            source_inode_num
        );
        Ok(())
    }

    /// Create a symbolic link
    ///
    /// Creates a new symbolic link at `linkpath` pointing to `target`.
    /// For short targets (<= 60 bytes), the target is stored inline in the inode
    /// (fast symlink). For longer targets, a data block is allocated.
    ///
    /// # Arguments
    /// * `target` - The target path the symlink points to
    /// * `linkpath` - Absolute path where the symlink will be created
    ///
    /// # Returns
    /// * `Ok(())` - Symlink was created successfully
    /// * `Err(msg)` - Error message
    pub fn create_symlink(&mut self, target: &str, linkpath: &str) -> Result<(), &'static str> {
        // linkpath must be absolute
        if !linkpath.starts_with('/') {
            return Err("Path must be absolute");
        }

        // Split linkpath into parent directory and link name
        let (parent_path, link_name) = match linkpath.rfind('/') {
            Some(0) => ("/", &linkpath[1..]), // Link in root directory
            Some(idx) => (&linkpath[..idx], &linkpath[idx + 1..]),
            None => return Err("Invalid path"),
        };

        // Validate the link name
        if link_name.is_empty() || link_name.len() > 255 {
            return Err("Invalid filename length");
        }
        if link_name.contains('/') || link_name == "." || link_name == ".." {
            return Err("Invalid filename");
        }

        // Verify target is not empty
        if target.is_empty() {
            return Err("Symlink target cannot be empty");
        }

        // Resolve parent directory
        let parent_inode_num = self.resolve_path(parent_path)?;
        let parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Check if the link already exists
        if self.resolve_path(linkpath).is_ok() {
            return Err("File already exists");
        }

        // Allocate a new inode
        let new_inode_num = allocate_inode(
            self.device.as_ref(),
            &self.superblock,
            &mut self.block_groups,
        )?;

        // Create the new symlink inode
        let mut new_inode = Ext2Inode::new_symlink(target);

        // If target is > 60 bytes, we need to allocate a data block
        if target.len() > 60 {
            // Allocate a block for the target
            let block_num = allocate_block(
                self.device.as_ref(),
                &self.superblock,
                &mut self.block_groups,
            )?;

            // Write the target to the block
            // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
            let block_size = self.superblock.block_size();
            let mut block_buf = [0u8; 4096]; // Max block size
            let target_bytes = target.as_bytes();
            block_buf[..target_bytes.len()].copy_from_slice(target_bytes);

            write_ext2_block(
                self.device.as_ref(),
                block_num,
                block_size,
                &block_buf[..block_size],
            )
            .map_err(|_| "Failed to write symlink target block")?;

            // Update inode to point to this block
            new_inode.i_block[0] = block_num;
            // i_blocks is in 512-byte sectors
            new_inode.i_blocks = (block_size / 512) as u32;
        }

        // Write the new inode to disk
        new_inode
            .write_to(
                self.device.as_ref(),
                new_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write symlink inode")?;

        // Add directory entry with EXT2_FT_SYMLINK type
        let mut dir_data = self.read_directory(&parent_inode)?;
        add_directory_entry(&mut dir_data, new_inode_num, link_name, EXT2_FT_SYMLINK)?;

        // Update parent directory timestamps
        let mut parent_inode_mut = parent_inode;
        parent_inode_mut.update_timestamps(false, true, true);

        // Write the modified directory data back
        self.write_directory_data(parent_inode_num, &dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut
            .write_to(
                self.device.as_ref(),
                parent_inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write parent inode")?;

        // Update superblock with new free inode count
        self.superblock.decrement_free_inodes();
        self.superblock
            .write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;

        // Write updated block group descriptors
        Ext2BlockGroupDesc::write_table(self.device.as_ref(), &self.superblock, &self.block_groups)
            .map_err(|_| "Failed to write block group descriptors")?;

        log::debug!("ext2: created symlink '{}' -> '{}'", linkpath, target);
        Ok(())
    }

    /// Read the target of a symbolic link
    ///
    /// # Arguments
    /// * `inode_num` - The inode number of the symbolic link
    ///
    /// # Returns
    /// * `Ok(String)` - The target path the symlink points to
    /// * `Err(msg)` - Error if not a symlink or read error
    pub fn read_symlink(&self, inode_num: u32) -> Result<alloc::string::String, &'static str> {
        use alloc::string::String;

        // Read the inode
        let inode = self.read_inode(inode_num)?;

        // Verify it's a symlink
        if !inode.is_symlink() {
            return Err("Not a symbolic link");
        }

        // Get the target length from i_size
        let target_len =
            unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_size)) } as usize;

        if target_len == 0 {
            return Err("Empty symlink target");
        }

        // Check if this is a fast symlink (target stored in i_block)
        // Fast symlinks have i_blocks == 0 (no data blocks allocated)
        let i_blocks = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_blocks)) };

        if i_blocks == 0 && target_len <= 60 {
            // Fast symlink: target is stored in the i_block array
            let i_block = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block)) };

            // Convert the i_block array to bytes
            let block_bytes =
                unsafe { core::slice::from_raw_parts(i_block.as_ptr() as *const u8, 60) };

            // Extract the target string
            let target_bytes = &block_bytes[..target_len];
            String::from_utf8(target_bytes.to_vec()).map_err(|_| "Invalid UTF-8 in symlink target")
        } else {
            // Regular symlink: target is stored in a data block
            let i_block = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block)) };

            let block_num = i_block[0];
            if block_num == 0 {
                return Err("Symlink has no data block");
            }

            // Read the data block
            // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
            let block_size = self.superblock.block_size();
            let mut block_buf = [0u8; 4096]; // Max block size
            read_ext2_block(
                self.device.as_ref(),
                block_num,
                block_size,
                &mut block_buf[..block_size],
            )
            .map_err(|_| "Failed to read symlink data block")?;

            // Extract the target string
            let target_bytes = &block_buf[..target_len];
            String::from_utf8(target_bytes.to_vec()).map_err(|_| "Invalid UTF-8 in symlink target")
        }
    }

    fn write_directory_data(&self, dir_inode_num: u32, data: &[u8]) -> Result<(), &'static str> {
        // Read the directory inode
        let inode = self.read_inode(dir_inode_num)?;

        if !inode.is_dir() {
            return Err("Not a directory");
        }

        // Get the direct block pointers
        let i_block = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block)) };

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
            // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
            let mut block_buf = [0u8; 4096]; // Max block size
            block_buf[..bytes_to_write].copy_from_slice(&data[offset..offset + bytes_to_write]);

            // Write the block using ext2-to-device block conversion
            file::write_ext2_block(
                self.device.as_ref(),
                block_num,
                block_size,
                &block_buf[..block_size],
            )
            .map_err(|_| "Failed to write directory block")?;

            offset += bytes_to_write;
        }

        Ok(())
    }
}

// =============================================================================
// #728 — ext2 lock-discipline observability
// =============================================================================
//
// `ROOT_EXT2`/`HOME_EXT2` are `spin::RwLock`s whose contended acquisition
// busy-spins with no `try_*`-then-park fallback (spin 0.9.8's default relax
// strategy is a hardware pause, never an OS yield or park). The mechanism
// that makes a contended spin un-interruptible differs by arch, and both
// halves matter:
//   - On aarch64, every syscall dispatch that can contend these locks runs
//     with `preempt_count() > 0`, which is the same counter the timer
//     interrupt's own preemption decision gates on
//     (`can_schedule() == (preempt_count() == 0)`) — IRQs are normally
//     *unmasked* in an aarch64 syscall, so the timer still fires, it just
//     declines to switch away from the spinner.
//   - On x86, the syscall entry path (`syscall/entry.asm:29`, `cli`, no
//     `sti` before `rust_syscall_handler`) runs the entire syscall with
//     RFLAGS.IF = 0, so the timer interrupt cannot fire *at all* — a
//     stronger, more direct cause than the preempt_count mechanism (which
//     is also true on x86, but not the reason the spin cannot be preempted).
// Either way: the moment a contended acquisition's own holder is genuinely
// parked for block-device I/O completion (see
// `Completion::wait_timeout_uninterruptible`, `task/completion.rs`) while
// still holding the guard, a spinning contender is, for the duration of its
// spin, structurally exempt from the mechanism that would otherwise let the
// timer ISR preempt it and let the actual holder's completion get
// dispatched. See #728 for the full analysis.
//
// `ext2_spin_wait`/`ext2_spin_wait_write` below make that spin *observable*
// without changing its behavior: they still busy-spin forever with no yield
// and no park when contended — the same defect, just no longer silent. A
// spin that crosses `EXT2_SPIN_STALL_THRESHOLD_NS` prints one
// `EXT2_LOCK_SPIN_STALL` line (once per stall episode) and increments
// `EXT2_LOCK_SPIN_STALLS`, so a gate script can detect the livelock without
// a userspace watchdog — during the pathological case in question nothing
// userspace can run at all, so the spinner has to be its own observer (see
// `kernel::fs::ext2_lock_race` for the in-kernel repro leg that exercises
// this).

/// Wall-clock threshold (monotonic ns) past which a contended ext2 lock
/// acquisition is reported as stalled. Ordinary contention resolves in well
/// under a millisecond (either the fast `try_*` path succeeds immediately,
/// or a genuine holder releases quickly); a spin still running half a second
/// later is not "briefly contended," it is the #728 livelock shape.
const EXT2_SPIN_STALL_THRESHOLD_NS: u64 = 500_000_000;

/// Check the clock every 65536 spin iterations rather than every iteration,
/// so the observability itself never becomes the bottleneck it's measuring.
const EXT2_SPIN_POLL_MASK: u32 = 0xFFFF;

/// Running count of observed acquisition stalls (test/gate use). Read via
/// `ext2_lock_spin_stalls()`.
static EXT2_LOCK_SPIN_STALLS: AtomicU64 = AtomicU64::new(0);

/// Read the running count of #728 spin-stall episodes observed since boot.
///
/// Used by the `ext2_lock_race` repro leg and its gate script: on unfixed
/// (spin-only) lock code this reliably increments under a forced read/write
/// collision; on fixed code the same collision resolves by parking instead,
/// so the counter should stay flat.
pub fn ext2_lock_spin_stalls() -> u64 {
    EXT2_LOCK_SPIN_STALLS.load(Ordering::Relaxed)
}

#[inline]
fn ext2_now_ns() -> u64 {
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    secs as u64 * 1_000_000_000 + nanos as u64
}

/// Per-acquisition-attempt stall tracker shared by the read and write spin
/// loops below. Reports at most once per loop (`ext2_spin_wait`/
/// `ext2_spin_wait_write` each construct a fresh tracker per call).
struct Ext2SpinStallTracker {
    start_ns: u64,
    iterations: u32,
    reported: bool,
}

impl Ext2SpinStallTracker {
    fn new() -> Self {
        Self {
            start_ns: ext2_now_ns(),
            iterations: 0,
            reported: false,
        }
    }

    /// Call once per spin iteration. No-op after the first report, and cheap
    /// (no clock read) on all but 1-in-65536 iterations.
    fn tick(&mut self, lock_name: &str) {
        if self.reported {
            return;
        }
        self.iterations = self.iterations.wrapping_add(1);
        if self.iterations & EXT2_SPIN_POLL_MASK != 0 {
            return;
        }
        let elapsed = ext2_now_ns().saturating_sub(self.start_ns);
        if elapsed > EXT2_SPIN_STALL_THRESHOLD_NS {
            self.reported = true;
            EXT2_LOCK_SPIN_STALLS.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!(
                "EXT2_LOCK_SPIN_STALL lock={} elapsed_ns={}",
                lock_name,
                elapsed
            );
        }
    }
}

/// Busy-spin until `try_acquire` succeeds. Behaviorally identical to calling
/// `spin::RwLock`'s own blocking `.read()` — no yield, no park — except that
/// a spin exceeding `EXT2_SPIN_STALL_THRESHOLD_NS` is reported once so the
/// condition is gate-visible. This is the fallback path both before and
/// after the #728 fix: pre-fix it is the *only* path (this function's
/// behavior on unfixed code, unchanged from the plain spin), post-fix it is
/// reached only when parking is unsafe (`ext2_lock_can_sleep()` false) or
/// after a bounded number of park rounds made no progress.
fn ext2_spin_wait<T>(lock_name: &str, mut try_acquire: impl FnMut() -> Option<T>) -> T {
    let mut tracker = Ext2SpinStallTracker::new();
    loop {
        if let Some(v) = try_acquire() {
            return v;
        }
        tracker.tick(lock_name);
        core::hint::spin_loop();
    }
}

/// Busy-spin to acquire the write (upgraded) guard: first the upgradeable
/// slot, then the upgrade itself. Two phases because `spin::RwLock` only
/// allows one upgradeable-read guard at a time, and `try_upgrade()` returns
/// the guard back on failure (`Err(Self)`) rather than losing the slot.
/// Same behavioral contract as `ext2_spin_wait`: unchanged spin, now observed.
fn ext2_spin_wait_write(
    lock: &'static RwLock<Option<Ext2Fs>>,
    lock_name: &str,
) -> spin::RwLockWriteGuard<'static, Option<Ext2Fs>> {
    let upgradeable = ext2_spin_wait(lock_name, || lock.try_upgradeable_read());
    ext2_spin_wait_upgrade(upgradeable, lock_name)
}

/// Phase 2 of the write spin: busy-spin `try_upgrade()` starting from an
/// already-held upgradeable guard, until every existing reader has drained.
/// Factored out so the park path (`ext2_acquire_write`, below) can fall back
/// to spinning on an upgrade it already holds the slot for, without
/// releasing and re-racing for the upgradeable slot itself.
fn ext2_spin_wait_upgrade(
    mut upgradeable: spin::RwLockUpgradableGuard<'static, Option<Ext2Fs>>,
    lock_name: &str,
) -> spin::RwLockWriteGuard<'static, Option<Ext2Fs>> {
    let mut tracker = Ext2SpinStallTracker::new();
    loop {
        match upgradeable.try_upgrade() {
            Ok(write_guard) => return write_guard,
            Err(back) => upgradeable = back,
        }
        tracker.tick(lock_name);
        core::hint::spin_loop();
    }
}

// =============================================================================
// #728 — park instead of spin when it is safe to
// =============================================================================
//
// A contended acquisition parks on a per-lock WaitQueueHead instead of
// spinning whenever `ext2_lock_can_sleep()` is true — mirroring the
// established parking-on-contention precedent in this kernel
// (`drivers/virtio/block.rs`'s `BlockRequestGate`) and the syscall sleep
// path's own can-sleep check (`task/completion.rs`'s
// `syscall_sleep_path_available`), tightened to an exact `preempt_count()` of
// 1 so `schedule_current_wait()`'s unconditional enable-then-disable pairing
// (`task/waitqueue.rs`) can neither underflow nor leave preemption wedged on
// return. When it is not safe — no current thread, interrupt context,
// interrupts masked, more than one nested preempt-disable, or (aarch64) the
// timer not yet initialized — the accessor falls back to exactly the same
// spin `ext2_spin_wait`/`ext2_spin_wait_write` performed before this commit,
// which is always correct, just not livelock-proof. This is why the
// observer commit came first: the fallback spin path is unchanged code, and
// the same instrument measures both the pre-fix state (spin, always) and
// this fix's fallback edges (spin, sometimes).
//
// A parked acquisition is also bounded: `EXT2_LOCK_PARK_ROUNDS` rounds of
// `EXT2_LOCK_PARK_TIMEOUT_NS` each, using `prepare_to_wait_checked`'s
// enqueue-under-the-waitqueue-lock recheck (never the untimed
// `prepare_to_wait`) so a missed wake degrades to a bounded retry rather than
// a permanent hang, matching this kernel's own lost-wake history (#584,
// #586, #589). If every round leaves the lock still unavailable — which
// would mean the wake path itself is broken — the accessor falls back to the
// unchanged spin rather than looping forever on a wait that isn't working.
//
// Guards returned by the four accessors below are wrapped in
// `Ext2ReadGuard`/`Ext2WriteGuard`, which `Deref`/`DerefMut` transparently to
// `Option<Ext2Fs>` (every call site already only ever calls `.as_ref()`/
// `.as_mut()` on the guard, so no call site changes). Their `Drop` releases
// the inner `spin::RwLock` guard *first* and only then calls
// `wake_up()` — never holding ext2 state across the wake — so the lock order
// this file participates in is exactly `EXT2_STATE -> WAITQUEUE ->
// SCHEDULER`, matching `WaitQueueHead::wake_up_one`'s own
// `WAITQUEUE -> SCHEDULER` order (`task/waitqueue.rs`,
// `task/scheduler.rs`'s `with_scheduler`).
//
// Residual, disclosed rather than silently left: this fix closes the
// *contention* half of #728 (a contender no longer denies the CPU the actual
// holder's completion needs) but does not remove "ext2 guard held across a
// park" itself — `Completion::wait_timeout()`'s own documented precondition
// ("no locks are held") is still violated at every read/write-family call
// site enumerated in the #728 fix PR. Removing that pattern entirely
// (drop the guard before the block-device wait, re-validate on reacquire) is
// Option A in the #728 analysis, deliberately deferred to a follow-on: it
// touches the filesystem's core read/write/mutate paths and needs a real
// revalidation story for the in-memory allocator state
// (`Ext2Fs::block_groups`/`superblock`), which is real filesystem-correctness
// work, not a locking-primitive change.

/// Rounds of `EXT2_LOCK_PARK_TIMEOUT_NS` a parked acquisition will retry
/// before giving up and falling back to the (always-correct) spin path.
const EXT2_LOCK_PARK_ROUNDS: u32 = 32;

/// Per-round park timeout. Bounded per C6 of the #728 pre-check: a missed
/// wake degrades to a retry at this granularity rather than a permanent
/// hang. 32 rounds at 200ms is a 6.4s worst-case bound before falling back
/// to the spin path — generous next to ordinary block-device I/O latency,
/// small next to "forever."
const EXT2_LOCK_PARK_TIMEOUT_NS: u64 = 200_000_000;

/// Running count of acquisition attempts that actually parked (queued on a
/// `WaitQueueHead` and called `schedule_current_wait()`), as opposed to
/// resolving on the fast `try_*` path or falling back to the spin. Read via
/// `ext2_lock_parks()`.
///
/// Exists so a green race-leg run can prove the park path was *entered*,
/// not merely that no stall was observed — the absence of a stall is also
/// what a contender that parked and then never woke up looks like before
/// its round timeout elapses, so "no stall" alone does not distinguish
/// "the fix worked" from "the fix parked and got lucky on the timeout."
/// (B4 of the #728 review.)
static EXT2_LOCK_PARKS: AtomicU64 = AtomicU64::new(0);

/// Read the running count of successful park entries since boot. See
/// `EXT2_LOCK_PARKS`.
pub fn ext2_lock_parks() -> u64 {
    EXT2_LOCK_PARKS.load(Ordering::Relaxed)
}

/// Returns true when it is safe for a contended ext2 lock acquisition to
/// park instead of spin. See the module docs above for the full rationale;
/// this mirrors `block_request_gate_can_sleep()`
/// (`drivers/virtio/block.rs`) and `syscall_sleep_path_available()`
/// (`task/completion.rs`) — including the arch split those two precedents
/// already use: aarch64 gates on `interrupts_enabled()` (see below), x86
/// does not.
///
/// # Why x86 does *not* check `interrupts_enabled()`
///
/// Every x86 syscall enters through `INT 0x80` on an interrupt gate with an
/// explicit `cli` (`syscall/entry.asm:29`) and there is no `sti` anywhere on
/// the path down into this file — RFLAGS.IF is unconditionally 0 for the
/// entire duration of every x86 syscall, including the ~44 fs/syscall sites
/// that call the four accessors below. An `interrupts_enabled()` conjunct
/// here would make this predicate false at every one of them, permanently,
/// which reduces the fix to a no-op on the arch #728 was reported on. IF=0
/// is not a no-park condition on x86 the way DAIR.I-masked is on aarch64
/// (C3): the x86 park primitive `schedule_current_wait()` calls on every
/// loop iteration is `arch_halt_with_interrupts()` ==
/// `X86Cpu::halt_with_interrupts()` == `enable_and_hlt` — the atomic
/// `sti; hlt` sequence — so parking *from* IF=0 is exactly the normal,
/// expected entry state; the primitive itself is what turns interrupts back
/// on, at the halt, so the timer ISR can wake the thread. Every block-device
/// read already parks this same way, from this same IF=0 syscall state, via
/// `block_request_gate_can_sleep()` (x86: `preempt_count() > 0`, no IF
/// check) and `syscall_sleep_path_available()` (x86: `preempt_count() > 0`,
/// no IF check) — this predicate now matches both.
///
/// aarch64 syscalls are the opposite: `syscall_entry.S` unmasks DAIF before
/// `rust_syscall_handler` runs, so IRQs are normally *unmasked* in an
/// aarch64 syscall, and the one aarch64 site that masks them anyway
/// (`load_test_binaries_from_ext2`, C3) needs exactly this check to stay a
/// hard no-park. That is why the check is aarch64-only rather than deleted
/// outright: on aarch64 it is load-bearing; on x86 it is never anything but
/// unconditionally false, so keeping it there defeats the fix.
#[inline]
fn ext2_lock_can_sleep() -> bool {
    if crate::task::scheduler::current_thread_id().is_none() {
        return false;
    }

    #[cfg(target_arch = "aarch64")]
    {
        if crate::per_cpu_aarch64::in_interrupt() {
            return false;
        }
        if !crate::arch_impl::aarch64::cpu::interrupts_enabled() {
            return false;
        }
        if crate::per_cpu_aarch64::preempt_count() != 1 {
            return false;
        }
        crate::arch_impl::aarch64::timer_interrupt::is_initialized()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        if crate::per_cpu::in_interrupt() {
            return false;
        }
        crate::per_cpu::preempt_count() == 1
    }
}

fn ext2_schedule_current_wait() {
    crate::task::waitqueue::schedule_current_wait();
}

/// Try the fast (uncontended) path, then park up to `EXT2_LOCK_PARK_ROUNDS`
/// times when it's safe to, then fall back to the unchanged spin. Shared by
/// the read and write acquisition paths below via the closures they pass.
fn ext2_acquire<T>(
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
    mut try_acquire: impl FnMut() -> Option<T>,
    spin_fallback: impl FnOnce() -> T,
) -> T {
    if let Some(v) = try_acquire() {
        return v;
    }

    if ext2_lock_can_sleep() {
        for _ in 0..EXT2_LOCK_PARK_ROUNDS {
            if let Some(v) = try_acquire() {
                return v;
            }

            let deadline = ext2_now_ns() + EXT2_LOCK_PARK_TIMEOUT_NS;
            // The recheck closure runs under the waitqueue lock, atomically
            // with publishing our BlockedOnIO state: a release that lands
            // between our failed `try_acquire()` above and this enqueue is
            // not lost, because either the closure now observes the lock
            // free (Mismatch — loop back and retry `try_acquire()`
            // immediately, uncontended) or it still observes it held, in
            // which case we are safely enqueued before the holder can
            // possibly release again. The closure's own successful
            // `try_acquire()` result (if any) is deliberately dropped here —
            // `prepare_to_wait_checked`'s `cond` can only report a bool, not
            // hand a value back — and reacquired uncontended by the very
            // next loop iteration's `try_acquire()` above.
            let outcome = waiters.prepare_to_wait_checked(
                crate::task::thread::ThreadState::BlockedOnIO,
                Some(deadline),
                || try_acquire().is_none(),
            );

            match outcome {
                crate::task::waitqueue::PrepareOutcome::Mismatch => continue,
                crate::task::waitqueue::PrepareOutcome::PublishFailed => break,
                crate::task::waitqueue::PrepareOutcome::Queued => {
                    EXT2_LOCK_PARKS.fetch_add(1, Ordering::Relaxed);
                    ext2_schedule_current_wait();
                    waiters.finish_wait();
                }
            }
        }
    }

    spin_fallback()
}

/// Write-acquisition variant of `ext2_acquire`: acquires the upgradeable
/// slot via `ext2_acquire` (park-capable, generic), then parks waiting for
/// the upgrade itself while *continuing to hold* the upgradeable guard
/// across every retry round — never releasing and re-racing for the
/// upgradeable slot between rounds. This is what preserves the writer
/// fairness `root_fs_write()`'s doc comment promises (the UPGRADED bit keeps
/// blocking new readers for the whole wait, not just between our park
/// rounds) — C8 of the #728 pre-check.
fn ext2_acquire_write(
    lock: &'static RwLock<Option<Ext2Fs>>,
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
    lock_name: &str,
) -> spin::RwLockWriteGuard<'static, Option<Ext2Fs>> {
    let mut upgradeable = ext2_acquire(
        waiters,
        || lock.try_upgradeable_read(),
        || ext2_spin_wait(lock_name, || lock.try_upgradeable_read()),
    );

    // Fast path: no readers in the way at all.
    upgradeable = match upgradeable.try_upgrade() {
        Ok(w) => return w,
        Err(back) => back,
    };

    if !ext2_lock_can_sleep() {
        return ext2_spin_wait_upgrade(upgradeable, lock_name);
    }

    for _ in 0..EXT2_LOCK_PARK_ROUNDS {
        // `slot` lets the `FnOnce` recheck closure below hand the
        // upgradeable guard back out (its own signature can only return
        // `bool`) via a captured-by-reference `Option`, so we never drop it
        // and re-race for the upgradeable position between rounds.
        let mut slot = Some(upgradeable);
        let mut upgraded: Option<spin::RwLockWriteGuard<'static, Option<Ext2Fs>>> = None;
        let deadline = ext2_now_ns() + EXT2_LOCK_PARK_TIMEOUT_NS;
        let outcome = waiters.prepare_to_wait_checked(
            crate::task::thread::ThreadState::BlockedOnIO,
            Some(deadline),
            || {
                let held = slot.take().expect("slot populated at recheck");
                match held.try_upgrade() {
                    Ok(w) => {
                        upgraded = Some(w);
                        false // Mismatch: got it, don't enqueue.
                    }
                    Err(back) => {
                        slot = Some(back);
                        true // readers still present: enqueue and block.
                    }
                }
            },
        );

        if let Some(w) = upgraded {
            return w;
        }
        upgradeable = slot.take().expect("slot repopulated by cond on Mismatch/Queued");

        match outcome {
            crate::task::waitqueue::PrepareOutcome::Mismatch => continue,
            crate::task::waitqueue::PrepareOutcome::PublishFailed => {
                return ext2_spin_wait_upgrade(upgradeable, lock_name);
            }
            crate::task::waitqueue::PrepareOutcome::Queued => {
                EXT2_LOCK_PARKS.fetch_add(1, Ordering::Relaxed);
                ext2_schedule_current_wait();
                waiters.finish_wait();
            }
        }
    }

    ext2_spin_wait_upgrade(upgradeable, lock_name)
}

/// Wraps a `spin::RwLockReadGuard` on `ROOT_EXT2`/`HOME_EXT2`. Transparent
/// `Deref` to `Option<Ext2Fs>` — every call site only ever calls `.as_ref()`
/// on the guard it gets back, so this needs no call-site changes. Releases
/// the inner guard before waking parked contenders (see module docs above).
pub struct Ext2ReadGuard {
    inner: Option<spin::RwLockReadGuard<'static, Option<Ext2Fs>>>,
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
}

impl core::ops::Deref for Ext2ReadGuard {
    type Target = Option<Ext2Fs>;
    fn deref(&self) -> &Option<Ext2Fs> {
        self.inner.as_ref().expect("Ext2ReadGuard used after drop")
    }
}

impl Drop for Ext2ReadGuard {
    fn drop(&mut self) {
        self.inner = None;
        // A dropped *read* guard only ever unblocks the single upgradeable
        // holder waiting on `try_upgrade()` (releasing a reader never
        // admits another reader -- spin's RwLock never blocks reader vs.
        // reader), so at most one waiter can actually make progress from
        // this event. wake_up_one() avoids taking the waitqueue mutex and
        // waking every queued waiter (thundering herd) on every ordinary,
        // uncontended read-guard drop; has_waiters() skips the lock
        // entirely in the common uncontended case. If the queue is shared
        // with waiters this release doesn't concern, they still get woken
        // by their own bounded per-round timeout (C6) -- this is a wake-
        // efficiency choice, not a correctness one (review finding m1).
        if self.waiters.has_waiters() {
            self.waiters.wake_up_one();
        }
    }
}

/// Wraps a `spin::RwLockWriteGuard` on `ROOT_EXT2`/`HOME_EXT2`. Transparent
/// `Deref`/`DerefMut` to `Option<Ext2Fs>` — every call site only ever calls
/// `.as_mut()` on the guard it gets back, so this needs no call-site
/// changes. Releases the inner guard before waking parked contenders (see
/// module docs above).
pub struct Ext2WriteGuard {
    inner: Option<spin::RwLockWriteGuard<'static, Option<Ext2Fs>>>,
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
}

impl core::ops::Deref for Ext2WriteGuard {
    type Target = Option<Ext2Fs>;
    fn deref(&self) -> &Option<Ext2Fs> {
        self.inner.as_ref().expect("Ext2WriteGuard used after drop")
    }
}

impl core::ops::DerefMut for Ext2WriteGuard {
    fn deref_mut(&mut self) -> &mut Option<Ext2Fs> {
        self.inner.as_mut().expect("Ext2WriteGuard used after drop")
    }
}

impl Drop for Ext2WriteGuard {
    fn drop(&mut self) {
        self.inner = None;
        // Unlike a read-guard drop, releasing the write/upgradeable slot
        // can unblock several distinct waiter kinds at once (every queued
        // try_read(), plus the next try_upgradeable_read()), so this side
        // keeps the full broadcast wake_up() -- only the has_waiters()
        // fast path is worth adding, to skip the waitqueue mutex entirely
        // on the common uncontended drop.
        if self.waiters.has_waiters() {
            self.waiters.wake_up();
        }
    }
}

/// Wait queue for contended `ROOT_EXT2` acquisitions (both read and write —
/// a shared queue is deliberate: every release wakes every waiter, which is
/// always correct for an RwLock, even if occasionally spurious for a waiter
/// whose specific condition a given release didn't satisfy, and the bounded
/// per-round timeout is the backstop regardless).
static ROOT_EXT2_WAITERS: crate::task::waitqueue::WaitQueueHead =
    crate::task::waitqueue::WaitQueueHead::new();

/// Wait queue for contended `HOME_EXT2` acquisitions. See `ROOT_EXT2_WAITERS`.
static HOME_EXT2_WAITERS: crate::task::waitqueue::WaitQueueHead =
    crate::task::waitqueue::WaitQueueHead::new();

/// Global mounted ext2 root filesystem
///
/// Uses RwLock to allow concurrent read access (exec, file reads, getdents)
/// while exclusive write access is needed only for mutations (create, truncate,
/// rename, link, unlink, write). This prevents spinlock contention under slow
/// I/O where a writer holding the lock blocks all readers.
static ROOT_EXT2: RwLock<Option<Ext2Fs>> = RwLock::new(None);

/// Initialize the root ext2 filesystem
///
/// Mounts the ext2 disk as the root filesystem.
/// Device layout:
///   - x86_64: Device 0 UEFI boot disk, device 1 test binaries disk, device 2 ext2 disk
///   - ARM64 (QEMU): Device 0 ext2 disk (VirtIO MMIO)
///   - ARM64 (Parallels): AHCI SATA port 0
///
/// This should be called during kernel initialization after block
/// device driver initialization.
pub fn init_root_fs() -> Result<(), &'static str> {
    // Try VirtIO block devices first (works on both x86_64 and QEMU ARM64)
    let device: alloc::boxed::Box<dyn BlockDevice> = {
        use crate::block::virtio::VirtioBlockWrapper;
        if let Some(dev) = VirtioBlockWrapper::new(2).or_else(|| VirtioBlockWrapper::new(0)) {
            #[cfg(target_arch = "aarch64")]
            crate::serial_println!(
                "[ext2] Using VirtIO block device ({} sectors)",
                dev.num_blocks()
            );
            alloc::boxed::Box::new(dev)
        } else {
            // Fall back to AHCI block devices (Parallels ARM64).
            // Try each SATA device looking for one with a valid ext2 superblock.
            // On Parallels, sata:0 is typically the FAT32 EFI boot disk.
            #[cfg(target_arch = "aarch64")]
            {
                crate::serial_println!("[ext2] No VirtIO block device, trying AHCI...");
                let count = crate::drivers::ahci::sata_device_count();
                let mut found: Option<crate::drivers::ahci::AhciBlockDevice> = None;
                for i in 0..count {
                    if let Some(dev) = crate::drivers::ahci::get_block_device_by_index(i) {
                        crate::serial_println!(
                            "[ext2] AHCI device {}: {} sectors ({} MB)",
                            i,
                            dev.num_blocks(),
                            dev.num_blocks() * 512 / (1024 * 1024)
                        );
                        // Try reading ext2 superblock (at byte offset 1024, sector 2)
                        let mut buf = [0u8; 512];
                        if dev.read_block(2, &mut buf).is_ok() {
                            let magic = (buf[56] as u16) | ((buf[57] as u16) << 8);
                            if magic == 0xEF53 {
                                crate::serial_println!(
                                    "[ext2] Found ext2 superblock on AHCI device {}",
                                    i
                                );
                                found = Some(dev);
                                break;
                            } else {
                                crate::serial_println!(
                                    "[ext2] AHCI device {}: not ext2 (magic={:#06x})",
                                    i,
                                    magic
                                );
                            }
                        } else {
                            crate::serial_println!("[ext2] AHCI device {}: read failed", i);
                        }
                    }
                }
                let ahci_dev = found.ok_or(
                    "No block device with ext2 filesystem (tried VirtIO and all AHCI devices)",
                )?;
                alloc::boxed::Box::new(ahci_dev)
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                return Err("No ext2 block device available (expected at device index 2 or 0)");
            }
        }
    };

    // Register with VFS mount system
    let mount_id = crate::fs::vfs::mount("/", "ext2");

    // Create the ext2 filesystem instance
    let fs = Ext2Fs::new(device, mount_id)?;

    // Read packed struct fields safely before logging
    let blocks_count =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(fs.superblock.s_blocks_count)) };
    let inodes_count =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(fs.superblock.s_inodes_count)) };
    log::info!(
        "ext2: Mounted root filesystem - {} blocks, {} inodes, block size {}",
        blocks_count,
        inodes_count,
        fs.superblock.block_size()
    );

    // Store globally
    *ROOT_EXT2.write() = Some(fs);

    Ok(())
}

/// Access the root ext2 filesystem for read-only operations
///
/// Multiple readers can hold this lock concurrently, allowing parallel
/// exec, file reads, getdents, and stat operations without contention.
pub fn root_fs_read() -> Ext2ReadGuard {
    let inner = ext2_acquire(
        &ROOT_EXT2_WAITERS,
        || ROOT_EXT2.try_read(),
        || ext2_spin_wait("ROOT_EXT2_read", || ROOT_EXT2.try_read()),
    );
    Ext2ReadGuard {
        inner: Some(inner),
        waiters: &ROOT_EXT2_WAITERS,
    }
}

/// Access the root ext2 filesystem for write operations
///
/// Exclusive access — blocks all readers and other writers.
/// Use only for operations that modify filesystem state: create, truncate,
/// rename, link, unlink, mkdir, rmdir, write.
///
/// Uses upgradeable_read() + upgrade() to prevent writer starvation.
/// spin::RwLock is reader-preferring: write() spins until all readers release,
/// but new readers can keep arriving indefinitely. The upgradeable guard sets
/// the UPGRADED bit, which causes try_read() to reject new readers. The writer
/// then only waits for existing readers to drain, guaranteeing forward progress.
pub fn root_fs_write() -> Ext2WriteGuard {
    let inner = ext2_acquire_write(&ROOT_EXT2, &ROOT_EXT2_WAITERS, "ROOT_EXT2_write");
    Ext2WriteGuard {
        inner: Some(inner),
        waiters: &ROOT_EXT2_WAITERS,
    }
}

/// Check if the root filesystem is mounted
///
/// Routed through `root_fs_read()` (M1 of the #728 fix round), not a plain
/// `ROOT_EXT2.read()`: `spin`'s `try_read()` rejects new readers while
/// UPGRADED is set, so a plain blocking `.read()` here spins
/// non-yieldingly with `preempt_count() == 1` whenever a writer holds the
/// upgradeable slot — the #728 shape, invisible to the gate because it
/// bypasses `ext2_spin_wait` entirely. This is called on syscall-hot paths
/// (`sys_write`/`sys_read`/`sys_fstat`/etc. via `home_mount_id()` below, and
/// this function itself from `ext2_lock_race`), so it needs the same
/// park-then-spin-fallback discipline the four accessors get, not a
/// bespoke one — reusing the guard means it also gets the observability and
/// the release-before-wake ordering for free.
pub fn is_mounted() -> bool {
    root_fs_read().is_some()
}

/// Global mounted ext2 home filesystem (/home)
///
/// Separate from ROOT_EXT2 so that user data lives on a different disk
/// from the system binaries. This allows syncing the system disk to
/// other machines without overwriting their local user data.
static HOME_EXT2: RwLock<Option<Ext2Fs>> = RwLock::new(None);

/// Initialize the home ext2 filesystem
///
/// Mounts a second ext2 disk as the /home filesystem.
/// Device layout:
///   - x86_64: Device 3 (boot=0, test=1, root=2, home=3)
///   - ARM64: Device 1 (root=0, home=1)
///
/// This is non-fatal — if no home disk is attached, /home falls through
/// to the root ext2 filesystem (backward compatible).
pub fn init_home_fs() -> Result<(), &'static str> {
    // Try x86_64 layout first (device index 3), then ARM64 layout (device index 1).
    use crate::block::virtio::VirtioBlockWrapper;
    let device: alloc::boxed::Box<dyn BlockDevice> = {
        let dev = VirtioBlockWrapper::new(3)
            .or_else(|| VirtioBlockWrapper::new(1))
            .ok_or("No home block device available (expected at device index 3 or 1)")?;
        alloc::boxed::Box::new(dev)
    };

    // Register with VFS mount system
    let mount_id = crate::fs::vfs::mount("/home", "ext2");

    // Create the ext2 filesystem instance
    let fs = Ext2Fs::new(device, mount_id)?;

    // Read packed struct fields safely before logging
    let blocks_count =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(fs.superblock.s_blocks_count)) };
    log::info!(
        "ext2: Mounted home filesystem at /home - {} blocks, block size {}",
        blocks_count,
        fs.superblock.block_size()
    );

    // Store globally
    *HOME_EXT2.write() = Some(fs);

    Ok(())
}

/// Access the home ext2 filesystem for read-only operations
pub fn home_fs_read() -> Ext2ReadGuard {
    let inner = ext2_acquire(
        &HOME_EXT2_WAITERS,
        || HOME_EXT2.try_read(),
        || ext2_spin_wait("HOME_EXT2_read", || HOME_EXT2.try_read()),
    );
    Ext2ReadGuard {
        inner: Some(inner),
        waiters: &HOME_EXT2_WAITERS,
    }
}

/// Access the home ext2 filesystem for write operations
///
/// Uses upgradeable_read() + upgrade() to prevent writer starvation (same as root_fs_write).
pub fn home_fs_write() -> Ext2WriteGuard {
    let inner = ext2_acquire_write(&HOME_EXT2, &HOME_EXT2_WAITERS, "HOME_EXT2_write");
    Ext2WriteGuard {
        inner: Some(inner),
        waiters: &HOME_EXT2_WAITERS,
    }
}

/// Check if the home filesystem is mounted
///
/// Routed through `home_fs_read()` — see `is_mounted()`'s doc comment (M1
/// of the #728 fix round); the same blocking-spin hazard applies here.
pub fn is_home_mounted() -> bool {
    home_fs_read().is_some()
}

/// Get the mount_id of the home filesystem, if mounted.
///
/// Used by FD-based syscall dispatch to determine which filesystem
/// a file descriptor belongs to. Routed through `home_fs_read()` — see
/// `is_mounted()`'s doc comment (M1 of the #728 fix round). This is the
/// highest-traffic of the three M1 sites: called from `sys_write`,
/// `sys_read`, `sys_pread64`, `sys_pwrite64`, `sys_fstat`,
/// `sys_getdents64` and `sys_utimensat`, immediately ahead of the very
/// accessor calls this fix repaired.
pub fn home_mount_id() -> Option<usize> {
    home_fs_read().as_ref().map(|fs| fs.mount_id)
}

/// Strip the /home prefix from a path, returning the path within the home filesystem.
///
/// "/home/foo.bmp" → "/foo.bmp"
/// "/home" → "/"
/// "/home/" → "/"
pub fn strip_home_prefix(path: &str) -> &str {
    if path == "/home" || path == "/home/" {
        "/"
    } else if path.starts_with("/home/") {
        // rest is "foo.bmp" — we need to return "/foo.bmp"
        // Since we can't allocate, we return the slice starting at the '/' before the rest
        // path = "/home/foo.bmp"
        //         01234567...
        // We want &path[5..] = "/foo.bmp"
        &path[5..]
    } else {
        path
    }
}

/// Check if a resolved path should be routed to the home filesystem.
pub fn is_home_path(path: &str) -> bool {
    (path == "/home" || path.starts_with("/home/")) && is_home_mounted()
}
