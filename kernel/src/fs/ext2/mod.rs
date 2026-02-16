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
    fn resolve_path_impl(&self, path: &str, follow_final: bool, depth: u32) -> Result<u32, &'static str> {
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
                                if j > 0 { r.push('/'); }
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
                                if j > 0 { parent.push('/'); }
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
        if let Err(_) = write_file_range(self.device.as_ref(), &mut inode, &self.superblock, &mut self.block_groups, offset, data) {
            return Err("Failed to write file data");
        }

        // Write the modified inode back to disk
        if let Err(_) = inode.write_to(self.device.as_ref(), inode_num, &self.superblock, &self.block_groups) {
            return Err("Failed to write inode");
        }

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

        // Update parent directory timestamps (mtime and ctime)
        let mut parent_inode_mut = parent_inode;
        parent_inode_mut.update_timestamps(false, true, true);

        // Write the modified directory data back
        self.write_directory_data(parent_inode_num, &dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut.write_to(
            self.device.as_ref(),
            parent_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write parent inode")?;

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
                ).is_ok() {
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
        inode.write_to(
            self.device.as_ref(),
            inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write truncated inode")?;

        // Update superblock free block count so freed blocks can be reused
        if blocks_freed > 0 {
            self.superblock.increment_free_blocks(blocks_freed);
            self.superblock.write_to(self.device.as_ref())
                .map_err(|_| "Failed to write superblock after truncate")?;
        }

        log::debug!("ext2: truncated inode {} to zero length, freed {} blocks", inode_num, blocks_freed);
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
        let link_count = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(target_inode.i_links_count))
        };

        // Calculate how many blocks this file uses (if we're about to free it)
        // i_blocks is in 512-byte sectors, so divide by (block_size / 512)
        let blocks_to_free = if link_count == 1 {
            let block_size = self.superblock.block_size();
            let sectors_per_block = (block_size / 512) as u32;
            let i_blocks = unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!(target_inode.i_blocks))
            };
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
        parent_inode_mut.write_to(
            self.device.as_ref(),
            parent_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write parent inode")?;

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
            self.superblock.write_to(self.device.as_ref())
                .map_err(|_| "Failed to write superblock")?;

            // Write updated block group descriptors
            Ext2BlockGroupDesc::write_table(
                self.device.as_ref(),
                &self.superblock,
                &self.block_groups,
            ).map_err(|_| "Failed to write block group descriptors")?;
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
        if old_filename == "." || old_filename == ".." || new_filename == "." || new_filename == ".." {
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
        let source_file_type = if source_is_dir { EXT2_FT_DIR } else { EXT2_FT_REG_FILE };

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
            add_directory_entry(&mut old_parent_data, source_inode_num, new_filename, source_file_type)?;

            // Update parent directory timestamps
            let mut parent_inode_mut = old_parent_inode;
            parent_inode_mut.update_timestamps(false, true, true);

            // Write back once
            self.write_directory_data(old_parent_num, &old_parent_data)?;

            // Write the updated parent directory inode
            parent_inode_mut.write_to(
                self.device.as_ref(),
                old_parent_num,
                &self.superblock,
                &self.block_groups,
            ).map_err(|_| "Failed to write parent inode")?;
        } else {
            // Different directories
            add_directory_entry(&mut new_parent_data, source_inode_num, new_filename, source_file_type)?;

            // Update timestamps for both parent directories
            let mut old_parent_mut = old_parent_inode;
            let mut new_parent_mut = new_parent_inode;
            old_parent_mut.update_timestamps(false, true, true);
            new_parent_mut.update_timestamps(false, true, true);

            // Write both directories back
            self.write_directory_data(old_parent_num, &old_parent_data)?;
            self.write_directory_data(new_parent_num, &new_parent_data)?;

            // Write the updated parent directory inodes
            old_parent_mut.write_to(
                self.device.as_ref(),
                old_parent_num,
                &self.superblock,
                &self.block_groups,
            ).map_err(|_| "Failed to write old parent inode")?;

            new_parent_mut.write_to(
                self.device.as_ref(),
                new_parent_num,
                &self.superblock,
                &self.block_groups,
            ).map_err(|_| "Failed to write new parent inode")?;

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
        dir_data[dotdot_offset + 4..dotdot_offset + 6].copy_from_slice(&dotdot_rec_len.to_le_bytes()); // rec_len
        dir_data[dotdot_offset + 6] = 2; // name_len
        dir_data[dotdot_offset + 7] = EXT2_FT_DIR; // file_type
        dir_data[dotdot_offset + 8] = b'.'; // name[0]
        dir_data[dotdot_offset + 9] = b'.'; // name[1]

        // Write the directory data block
        file::write_ext2_block(self.device.as_ref(), new_block, block_size, &dir_data[..block_size])
            .map_err(|_| "Failed to write directory data block")?;

        // Write the new inode to disk
        new_inode.write_to(
            self.device.as_ref(),
            new_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write new directory inode")?;

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
        parent_inode_mut.write_to(
            self.device.as_ref(),
            parent_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write parent inode")?;

        // Update superblock with new free inode and block counts
        self.superblock.decrement_free_inodes();
        self.superblock.decrement_free_blocks();
        self.superblock.write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;

        // Update block group used directories count
        let inodes_per_group = self.superblock.s_inodes_per_group;
        let bg_index = ((new_inode_num - 1) / inodes_per_group) as usize;
        if bg_index < self.block_groups.len() {
            let bg = &mut self.block_groups[bg_index];
            let used_dirs = unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!(bg.bg_used_dirs_count))
            };
            unsafe {
                core::ptr::write_unaligned(
                    core::ptr::addr_of_mut!(bg.bg_used_dirs_count),
                    used_dirs + 1,
                );
            }
        }

        // Write updated block group descriptors
        Ext2BlockGroupDesc::write_table(
            self.device.as_ref(),
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write block group descriptors")?;

        log::debug!("ext2: created directory '{}' with inode {}", path, new_inode_num);
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
        parent_inode_mut.write_to(
            self.device.as_ref(),
            parent_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write parent inode")?;

        // Free the directory's data blocks
        let i_block = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(target_inode.i_block))
        };
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
        self.superblock.write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;

        // Write updated block group descriptors
        Ext2BlockGroupDesc::write_table(
            self.device.as_ref(),
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write block group descriptors")?;

        log::debug!("ext2: removed directory '{}' (inode {})", path, target_inode_num);
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
        add_directory_entry(&mut dir_data, source_inode_num, new_filename, EXT2_FT_REG_FILE)?;

        // Update parent directory timestamps
        let mut parent_inode_mut = new_parent_inode;
        parent_inode_mut.update_timestamps(false, true, true);

        // Write the modified directory data back
        self.write_directory_data(new_parent_inode_num, &dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut.write_to(
            self.device.as_ref(),
            new_parent_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write parent inode")?;

        // Increment the source inode's link count
        increment_inode_links(
            self.device.as_ref(),
            source_inode_num,
            &self.superblock,
            &self.block_groups,
        )?;

        log::debug!(
            "ext2: created hard link {} -> {} (inode {})",
            newpath, oldpath, source_inode_num
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

            write_ext2_block(self.device.as_ref(), block_num, block_size, &block_buf[..block_size])
                .map_err(|_| "Failed to write symlink target block")?;

            // Update inode to point to this block
            new_inode.i_block[0] = block_num;
            // i_blocks is in 512-byte sectors
            new_inode.i_blocks = (block_size / 512) as u32;
        }

        // Write the new inode to disk
        new_inode.write_to(
            self.device.as_ref(),
            new_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write symlink inode")?;

        // Add directory entry with EXT2_FT_SYMLINK type
        let mut dir_data = self.read_directory(&parent_inode)?;
        add_directory_entry(&mut dir_data, new_inode_num, link_name, EXT2_FT_SYMLINK)?;

        // Update parent directory timestamps
        let mut parent_inode_mut = parent_inode;
        parent_inode_mut.update_timestamps(false, true, true);

        // Write the modified directory data back
        self.write_directory_data(parent_inode_num, &dir_data)?;

        // Write the updated parent directory inode
        parent_inode_mut.write_to(
            self.device.as_ref(),
            parent_inode_num,
            &self.superblock,
            &self.block_groups,
        ).map_err(|_| "Failed to write parent inode")?;

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
        let target_len = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_size))
        } as usize;

        if target_len == 0 {
            return Err("Empty symlink target");
        }

        // Check if this is a fast symlink (target stored in i_block)
        // Fast symlinks have i_blocks == 0 (no data blocks allocated)
        let i_blocks = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_blocks))
        };

        if i_blocks == 0 && target_len <= 60 {
            // Fast symlink: target is stored in the i_block array
            let i_block = unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block))
            };

            // Convert the i_block array to bytes
            let block_bytes = unsafe {
                core::slice::from_raw_parts(
                    i_block.as_ptr() as *const u8,
                    60,
                )
            };

            // Extract the target string
            let target_bytes = &block_bytes[..target_len];
            String::from_utf8(target_bytes.to_vec())
                .map_err(|_| "Invalid UTF-8 in symlink target")
        } else {
            // Regular symlink: target is stored in a data block
            let i_block = unsafe {
                core::ptr::read_unaligned(core::ptr::addr_of!(inode.i_block))
            };

            let block_num = i_block[0];
            if block_num == 0 {
                return Err("Symlink has no data block");
            }

            // Read the data block
            // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
            let block_size = self.superblock.block_size();
            let mut block_buf = [0u8; 4096]; // Max block size
            read_ext2_block(self.device.as_ref(), block_num, block_size, &mut block_buf[..block_size])
                .map_err(|_| "Failed to read symlink data block")?;

            // Extract the target string
            let target_bytes = &block_buf[..target_len];
            String::from_utf8(target_bytes.to_vec())
                .map_err(|_| "Invalid UTF-8 in symlink target")
        }
    }

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
            // Use stack-based buffer to avoid heap allocation (bump allocator doesn't reclaim)
            let mut block_buf = [0u8; 4096]; // Max block size
            block_buf[..bytes_to_write].copy_from_slice(&data[offset..offset + bytes_to_write]);

            // Write the block using ext2-to-device block conversion
            file::write_ext2_block(self.device.as_ref(), block_num, block_size, &block_buf[..block_size])
                .map_err(|_| "Failed to write directory block")?;

            offset += bytes_to_write;
        }

        Ok(())
    }
}

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
///   - ARM64: Device 0 ext2 disk
///
/// This should be called during kernel initialization after VirtIO
/// block device initialization.
pub fn init_root_fs() -> Result<(), &'static str> {
    // Try x86_64 layout first (device index 2), then ARM64 layout (device index 0).
    let device = VirtioBlockWrapper::new(2)
        .or_else(|| VirtioBlockWrapper::new(0))
        .ok_or("No ext2 block device available (expected at device index 2 or 0)")?;
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
    *ROOT_EXT2.write() = Some(fs);

    Ok(())
}

/// Access the root ext2 filesystem for read-only operations
///
/// Multiple readers can hold this lock concurrently, allowing parallel
/// exec, file reads, getdents, and stat operations without contention.
pub fn root_fs_read() -> spin::RwLockReadGuard<'static, Option<Ext2Fs>> {
    ROOT_EXT2.read()
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
pub fn root_fs_write() -> spin::RwLockWriteGuard<'static, Option<Ext2Fs>> {
    ROOT_EXT2.upgradeable_read().upgrade()
}

/// Check if the root filesystem is mounted
pub fn is_mounted() -> bool {
    ROOT_EXT2.read().is_some()
}
