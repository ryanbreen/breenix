//! Path lookup shared by kernel consumers and the ext2 host tests.
use super::{dir, file::read_ext2_block, Ext2Fs, Ext2Inode};
use alloc::vec::Vec;

impl Ext2Fs {
    /// Look up a path component in a directory
    ///
    /// Returns the inode number of the matching entry, or None if not found.
    pub fn lookup_in_dir(
        &self,
        dir_inode: &Ext2Inode,
        name: &str,
    ) -> Result<Option<u32>, &'static str> {
        self.lookup_in_dir_bytes(dir_inode, name.as_bytes())
    }
    pub fn lookup_in_dir_bytes(&self, dir_inode: &Ext2Inode, name: &[u8]) -> Result<Option<u32>, &'static str> {
        let dir_data = self.read_directory(dir_inode)?;
        Ok(dir::find_inode_bytes(&dir_data, name))
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

        // Start at the selected namespace root (inode 2 on a flat partition).
        let mut current_inode_num = self.namespace.root;

        // Collect components so we can detect the final one
        let components: alloc::vec::Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        for (i, component) in components.iter().enumerate() {
            // Read the current directory inode
            let current_inode = self.read_inode(current_inode_num)?;

            // Make sure it's a directory
            if !current_inode.is_dir() {
                return Err("Not a directory in path");
            }

            if *component == "." { continue; }
            if *component == ".." && self.namespace.is_mount(current_inode_num) {
                current_inode_num = self.namespace.root;
                continue;
            }
            if self.namespace.hidden(current_inode_num, component.as_bytes()) {
                return Err("Path component not found");
            }

            // Look up the component in this directory
            let child = match self.namespace.shared(current_inode_num, component.as_bytes()) {
                Some(ino) => Some(ino),
                None => self.lookup_in_dir(&current_inode, component)?,
            };
            match child {
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

    /// Read the target of a symbolic link
    ///
    /// # Arguments
    /// * `inode_num` - The inode number of the symbolic link
    ///
    /// # Returns
    /// * `Ok(String)` - The target path the symlink points to
    /// * `Err(msg)` - Error if not a symlink or read error
    pub fn read_symlink(&self, inode_num: u32) -> Result<alloc::string::String, &'static str> {
        alloc::string::String::from_utf8(self.read_symlink_bytes(inode_num)?).map_err(|_| "Invalid UTF-8 in symlink target")
    }
    pub fn read_symlink_bytes(&self, inode_num: u32) -> Result<Vec<u8>, &'static str> {

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
            {
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(target_bytes.len())
                    .map_err(|_| "No memory for symlink target")?;
                bytes.extend_from_slice(target_bytes);
                Ok(bytes)
            }
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
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(target_bytes.len()).map_err(|_| "No memory for symlink target")?;
            bytes.extend_from_slice(target_bytes);
            Ok(bytes)
        }
    }
}
