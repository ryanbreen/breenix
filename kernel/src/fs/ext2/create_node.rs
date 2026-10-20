//! Ext2 namespace creation: every new inode and its first name, and the
//! FIFO, socket and device inodes mknod creates.
use super::directory_io::{self, Write};
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
        name: impl AsRef<[u8]>,
        mode: u16,
        uid: u32,
        gid: u32,
        dev: u32,
    ) -> Result<u32, &'static str> {
        self.create_node_prepared(parent, name.as_ref(), mode, uid, gid, dev, |_, ino| Ok(ino))
    }

    /// Create a node and name it, with `prepare` readying the caller's
    /// reference to the new inode before the disk changes.
    pub fn create_node_prepared<R>(
        &mut self, parent: u32, name: &[u8], mode: u16, uid: u32, gid: u32, dev: u32,
        prepare: impl FnOnce(&mut Self, u32) -> Result<R, &'static str>,
    ) -> Result<R, &'static str> {
        if name.is_empty() || name.len() > 255 || name.contains(&b'/') || name.contains(&0) || name == b"." || name == b".."
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
        if dir::find_inode_bytes(&data, name).is_some() {
            return Err("File already exists");
        }
        self.make_directory_room(parent, &mut parent_inode, &mut data, name.len())?;
        let node = node_inode(mode, uid, gid, dev);
        self.create_entry(parent, &parent_inode, &data, name, node, None, prepare)
    }

    /// Create inode `node` and name it `name` in directory `parent`, whose
    /// inode is `parent_inode` and whose records `data` already have room for
    /// the name. With `block`, the inode gets one data block, which `block`
    /// fills given the new inode's number. `prepare` readies the caller's
    /// reference to the inode.
    ///
    /// Everything is allocated, `prepare` included, and every write planned
    /// before the disk changes. The writes go bitmaps first, then the new
    /// block and inode, then the name and the parent, and the free counts
    /// last, so a crash leaves at worst an allocation no name reaches. A
    /// failed write is undone with the writes before it; when that fails too,
    /// the mount writes nothing more.
    pub(super) fn create_entry<R>(
        &mut self,
        parent: u32,
        parent_inode: &Ext2Inode,
        data: &[u8],
        name: &[u8],
        mut node: Ext2Inode,
        block: Option<&dyn Fn(u32, &mut [u8])>,
        prepare: impl FnOnce(&mut Self, u32) -> Result<R, &'static str>,
    ) -> Result<R, &'static str> {
        let size = self.superblock.block_size();
        let mut image = directory_io::copy(data)?;
        let offset = dir::add_directory_entry_bytes(&mut image, u32::MAX, name, dir_entry_type(&node))?;
        let mut groups = directory_io::copy(&self.block_groups)?;
        let mut superblock = self.superblock;
        let inode_claim = block_group::claim(self.device.as_ref(), &superblock, &mut groups, true)?;
        let ino = inode_claim.number;
        superblock.decrement_free_inodes();
        image[offset..offset + 4].copy_from_slice(&ino.to_le_bytes());
        let mut contents = Vec::new();
        let block_claim = match block {
            None => None,
            Some(fill) => {
                let claim = block_group::claim(self.device.as_ref(), &superblock, &mut groups, false)?;
                superblock.decrement_free_blocks();
                contents
                    .try_reserve_exact(size)
                    .map_err(|_| "No memory for new block")?;
                contents.resize(size, 0);
                fill(ino, &mut contents);
                node.i_block[0] = claim.number;
                node.i_blocks = (size / 512) as u32;
                Some(claim)
            }
        };
        let mut parent_after = *parent_inode;
        parent_after.update_timestamps(false, true, true);
        if node.is_dir() {
            // The new directory's `..` is another link to the parent.
            parent_after.i_links_count = parent_inode
                .i_links_count
                .checked_add(1)
                .ok_or("Too many links")?;
            inode::note_directory_used(ino, &superblock, &mut groups);
        }
        let replaced = self.read_inode(ino)?;

        let mut writes = Vec::new();
        writes
            .try_reserve_exact(5)
            .map_err(|_| "No memory for directory write plan")?;
        writes.push(Write::Block {
            block: inode_claim.bitmap,
            old: &inode_claim.old,
            new: &inode_claim.new,
        });
        if let Some(claim) = &block_claim {
            writes.push(Write::Block {
                block: claim.bitmap,
                old: &claim.old,
                new: &claim.new,
            });
            writes.push(Write::Fresh {
                block: claim.number,
                new: &contents,
            });
        }
        writes.push(Write::Inode {
            ino,
            old: replaced,
            new: node,
        });
        self.plan_blocks(parent_inode, data, &image, offset, &mut writes)?;
        writes
            .try_reserve(2)
            .map_err(|_| "No memory for directory write plan")?;
        writes.push(Write::Inode {
            ino: parent,
            old: *parent_inode,
            new: parent_after,
        });
        writes.push(Write::Counts {
            superblock: &superblock,
            groups: &groups,
        });

        let prepared = prepare(self, ino)?;
        // The disk changes from here on; nothing below allocates.
        if self.commit_writes(&writes).is_err() {
            drop(prepared);
            return Err("Failed to write new directory entry");
        }
        self.superblock = superblock;
        self.block_groups = groups;
        Ok(prepared)
    }
}
