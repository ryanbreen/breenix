//! POSIX rename within one ext2 filesystem.
//!
//! A rename is planned in full before the disk changes: the directory images
//! with the new name placed and the old one removed, a moved directory's `..`,
//! every inode whose link count or times change, and the order of the writes.
//! Every allocation the rename needs is made while planning. The plan then
//! runs as a list of writes, each holding the bytes it replaces. When a write
//! fails, it and the writes before it are rewritten with their old bytes, last
//! first, so the tree is left as it was.
//!
//! The order keeps the destination name resolving throughout. The block that
//! gives the new name its inode is written first, while the old name is still
//! present; the block that drops the old name follows. A replaced name is
//! retargeted in place, one entry in one block, so it names the replaced inode
//! or the renamed one and never nothing. The caller holds the filesystem write
//! guard across the whole rename, so no lookup sees the tree between writes.
//!
//! Nothing after the last write allocates. A replaced inode left without a
//! link is handed to the ext2 finalizer, which releases its blocks and inode
//! once its last observer has gone, the custody unlink gives an open orphan.

use super::live_inode::FileHandle;
use super::{
    add_directory_entry, dir_entry_type, find_entry, is_directory_empty,
    remove_entry, update_directory_entry, write_ext2_block, Ext2Fs, Ext2Inode, EXT2_FT_DIR,
    EXT2_ROOT_INO,
};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

/// Why a rename did not happen. The tree is unchanged after every one of
/// them; `Io` also covers a failed write whose rollback failed too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameError {
    /// The old name, or a directory holding either name, no longer exists.
    NotFound,
    /// A directory would replace a non-directory, or a pathname that must
    /// name a directory names something else.
    NotDirectory,
    /// A non-directory would replace a directory.
    IsDirectory,
    /// The replaced directory has entries other than `.` and `..`.
    NotEmpty,
    /// A directory would move into itself or one of its descendants.
    Invalid,
    /// The replaced directory is in use as a working directory or open.
    Busy,
    /// The new name's directory has no room for another entry.
    NoSpace,
    /// A read or write failed.
    Io,
}

/// One planned write and the bytes it replaces.
enum Write<'a> {
    Block {
        block: u32,
        old: &'a [u8],
        new: &'a [u8],
    },
    Inode {
        ino: u32,
        old: Ext2Inode,
        new: Ext2Inode,
    },
}

fn io<T>(_: T) -> RenameError {
    RenameError::Io
}

fn adjust_links(inode: &mut Ext2Inode, delta: i32) {
    let links = inode.i_links_count as i32 + delta;
    inode.i_links_count = links.clamp(0, u16::MAX as i32) as u16;
}

impl Ext2Fs {
    /// Rename the entry `old_name` in the directory `old_dir` holds to
    /// `new_name` in the directory `new_dir` holds, replacing what that name
    /// names. Each directory is the one a pathname walk found and holds, so
    /// both names are looked up again here, under the write guard, in those
    /// directories. `want_dir` is set when a pathname ended in `/`, so the
    /// renamed entry must be a directory.
    pub fn rename(
        &mut self,
        old_dir: &FileHandle,
        old_name: &str,
        new_dir: &FileHandle,
        new_name: &str,
        want_dir: bool,
    ) -> Result<(), RenameError> {
        let old_dir = old_dir.verify(self).map_err(|_| RenameError::NotFound)?;
        let new_dir = new_dir.verify(self).map_err(|_| RenameError::NotFound)?;
        let same_dir = old_dir == new_dir;
        let old_dir_inode = self.named_dir(old_dir)?;
        let new_dir_inode = self.named_dir(new_dir)?;

        let old_data = self.read_directory(&old_dir_inode).map_err(io)?;
        let source = find_entry(&old_data, old_name)
            .ok_or(RenameError::NotFound)?
            .inode;
        let source_inode = self.read_inode(source).map_err(io)?;
        let source_is_dir = source_inode.is_dir();
        if want_dir && !source_is_dir {
            return Err(RenameError::NotDirectory);
        }
        let new_data = if same_dir {
            None
        } else {
            Some(self.read_directory(&new_dir_inode).map_err(io)?)
        };
        let replaced = find_entry(new_data.as_deref().unwrap_or(&old_data), new_name)
            .map(|entry| entry.inode);
        // Two names for one file, or one name twice: POSIX has rename return
        // successfully and do nothing else, so both names remain.
        if replaced == Some(source) {
            return Ok(());
        }
        if source_is_dir && !same_dir {
            self.check_not_below(source, new_dir)?;
        }

        let victim = match replaced {
            None => None,
            Some(ino) => {
                let inode = self.read_inode(ino).map_err(io)?;
                match (source_is_dir, inode.is_dir()) {
                    (false, true) => return Err(RenameError::IsDirectory),
                    (true, false) => return Err(RenameError::NotDirectory),
                    _ => {}
                }
                if inode.is_dir() {
                    let data = self.read_directory(&inode).map_err(io)?;
                    if !is_directory_empty(&data) {
                        return Err(RenameError::NotEmpty);
                    }
                    // As rmdir: a held directory keeps its inode and blocks.
                    if self
                        .live_inodes
                        .get(ino)
                        .is_some_and(|object| !object.unused())
                    {
                        return Err(RenameError::Busy);
                    }
                }
                // Held from before the disk changes, so the zero-link inode
                // can be handed to the finalizer afterwards without allocating.
                let handle = self.pin_loaded_inode(ino, inode.size()).map_err(io)?;
                Some((ino, inode, handle))
            }
        };
        let victim_is_dir = victim
            .as_ref()
            .is_some_and(|(_, inode, _)| inode.is_dir());

        // The directory images after the rename. In one directory the old
        // name is removed before a new entry is placed, so its room can be
        // reused; a replaced name is retargeted where it stands.
        let block_size = self.superblock.block_size();
        let file_type = dir_entry_type(&source_inode);
        let entry_err = |e: &'static str| {
            if e == "No space in directory" {
                RenameError::NoSpace
            } else {
                RenameError::Io
            }
        };
        let mut old_image = old_data.clone();
        let mut new_image = new_data.clone();
        let named_at = match new_image.as_mut() {
            None if victim.is_some() => {
                let at = update_directory_entry(&mut old_image, new_name, source, file_type)
                    .map_err(io)?;
                remove_entry(&mut old_image, old_name, block_size).map_err(io)?;
                at
            }
            None => {
                remove_entry(&mut old_image, old_name, block_size).map_err(io)?;
                add_directory_entry(&mut old_image, source, new_name, file_type)
                    .map_err(entry_err)?
            }
            Some(image) => {
                remove_entry(&mut old_image, old_name, block_size).map_err(io)?;
                if victim.is_some() {
                    update_directory_entry(image, new_name, source, file_type).map_err(io)?
                } else {
                    add_directory_entry(image, source, new_name, file_type).map_err(entry_err)?
                }
            }
        };
        // A directory moving to another parent names that parent in `..`.
        let moved = if source_is_dir && !same_dir {
            let data = self.read_directory(&source_inode).map_err(io)?;
            let mut image = data.clone();
            update_directory_entry(&mut image, "..", new_dir, EXT2_FT_DIR).map_err(io)?;
            Some((data, image))
        } else {
            None
        };

        // Link counts: the new parent gains the moved directory's `..` and
        // loses the replaced directory's; the old parent loses the moved
        // directory's. In one directory the moved `..` cancels.
        let mut new_dir_after = new_dir_inode;
        let mut old_dir_after = old_dir_inode;
        new_dir_after.update_timestamps(false, true, true);
        old_dir_after.update_timestamps(false, true, true);
        let gained = i32::from(source_is_dir) - i32::from(victim_is_dir);
        let lost = i32::from(source_is_dir);
        if same_dir {
            adjust_links(&mut new_dir_after, gained - lost);
        } else {
            adjust_links(&mut new_dir_after, gained);
            adjust_links(&mut old_dir_after, -lost);
        }
        let mut source_after = source_inode;
        source_after.update_timestamps(false, false, true);
        // A replaced directory loses its name and its own `.`.
        let victim_after = victim.as_ref().map(|(_, inode, _)| {
            let mut after = *inode;
            adjust_links(&mut after, if inode.is_dir() { -2 } else { -1 });
            after.update_timestamps(false, false, true);
            after
        });

        let mut writes = Vec::new();
        match (&new_image, &new_data) {
            (Some(image), Some(data)) => {
                self.plan_blocks(&new_dir_inode, data, image, named_at, &mut writes)?;
                self.plan_blocks(&old_dir_inode, &old_data, &old_image, 0, &mut writes)?;
            }
            _ => self.plan_blocks(&old_dir_inode, &old_data, &old_image, named_at, &mut writes)?,
        }
        if let Some((data, image)) = &moved {
            self.plan_blocks(&source_inode, data, image, 0, &mut writes)?;
        }
        writes.push(Write::Inode {
            ino: new_dir,
            old: new_dir_inode,
            new: new_dir_after,
        });
        if !same_dir {
            writes.push(Write::Inode {
                ino: old_dir,
                old: old_dir_inode,
                new: old_dir_after,
            });
        }
        if let (Some((ino, inode, _)), Some(after)) = (&victim, victim_after) {
            writes.push(Write::Inode {
                ino: *ino,
                old: *inode,
                new: after,
            });
        }
        writes.push(Write::Inode {
            ino: source,
            old: source_inode,
            new: source_after,
        });

        // The disk changes from here on; nothing below allocates.
        for (done, write) in writes.iter().enumerate() {
            if self.apply(write, false).is_err() {
                // The failed write may have landed in part, so it is undone
                // with the rest.
                for undo in writes[..=done].iter().rev() {
                    let _ = self.apply(undo, true);
                }
                return Err(RenameError::Io);
            }
        }

        if let (Some((_, _, handle)), Some(after)) = (victim, victim_after) {
            if after.i_links_count == 0 {
                handle.object.orphan.store(true, Ordering::Release);
                match handle.release_sole() {
                    Ok(object) => object.defer(),
                    // An open descriptor still observes it; its last close
                    // queues the orphan.
                    Err(handle) => drop(handle),
                }
            }
        }
        Ok(())
    }

    /// A directory a walk holds, still named in the tree.
    fn named_dir(&self, ino: u32) -> Result<Ext2Inode, RenameError> {
        let inode = self.read_inode(ino).map_err(io)?;
        // A directory removed since the walk holds no names.
        if !inode.is_dir() || inode.i_links_count == 0 {
            return Err(RenameError::NotFound);
        }
        Ok(inode)
    }

    /// Refuse to move directory `source` into `dir` when `dir` is `source`
    /// or below it, by following `..` from `dir` to the root.
    fn check_not_below(&self, source: u32, dir: u32) -> Result<(), RenameError> {
        let mut ino = dir;
        for _ in 0..self.superblock.s_inodes_count {
            if ino == source {
                return Err(RenameError::Invalid);
            }
            if ino == EXT2_ROOT_INO {
                return Ok(());
            }
            let inode = self.read_inode(ino).map_err(io)?;
            let data = self.read_directory(&inode).map_err(io)?;
            ino = find_entry(&data, "..").ok_or(RenameError::Io)?.inode;
        }
        // More steps than inodes: the `..` chain has a cycle.
        Err(RenameError::Io)
    }

    /// Plan the writes of the blocks of directory `dir` that differ between
    /// `old` and `new`, the block holding byte `first` first.
    fn plan_blocks<'a>(
        &self,
        dir: &Ext2Inode,
        old: &'a [u8],
        new: &'a [u8],
        first: usize,
        writes: &mut Vec<Write<'a>>,
    ) -> Result<(), RenameError> {
        let block_size = self.superblock.block_size();
        if old.len() != new.len() || old.len() % block_size != 0 {
            return Err(RenameError::Io);
        }
        let blocks = old.len() / block_size;
        let first = first / block_size;
        let order = core::iter::once(first).chain((0..blocks).filter(|index| *index != first));
        for index in order.filter(|index| *index < blocks) {
            let range = index * block_size..(index + 1) * block_size;
            if old[range.clone()] == new[range.clone()] {
                continue;
            }
            let block = super::file::get_block_num(
                self.device.as_ref(),
                dir,
                &self.superblock,
                index as u32,
            )
            .map_err(io)?
            .ok_or(RenameError::Io)?;
            writes.push(Write::Block {
                block,
                old: &old[range.clone()],
                new: &new[range],
            });
        }
        Ok(())
    }

    /// Perform one planned write, or with `undo` restore what it replaced.
    fn apply(&self, write: &Write, undo: bool) -> Result<(), ()> {
        match write {
            Write::Block { block, old, new } => write_ext2_block(
                self.device.as_ref(),
                *block,
                self.superblock.block_size(),
                if undo { old } else { new },
            )
            .map_err(|_| ()),
            Write::Inode { ino, old, new } => (if undo { old } else { new })
                .write_to(self.device.as_ref(), *ino, &self.superblock, &self.block_groups)
                .map_err(|_| ()),
        }
    }
}
