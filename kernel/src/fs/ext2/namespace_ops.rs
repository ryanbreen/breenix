//! Directory lookup, enumeration and namespace mutations shared with the host harness.
use super::{
    create_node::dir_entry_type, dir, directory_io, file::read_file, add_directory_entry,
    is_directory_empty, remove_entry, Ext2Fs, Ext2Inode, EXT2_FT_DIR, EXT2_LINK_MAX, EXT2_ROOT_INO,
};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

impl Ext2Fs {
    /// Look up a path component in a directory
    ///
    /// Returns the matching inode number when an entry exists.
    pub fn lookup_in_dir(
        &self,
        dir_inode: &Ext2Inode,
        name: impl AsRef<[u8]>,
    ) -> Result<Option<u32>, &'static str> {
        self.lookup_in_dir_bytes(dir_inode, name.as_ref())
    }
    pub fn lookup_in_dir_bytes(&self, dir_inode: &Ext2Inode, name: &[u8]) -> Result<Option<u32>, &'static str> {
        let dir_data = self.read_directory(dir_inode)?;
        Ok(dir::find_inode_bytes(&dir_data, name))
    }

    /// Read directory entries from an inode
    ///
    /// Returns the raw directory data for parsing with DirReader.
    pub fn read_directory(&self, inode: &Ext2Inode) -> Result<Vec<u8>, &'static str> {
        if !inode.is_dir() {
            return Err("Not a directory");
        }
        read_file(self.device.as_ref(), inode, &self.superblock).map_err(|e| {
            if e == crate::block::BlockError::OutOfMemory {
                "No memory for directory data"
            } else {
                "Failed to read directory data"
            }
        })
    }

    pub fn unlink_in_dir(
        &mut self,
        parent_inode_num: u32,
        filename: impl AsRef<[u8]>,
    ) -> Result<(), &'static str> {
        let filename = filename.as_ref();
        self.ensure_writable()?;
        let parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Read the parent directory data
        let dir_data = self.read_directory(&parent_inode)?;

        // Find the entry to verify it exists and get its inode
        let target_inode_num = dir::find_inode_bytes(&dir_data, filename).ok_or("File not found")?;

        // Check that we're not unlinking a directory (use rmdir for that)
        let target_inode = self.read_inode(target_inode_num)?;
        if target_inode.is_dir() {
            return Err("Cannot unlink directory (use rmdir)");
        }

        // Pin before dropping the directory entry; an open/mapped orphan
        // keeps its allocation and incarnation until its last observer leaves.
        let handle = self.pin_loaded_inode(target_inode_num, target_inode.size())?;

        // The parent's records with the name removed, and its mtime and ctime.
        let mut image = directory_io::copy(&dir_data)?;
        remove_entry(&mut image, filename, self.superblock.block_size())?;
        let mut parent_after = parent_inode;
        parent_after.update_timestamps(false, true, true);
        // The target loses a link, and its ctime changes.
        let mut target_after = target_inode;
        target_after.i_links_count = target_inode
            .i_links_count
            .checked_sub(1)
            .ok_or("Inode already unlinked")?;
        target_after.update_timestamps(false, false, true);
        let links = target_after.i_links_count;

        // The name goes first; the count it held falls after it is gone.
        let mut writes = Vec::new();
        writes
            .try_reserve_exact(3)
            .map_err(|_| "No memory for directory write plan")?;
        self.plan_blocks(&parent_inode, &dir_data, &image, 0, &mut writes)?;
        writes
            .try_reserve(2)
            .map_err(|_| "No memory for directory write plan")?;
        writes.push(directory_io::Write::Inode {
            ino: parent_inode_num,
            old: parent_inode,
            new: parent_after,
        });
        writes.push(directory_io::Write::Inode {
            ino: target_inode_num,
            old: target_inode,
            new: target_after,
        });

        // Commit the plan before marking the detached inode as an orphan.
        self.commit_writes(&writes)
            .map_err(|_| "Failed to write directory entry removal")?;
        if links == 0 {
            handle.object.orphan.store(true, Ordering::Release);
            // The last observer queues reclamation; writers settle pending
            // allocations under space pressure before trying new allocation.
        }
        drop(handle);

        log::debug!("ext2: unlinked {:?} (inode {})", filename, target_inode_num);
        Ok(())
    }

    pub fn create_directory_in(
        &mut self,
        parent_inode_num: u32,
        dirname: impl AsRef<[u8]>,
        mode: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u32, &'static str> {
        let dirname = dirname.as_ref();
        self.ensure_writable()?;
        // Validate name
        if dirname.is_empty() || dirname.len() > 255 {
            return Err("Invalid directory name length");
        }
        if dirname.contains(&b'/') || dirname == b"." || dirname == b".." {
            return Err("Invalid directory name");
        }

        let mut parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }
        // The new directory's `..` is another link to the parent.
        if parent_inode.i_links_count >= EXT2_LINK_MAX {
            return Err("Too many links");
        }

        // Read the parent directory data
        let mut parent_dir_data = self.read_directory(&parent_inode)?;

        // Check if the directory already exists
        if dir::find_inode_bytes(&parent_dir_data, dirname).is_some() {
            return Err("Directory already exists");
        }
        self.make_directory_room(parent_inode_num, &mut parent_inode, &mut parent_dir_data, dirname.len())?;

        // The new directory holds one block, for its "." and ".." entries.
        let block_size = self.superblock.block_size();
        let mut new_inode = Ext2Inode::new_directory(mode);
        new_inode.set_owner(uid, gid);
        new_inode.i_size = block_size as u32;
        let entries = |new_inode_num: u32, dir_data: &mut [u8]| {
            // "." points to the directory itself:
            // inode (4) + rec_len (2) + name_len (1) + file_type (1) + name (1) = 9, aligned to 12
            let dot_rec_len = 12u16;
            dir_data[0..4].copy_from_slice(&new_inode_num.to_le_bytes()); // inode
            dir_data[4..6].copy_from_slice(&dot_rec_len.to_le_bytes()); // rec_len
            dir_data[6] = 1; // name_len
            dir_data[7] = EXT2_FT_DIR; // file_type
            dir_data[8] = b'.'; // name

            // ".." points to the parent and takes up the rest of the block
            let dotdot_offset = 12usize;
            let dotdot_rec_len = (block_size - 12) as u16;
            dir_data[dotdot_offset..dotdot_offset + 4].copy_from_slice(&parent_inode_num.to_le_bytes()); // inode
            dir_data[dotdot_offset + 4..dotdot_offset + 6]
                .copy_from_slice(&dotdot_rec_len.to_le_bytes()); // rec_len
            dir_data[dotdot_offset + 6] = 2; // name_len
            dir_data[dotdot_offset + 7] = EXT2_FT_DIR; // file_type
            dir_data[dotdot_offset + 8] = b'.'; // name[0]
            dir_data[dotdot_offset + 9] = b'.'; // name[1]
        };
        // The parent gains the link the new ".." gives it, and the group
        // counts the new directory.
        let new_inode_num = self.create_entry(
            parent_inode_num,
            &parent_inode,
            &parent_dir_data,
            dirname,
            new_inode,
            Some(&entries),
            |_, ino| Ok(ino),
        )?;

        log::debug!(
            "ext2: created directory '{:?}' with inode {}",
            dirname,
            new_inode_num
        );
        Ok(new_inode_num)
    }

    pub fn remove_directory_in(
        &mut self,
        parent_inode_num: u32,
        dir_name: impl AsRef<[u8]>,
    ) -> Result<(), &'static str> {
        let dir_name = dir_name.as_ref();
        self.ensure_writable()?;
        let parent_inode = self.read_inode(parent_inode_num)?;
        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }
        let target_inode_num = self
            .lookup_in_dir(&parent_inode, dir_name)?
            .ok_or("Directory not found")?;
        if target_inode_num == EXT2_ROOT_INO || self.namespace.is_mount(target_inode_num)
            || self.namespace.mount_entry(parent_inode_num, dir_name) {
            return Err("Cannot remove root directory");
        }
        let target_inode = self.read_inode(target_inode_num)?;

        // Verify it's a directory
        if !target_inode.is_dir() {
            return Err("Not a directory");
        }

        // A held directory (a working directory) keeps its inode and blocks.
        if self
            .live_inodes
            .get(target_inode_num)
            .is_some_and(|object| !object.unused())
        {
            return Err("Directory is busy");
        }

        // Read directory contents and check if empty
        let dir_data = self.read_directory(&target_inode)?;
        if !is_directory_empty(&dir_data) {
            return Err("Directory not empty");
        }

        // The parent's entries with the name removed
        let parent_dir_data = self.read_directory(&parent_inode)?;
        let mut parent_image = Vec::new();
        parent_image
            .try_reserve_exact(parent_dir_data.len())
            .map_err(|_| "No memory for directory removal")?;
        parent_image.extend_from_slice(&parent_dir_data);
        remove_entry(&mut parent_image, dir_name, self.superblock.block_size())?;

        // The parent loses the link the directory's ".." gave it, and its
        // mtime and ctime change.
        let mut parent_after = parent_inode;
        parent_after.update_timestamps(false, true, true);
        parent_after.i_links_count = parent_inode
            .i_links_count
            .checked_sub(1)
            .ok_or("Corrupt parent link count")?;

        // The directory loses its name and its own ".": no link remains.
        let mut target_after = target_inode;
        target_after.i_links_count = 0;
        target_after.update_timestamps(false, false, true);

        // Held from before the disk changes, so the zero-link inode can be
        // handed to the finalizer afterwards without allocating.
        let handle = self.pin_loaded_inode(target_inode_num, target_inode.size())?;

        // The name goes first; the counts it held fall after it is gone.
        let mut writes = Vec::new();
        writes
            .try_reserve_exact(parent_dir_data.len().div_ceil(self.superblock.block_size()) + 2)
            .map_err(|_| "No memory for directory removal plan")?;
        self.plan_blocks(
            &parent_inode,
            &parent_dir_data,
            &parent_image,
            0,
            &mut writes,
        )?;
        writes.push(directory_io::Write::Inode {
            ino: target_inode_num,
            old: target_inode,
            new: target_after,
        });
        writes.push(directory_io::Write::Inode {
            ino: parent_inode_num,
            old: parent_inode,
            new: parent_after,
        });

        // Commit the plan before handing the detached directory to the finalizer.
        self.commit_writes(&writes)
            .map_err(|_| "Failed to write directory removal")?;

        handle.object.orphan.store(true, Ordering::Release);
        match handle.release_sole() {
            Ok(object) => object.defer(),
            // Another observer holds it; its last close queues the orphan.
            Err(handle) => drop(handle),
        }

        log::debug!(
            "ext2: removed directory '{:?}' (inode {})",
            dir_name,
            target_inode_num
        );
        Ok(())
    }

    pub fn link_in_dir(
        &mut self,
        source_inode_num: u32,
        new_parent_inode_num: u32,
        new_filename: impl AsRef<[u8]>,
    ) -> Result<(), &'static str> {
        let new_filename = new_filename.as_ref();
        self.ensure_writable()?;
        let source_inode = self.read_inode(source_inode_num)?;
        if source_inode.i_links_count == 0 {
            return Err("Source not found");
        }
        if source_inode.is_dir() {
            return Err("Cannot create hard link to directory");
        }
        if source_inode.i_links_count >= EXT2_LINK_MAX {
            return Err("Too many links");
        }
        if new_filename.is_empty()
            || new_filename.len() > 255
            || new_filename.contains(&b'/')
            || new_filename == b"."
            || new_filename == b".."
        {
            return Err("Invalid filename");
        }
        let mut new_parent_inode = self.read_inode(new_parent_inode_num)?;
        if !new_parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }
        if new_parent_inode.i_links_count == 0 {
            return Err("Parent not found");
        }
        // Read the parent directory data
        let mut dir_data = self.read_directory(&new_parent_inode)?;

        if dir::find_inode_bytes(&dir_data, new_filename).is_some() {
            return Err("Destination already exists");
        }
        self.make_directory_room(new_parent_inode_num, &mut new_parent_inode, &mut dir_data, new_filename.len())?;

        // The parent's records with the new name, and its mtime and ctime.
        let mut image = directory_io::copy(&dir_data)?;
        let offset = add_directory_entry(
            &mut image,
            source_inode_num,
            new_filename,
            dir_entry_type(&source_inode),
        )?;
        let mut parent_after = new_parent_inode;
        parent_after.update_timestamps(false, true, true);
        // The source gains a link, and its ctime changes.
        let mut source_after = source_inode;
        source_after.i_links_count += 1;
        source_after.update_timestamps(false, false, true);

        // Publish the raised link count before the new name, leaving crash
        // repair to lower an excess count.
        let mut writes = Vec::new();
        writes
            .try_reserve_exact(3)
            .map_err(|_| "No memory for directory write plan")?;
        writes.push(directory_io::Write::Inode {
            ino: source_inode_num,
            old: source_inode,
            new: source_after,
        });
        self.plan_blocks(&new_parent_inode, &dir_data, &image, offset, &mut writes)?;
        writes
            .try_reserve(1)
            .map_err(|_| "No memory for directory write plan")?;
        writes.push(directory_io::Write::Inode {
            ino: new_parent_inode_num,
            old: new_parent_inode,
            new: parent_after,
        });

        // Publish the prepared directory and inode writes.
        self.commit_writes(&writes)
            .map_err(|_| "Failed to write new directory entry")?;

        log::debug!(
            "ext2: created hard link {:?} (inode {})",
            new_filename,
            source_inode_num
        );
        Ok(())
    }

    pub fn create_symlink_in(
        &mut self,
        target: impl AsRef<[u8]>,
        parent_inode_num: u32,
        link_name: impl AsRef<[u8]>,
        uid: u32,
        gid: u32,
    ) -> Result<(), &'static str> {
        self.create_symlink_node_in(target.as_ref(), parent_inode_num, link_name.as_ref(), 0o777, uid, gid)
            .map(|_| ())
    }

    /// Create a symlink with VFS-selected attributes in the namespace plan.
    pub fn create_symlink_node_in(
        &mut self,
        target: &[u8],
        parent_inode_num: u32,
        link_name: &[u8],
        mode: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u32, &'static str> {
        self.ensure_writable()?;
        // Validate the link name
        if link_name.is_empty() || link_name.len() > 255 {
            return Err("Invalid filename length");
        }
        if link_name.contains(&b'/') || link_name == b"." || link_name == b".." {
            return Err("Invalid filename");
        }

        // Verify target is not empty
        if target.is_empty() {
            return Err("Symlink target cannot be empty");
        }
        // A target and its NUL must fit the one block a symlink holds (Linux
        // ext2 refuses a longer one the same way).
        if target.len() >= self.superblock.block_size() {
            return Err("Symlink target too long");
        }

        let mut parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        if self.lookup_in_dir_bytes(&parent_inode, link_name)?.is_some() {
            return Err("File already exists");
        }

        // Read the parent directory before the first disk mutation, so a
        // failed read or allocation leaves no inode without an entry.
        let mut dir_data = self.read_directory(&parent_inode)?;
        self.make_directory_room(parent_inode_num, &mut parent_inode, &mut dir_data, link_name.len())?;

        // A target of more than 60 bytes is stored in a data block; a shorter
        // one in the inode itself (a fast symlink).
        let mut new_inode = Ext2Inode::new_symlink(target);
        new_inode.i_mode = super::EXT2_S_IFLNK | (mode & 0o7777);
        new_inode.set_owner(uid, gid);
        let target_block = |_: u32, block: &mut [u8]| {
            block[..target.len()].copy_from_slice(target);
        };
        let block: Option<&dyn Fn(u32, &mut [u8])> =
            if target.len() > 60 { Some(&target_block) } else { None };
        let ino = self.create_entry(
            parent_inode_num,
            &parent_inode,
            &dir_data,
            link_name,
            new_inode,
            block,
            |_, ino| Ok(ino),
        )?;

        log::debug!("ext2: created symlink '{:?}' -> '{:?}'", link_name, target);
        Ok(ino)
    }
}
