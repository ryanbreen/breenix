//! Ext2 namespace creation for regular files, FIFO, socket and device inodes.
use super::*;

/// The directory-entry file type recorded for an inode.
pub(super) fn dir_entry_type(inode: &Ext2Inode) -> u8 {
    match inode.file_type() {
        FileType::Regular => EXT2_FT_REG_FILE,
        FileType::Directory => EXT2_FT_DIR,
        FileType::CharDevice => EXT2_FT_CHRDEV,
        FileType::BlockDevice => EXT2_FT_BLKDEV,
        FileType::Fifo => EXT2_FT_FIFO,
        FileType::Socket => EXT2_FT_SOCK,
        FileType::SymLink => EXT2_FT_SYMLINK,
        FileType::Unknown => EXT2_FT_UNKNOWN,
    }
}

pub fn node_inode(mode: u16, uid: u32, gid: u32, dev: u32) -> Ext2Inode {
    let mut inode = Ext2Inode::new_regular_file(mode & 0o7777, 0, 0);
    inode.i_mode = mode;
    inode.set_owner(uid, gid);
    if mode & 0o170000 == EXT2_S_IFCHR || mode & 0o170000 == EXT2_S_IFBLK {
        let major = (dev >> 8) & 0xfff;
        let minor = (dev & 0xff) | ((dev >> 12) & 0xfff00);
        if major < 256 && minor < 256 {
            inode.i_block[0] = (major << 8) | minor;
        } else {
            inode.i_block[1] = dev;
        }
    }
    inode
}

pub fn node_device(inode: &Ext2Inode) -> u64 {
    if !matches!(
        inode.file_type(),
        FileType::CharDevice | FileType::BlockDevice
    ) {
        return 0;
    }
    let old = inode.i_block[0];
    if old != 0 {
        old as u64
    } else {
        inode.i_block[1] as u64
    }
}

impl Ext2Fs {
    /// Caller holds the mount write guard and verifies the parent pin and
    /// credentials before entering this method. Recheck the final name here.
    pub fn create_node(
        &mut self,
        parent: u32,
        name: &str,
        mode: u16,
        uid: u32,
        gid: u32,
        dev: u32,
    ) -> Result<u32, &'static str> {
        if name.is_empty() || name.len() > 255 || name.contains('/') || name == "." || name == ".."
        {
            return Err("Invalid filename");
        }
        let mut parent_inode = self.read_inode(parent)?;
        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }
        if parent_inode.i_links_count == 0 {
            return Err("Parent not found");
        }
        let mut data = self.read_directory(&parent_inode)?;
        if dir::find_inode(&data, name).is_some() {
            return Err("File already exists");
        }
        let blocks = directory_io::prepare(
            self.device.as_ref(),
            &parent_inode,
            &self.superblock,
            data.len(),
        )?;
        let mut before = alloc::vec::Vec::new();
        before
            .try_reserve_exact(data.len())
            .map_err(|_| "No memory for directory rollback")?;
        before.extend_from_slice(&data);
        // Stage insertion before inode allocation; a full directory leaves
        // allocation bitmaps untouched. Replace the temporary inode before I/O.
        let offset = add_directory_entry(
            &mut data,
            u32::MAX,
            name,
            dir_entry_type(&node_inode(mode, uid, gid, dev)),
        )?;
        let ino = allocate_inode(
            self.device.as_ref(),
            &self.superblock,
            &mut self.block_groups,
        )?;
        let inode = node_inode(mode, uid, gid, dev);
        if let Err(error) = self.write_inode(ino, &inode) {
            let mut attempted = false;
            inode::release_orphan_inode(
                self.device.as_ref(),
                ino,
                &self.superblock,
                &mut self.block_groups,
                &mut attempted,
            )?;
            return Err(error);
        }
        data[offset..offset + 4].copy_from_slice(&ino.to_le_bytes());
        if let Err(error) = directory_io::write(
            self.device.as_ref(),
            self.superblock.block_size(),
            &blocks,
            &data,
        ) {
            // A device may write bytes before returning an error. Restore the
            // old directory before releasing an inode that a name might reach.
            directory_io::write(
                self.device.as_ref(),
                self.superblock.block_size(),
                &blocks,
                &before,
            )?;
            let mut attempted = false;
            inode::release_orphan_inode(
                self.device.as_ref(),
                ino,
                &self.superblock,
                &mut self.block_groups,
                &mut attempted,
            )?;
            return Err(error);
        }
        parent_inode.update_timestamps(false, true, true);
        self.write_inode(parent, &parent_inode)?;
        self.superblock.decrement_free_inodes();
        self.superblock
            .write_to(self.device.as_ref())
            .map_err(|_| "Failed to write superblock")?;
        Ext2BlockGroupDesc::write_table(self.device.as_ref(), &self.superblock, &self.block_groups)
            .map_err(|_| "Failed to write group descriptors")?;
        Ok(ino)
    }
}
