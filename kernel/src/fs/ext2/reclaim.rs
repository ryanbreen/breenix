//! Retry custody for zero-link inodes. Reclamation walks the block tree from
//! its tail in bounded batches, so memory does not grow with file size. A
//! batch's bits stay excluded from allocation until its deletion metadata and
//! counts have been written.

use super::file::TailBatch;
use super::{Ext2Fs, Ext2Inode};
use crate::block::BlockDevice;
use alloc::collections::BTreeSet;
use spin::Mutex;

/// Allocations detached and released per batch.
const BATCH_BLOCKS: usize = 64;

static QUARANTINE: Mutex<BTreeSet<(usize, bool, u32)>> = Mutex::new(BTreeSet::new());

fn device_key<B: BlockDevice + ?Sized>(device: &B) -> usize {
    device as *const B as *const () as usize
}

pub(super) fn quarantined<B: BlockDevice + ?Sized>(device: &B, inode: bool, number: u32) -> bool {
    QUARANTINE
        .lock()
        .contains(&(device_key(device), inode, number))
}

pub(super) enum ReclaimError {
    /// An I/O step failed; the staged progress resumes on the next attempt.
    Retry,
    /// The inode cannot be reclaimed; it stays allocated on disk.
    Abandon(&'static str),
}

struct Batch {
    tail: TailBatch,
    published: bool,
    next: usize,
}

pub(super) struct Reclaim {
    inode: Ext2Inode,
    batch: Option<Batch>,
    tree_done: bool,
    inode_freed: bool,
    /// Set immediately before a bitmap write, cleared once that allocation is
    /// posted. A clear bit is evidence of an earlier write only when set.
    write_attempted: bool,
}

impl Reclaim {
    pub fn prepare(fs: &Ext2Fs, ino: u32) -> Result<Self, ReclaimError> {
        let mut inode = fs.read_inode(ino).map_err(|_| ReclaimError::Retry)?;
        if inode.i_links_count != 0 {
            return Err(ReclaimError::Abandon("inode is linked"));
        }
        // Fast symlinks keep their target in i_block, and device, FIFO and
        // socket inodes own no blocks; none of their words are pointers.
        if !(inode.is_file() || inode.is_dir() || (inode.is_symlink() && inode.i_blocks != 0)) {
            inode.i_block = [0; 15];
        }
        // An external ACL/xattr block may be shared and its reference count
        // is not maintained here, so the block is left allocated.
        inode.i_file_acl = 0;
        inode.i_size = 0;
        inode.i_dir_acl = 0;
        inode.i_dtime = crate::time::current_unix_time() as u32;
        QUARANTINE
            .lock()
            .insert((device_key(fs.device.as_ref()), true, ino));
        Ok(Self {
            inode,
            batch: None,
            tree_done: false,
            inode_freed: false,
            write_attempted: false,
        })
    }

    /// Advance reclamation by at most `budget` bitmap transitions. Returns
    /// `Ok(true)` once the inode is free and every count is written.
    pub fn finish(
        &mut self,
        fs: &mut Ext2Fs,
        ino: u32,
        budget: &mut usize,
    ) -> Result<bool, ReclaimError> {
        let key = device_key(fs.device.as_ref());
        while !self.tree_done {
            if self.batch.is_none() {
                let tail = super::file::take_tail_blocks(
                    fs.device.as_ref(),
                    &fs.superblock,
                    self.inode.i_block,
                    BATCH_BLOCKS,
                )
                .map_err(|_| ReclaimError::Retry)?;
                let mut quarantine = QUARANTINE.lock();
                for block in &tail.blocks {
                    quarantine.insert((key, false, *block));
                }
                drop(quarantine);
                self.batch = Some(Batch {
                    tail,
                    published: false,
                    next: 0,
                });
            }
            let Self {
                inode,
                batch,
                write_attempted,
                ..
            } = self;
            let batch = batch.as_mut().expect("orphan batch staged");
            if !batch.published {
                // No bitmap bit is freed until no disk pointer names it.
                super::file::write_pointer_blocks(
                    fs.device.as_ref(),
                    fs.superblock.block_size(),
                    &batch.tail.edits,
                )
                .map_err(|_| ReclaimError::Retry)?;
                let mut published = *inode;
                published.i_block = batch.tail.pointers;
                let sectors = (fs.superblock.block_size() / 512) as u32;
                published.i_blocks = published
                    .i_blocks
                    .saturating_sub(batch.tail.blocks.len() as u32 * sectors);
                fs.write_inode(ino, &published)
                    .map_err(|_| ReclaimError::Retry)?;
                *inode = published;
                batch.published = true;
            }
            while let Some(&block) = batch.tail.blocks.get(batch.next) {
                if *budget == 0 {
                    return Ok(false);
                }
                *budget -= 1;
                if super::block_group::release_orphan_block(
                    fs.device.as_ref(),
                    block,
                    &fs.superblock,
                    &mut fs.block_groups,
                    write_attempted,
                )
                .map_err(|_| ReclaimError::Retry)?
                {
                    fs.superblock.increment_free_blocks(1);
                } else {
                    // A duplicate or out-of-range pointer: not ours to count.
                    log::warn!("ext2: orphan inode {} block {} was not allocated", ino, block);
                }
                *write_attempted = false;
                batch.next += 1;
            }
            if !batch.tail.blocks.is_empty() {
                persist_counts(fs)?;
            }
            let mut quarantine = QUARANTINE.lock();
            for block in &batch.tail.blocks {
                quarantine.remove(&(key, false, *block));
            }
            drop(quarantine);
            self.tree_done = batch.tail.blocks.is_empty();
            self.batch = None;
        }
        if !self.inode_freed {
            if *budget == 0 {
                return Ok(false);
            }
            *budget -= 1;
            if super::inode::release_orphan_inode(
                fs.device.as_ref(),
                ino,
                &fs.superblock,
                &mut fs.block_groups,
                &mut self.write_attempted,
            )
            .map_err(|_| ReclaimError::Retry)?
            {
                fs.superblock.increment_free_inodes();
            } else {
                log::warn!("ext2: orphan inode {} was not allocated", ino);
            }
            self.write_attempted = false;
            self.inode_freed = true;
        }
        persist_counts(fs)?;
        QUARANTINE.lock().remove(&(key, true, ino));
        Ok(true)
    }
}

fn persist_counts(fs: &Ext2Fs) -> Result<(), ReclaimError> {
    fs.superblock
        .write_to(fs.device.as_ref())
        .map_err(|_| ReclaimError::Retry)?;
    super::Ext2BlockGroupDesc::write_table(fs.device.as_ref(), &fs.superblock, &fs.block_groups)
        .map_err(|_| ReclaimError::Retry)
}
