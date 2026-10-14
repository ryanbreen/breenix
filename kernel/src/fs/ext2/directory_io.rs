//! Namespace changes as planned writes, and growing a directory that has no
//! room for a new entry.
//!
//! A namespace change (a name added, removed or moved, an inode created or
//! unlinked) is planned in full before the disk changes: every block and inode
//! it writes, with the bytes each replaces, and every allocation the plan
//! needs. The plan then runs in order. When a write fails, it and the writes
//! before it are rewritten with their old bytes, last first, so a write that
//! landed in part is undone with the rest and the tree is left as it was.
//! When one of those rewrites fails too, the tree cannot be restored: the
//! mount records the error in its superblock and refuses every later write.
//!
//! Only the directory blocks whose records change are written, so the writes
//! of a change do not grow with the directory.
use super::{block_group, dir, file, Ext2BlockGroupDesc, Ext2Fs, Ext2Inode, Ext2Superblock};
use crate::block::BlockError;
use alloc::vec::Vec;
use super::directory_extension::Extension;

/// One planned write and what undoing it restores.
pub(super) enum Write<'a> {
    /// A block, and the bytes it held.
    Block {
        block: u32,
        old: &'a [u8],
        new: &'a [u8],
    },
    /// A block the plan allocated, which nothing reaches until a later write
    /// of the plan links it. Undoing the plan clears its bitmap bit, so the
    /// block has nothing to restore.
    Fresh { block: u32, new: &'a [u8] },
    /// An inode, and the inode it replaced.
    Inode {
        ino: u32,
        old: Ext2Inode,
        new: Ext2Inode,
    },
    /// The free counts after the plan, in the superblock and the group
    /// descriptors. The mount's own copies change only once the whole plan
    /// has been written, so undoing this writes those.
    Counts {
        superblock: &'a Ext2Superblock,
        groups: &'a [Ext2BlockGroupDesc],
    },
}

/// A copy for a plan to edit or point at, made before the disk changes.
pub(super) fn copy<T: Copy>(items: &[T]) -> Result<Vec<T>, &'static str> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(items.len())
        .map_err(|_| "No memory for namespace image")?;
    copy.extend_from_slice(items);
    Ok(copy)
}

impl Ext2Fs {
    /// Perform planned writes in order. When one fails, it and the writes
    /// before it are rewritten with their old bytes, last first, and the
    /// result is an error. When one of those rewrites fails too, the tree is
    /// between two states and stays so: the mount writes nothing more.
    pub(super) fn commit_writes(&mut self, writes: &[Write]) -> Result<(), ()> {
        for (done, write) in writes.iter().enumerate() {
            if self.apply(write, false).is_err() {
                // The failed write may have landed in part, so it is undone
                // with the rest. Undoing stops at the first rewrite that
                // fails, so what stays on disk is a prefix of the plan: the
                // state a crash there leaves, with counts high, never low.
                let restored = writes[..=done]
                    .iter()
                    .rev()
                    .all(|undo| self.apply(undo, true).is_ok());
                if !restored {
                    self.fail_writes();
                }
                return Err(());
            }
        }
        Ok(())
    }

    /// Plan the writes of the blocks of directory `dir` that differ between
    /// `old` and `new`, the block holding byte `first` first. Mapping a block
    /// can read and allocate, so plans are made before the disk changes.
    pub(super) fn plan_blocks<'a>(
        &self,
        dir: &Ext2Inode,
        old: &'a [u8],
        new: &'a [u8],
        first: usize,
        writes: &mut Vec<Write<'a>>,
    ) -> Result<(), &'static str> {
        let block_size = self.superblock.block_size();
        if block_size == 0 || old.len() != new.len() || old.len() % block_size != 0 {
            return Err("Invalid directory write plan");
        }
        let blocks = old.len() / block_size;
        let first = first / block_size;
        let order = core::iter::once(first).chain((0..blocks).filter(|index| *index != first));
        for index in order.filter(|index| *index < blocks) {
            let range = index * block_size..(index + 1) * block_size;
            if old[range.clone()] == new[range.clone()] {
                continue;
            }
            let block = file::get_block_num(self.device.as_ref(), dir, &self.superblock, index as u32)
                .map_err(|e| {
                    if e == BlockError::OutOfMemory {
                        "No memory for directory addressing"
                    } else {
                        "Failed to read directory addressing"
                    }
                })?
                .ok_or("No space for directory data")?;
            writes
                .try_reserve(1)
                .map_err(|_| "No memory for directory write plan")?;
            writes.push(Write::Block {
                block,
                old: &old[range.clone()],
                new: &new[range],
            });
        }
        Ok(())
    }

    /// Perform one planned write, or with `undo` restore what it replaced.
    fn apply(&mut self, write: &Write, undo: bool) -> Result<(), ()> {
        let size = self.superblock.block_size();
        match write {
            Write::Block { block, old, new } => {
                file::write_ext2_block(self.device.as_ref(), *block, size, if undo { old } else { new })
                    .map_err(|_| ())
            }
            Write::Fresh { .. } if undo => Ok(()),
            Write::Fresh { block, new } => {
                file::write_ext2_block(self.device.as_ref(), *block, size, new).map_err(|_| ())
            }
            Write::Inode { ino, old, new } => if undo {
                self.write_inode_disk(*ino, old)
            } else {
                self.write_inode(*ino, new)
            }
            .map_err(|_| ()),
            Write::Counts { superblock, groups } => {
                let (superblock, groups) = if undo {
                    (&self.superblock, &self.block_groups[..])
                } else {
                    (*superblock, *groups)
                };
                Ext2BlockGroupDesc::write_table(self.device.as_ref(), superblock, groups)
                    .map_err(|_| ())?;
                superblock.write_to(self.device.as_ref()).map_err(|_| ())
            }
        }
    }
}

impl Ext2Fs {
    /// Make room in the directory `dir_ino` for an entry with a `name_len`-byte name.
    /// `data` holds the directory's records and `inode` is the caller's copy of its inode.
    /// When no record has room, the directory gets one more block holding a single empty
    /// record: the block is written first, then linked at the directory's end with the
    /// size grown, so the directory on disk is whole at every step and stays valid whether
    /// or not the caller's insertion then succeeds. `data` and `inode` are updated to match.
    pub(super) fn make_directory_room(
        &mut self,
        dir_ino: u32,
        inode: &mut Ext2Inode,
        data: &mut Vec<u8>,
        name_len: usize,
    ) -> Result<(), &'static str> {
        if dir::has_room(data, name_len)? {
            return Ok(());
        }
        let size = self.superblock.block_size();
        if size > 4096 || data.len() as u64 != inode.size() || data.len() % size != 0 {
            return Err("Corrupt directory size");
        }
        let logical = u32::try_from(data.len() / size).map_err(|_| "No space in directory")?;
        let new_size = u32::try_from(data.len() + size).map_err(|_| "No space in directory")?;
        data.try_reserve_exact(size)
            .map_err(|_| "No memory for directory data")?;
        // One empty record spanning the block: inode 0, rec_len the block size.
        let mut record = [0u8; 4096];
        record[4..6].copy_from_slice(&(size as u16).to_le_bytes());

        let mut groups = Vec::new();
        groups.try_reserve_exact(self.block_groups.len()).map_err(|_| "No memory for directory transaction")?;
        groups.extend_from_slice(&self.block_groups);
        let transaction = Extension::new(self.device.as_ref(), &self.superblock, &groups)?;
        let block = block_group::allocate_data_block(&transaction, &self.superblock, &mut groups)
            .map_err(|e| if e == "No free blocks available" { "No space for another directory block" } else { e })?;
        let mut grown = *inode;
        file::write_ext2_block(&transaction, block, size, &record[..size])
            .map_err(|_| "Failed to write directory block")?;
        file::set_block_num(&transaction, &mut grown, &self.superblock, &mut groups, logical, block)
            .map_err(|e| if e == BlockError::NoSpace { "No space for directory addressing" } else { "Failed to link directory block" })?;
        grown.i_blocks += (size / 512) as u32;
        grown.i_size = new_size;
        grown.write_to(&transaction, dir_ino, &self.superblock, &groups)
            .map_err(|_| "Failed to write inode")?;
        let mut sb = self.superblock;
        sb.s_free_blocks_count = groups.iter().map(|group| group.bg_free_blocks_count as u32).sum();
        sb.write_to(&transaction).map_err(|_| "Failed to write superblock")?;
        Ext2BlockGroupDesc::write_table(&transaction, &sb, &groups)
            .map_err(|_| "Failed to write block group descriptors")?;
        let order = transaction.block_order(grown.i_block, logical, block)
            .map_err(|_| "Failed to verify directory addressing")?;
        if let Err(restored) = transaction.publish(&order) {
            if !restored {
                self.block_groups = groups;
                self.superblock = sb;
                return Err("Failed to restore directory extension; allocations retained");
            }
            return Err("Failed to publish directory extension");
        }
        self.block_groups = groups;
        self.superblock = sb;
        if let Some(object) = self.live_inodes.get(dir_ino) {
            object.map.note_disk_write(self.durable_epoch());
        }
        *inode = grown;
        data.extend_from_slice(&record[..size]);
        Ok(())
    }
}
