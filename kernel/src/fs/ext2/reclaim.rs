//! Retry custody for zero-link inodes. Detached allocation bits stay excluded
//! from allocation until deletion metadata and counts have completed.

use super::{Ext2Fs, Ext2Inode};
use crate::block::BlockDevice;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use spin::Mutex;

static QUARANTINE: Mutex<BTreeSet<(usize, bool, u32)>> = Mutex::new(BTreeSet::new());

fn device_key<B: BlockDevice + ?Sized>(device: &B) -> usize {
    device as *const B as *const () as usize
}

pub(super) fn quarantined<B: BlockDevice + ?Sized>(device: &B, inode: bool, number: u32) -> bool {
    QUARANTINE
        .lock()
        .contains(&(device_key(device), inode, number))
}

pub(super) struct Reclaim {
    inode: Ext2Inode,
    blocks: Vec<u32>,
    published: bool,
    next: usize,
    inode_freed: bool,
    block_attempted: bool,
    inode_attempted: bool,
}

impl Reclaim {
    pub fn prepare(fs: &Ext2Fs, ino: u32) -> Result<Self, &'static str> {
        let mut inode = fs.read_inode(ino)?;
        if inode.i_links_count != 0 {
            return Err("Cannot reclaim linked inode");
        }
        // External ACL/xattr blocks may be shared. Their reference-count
        // transaction is separate; retain the orphan on an unsupported layout.
        if inode.i_file_acl != 0 {
            return Err("Shared ext2 ACL reclamation is unsupported");
        }
        let blocks =
            super::file::detach_inode_blocks(fs.device.as_ref(), &mut inode, &fs.superblock)
                .map_err(|_| "Failed to collect orphan blocks")?;
        inode.i_dtime = crate::time::current_unix_time() as u32;
        let key = device_key(fs.device.as_ref());
        let mut quarantine = QUARANTINE.lock();
        quarantine.insert((key, true, ino));
        for block in &blocks {
            quarantine.insert((key, false, *block));
        }
        Ok(Self {
            inode,
            blocks,
            published: false,
            next: 0,
            inode_freed: false,
            block_attempted: false,
            inode_attempted: false,
        })
    }

    pub fn finish(&mut self, fs: &mut Ext2Fs, ino: u32) -> Result<bool, &'static str> {
        if !self.published {
            // No bitmap bit is freed until the disk inode has no block pointers.
            fs.write_inode(ino, &self.inode)?;
            self.published = true;
        }
        // One bitmap transition per pass. The service drops the filesystem
        // guard and parks before continuing, so foreground I/O can run between
        // steps even when a large orphan has many allocated blocks.
        if let Some(&block) = self.blocks.get(self.next) {
            let allocated = super::block_group::block_is_allocated(
                fs.device.as_ref(),
                block,
                &fs.superblock,
                &fs.block_groups,
            )?;
            let retry = self.block_attempted;
            self.block_attempted = true;
            if allocated {
                super::block_group::free_block(
                    fs.device.as_ref(),
                    block,
                    &fs.superblock,
                    &mut fs.block_groups,
                )?;
            } else if retry {
                // An earlier bitmap write reported an error after clearing the
                // bit. Quarantine prevented reuse; its count still needs posting.
                let group = ((block - fs.superblock.s_first_data_block)
                    / fs.superblock.s_blocks_per_group) as usize;
                fs.block_groups[group].bg_free_blocks_count += 1;
            } else {
                return Err("Orphan block was already free");
            }
            fs.superblock.increment_free_blocks(1);
            self.next += 1;
            self.block_attempted = false;
            return Ok(false);
        }
        if !self.inode_freed {
            let allocated = super::inode::inode_is_allocated(
                fs.device.as_ref(),
                ino,
                &fs.superblock,
                &fs.block_groups,
            )?;
            let retry = self.inode_attempted;
            self.inode_attempted = true;
            if allocated {
                super::inode::free_inode_bitmap(
                    fs.device.as_ref(),
                    ino,
                    &fs.superblock,
                    &mut fs.block_groups,
                )?;
            } else if retry {
                let group = ((ino - 1) / fs.superblock.s_inodes_per_group) as usize;
                fs.block_groups[group].bg_free_inodes_count += 1;
            } else {
                return Err("Orphan inode was already free");
            }
            fs.superblock.increment_free_inodes();
            self.inode_freed = true;
        }
        fs.superblock
            .write_to(fs.device.as_ref())
            .map_err(|_| "Failed to persist deletion counts")?;
        super::Ext2BlockGroupDesc::write_table(
            fs.device.as_ref(),
            &fs.superblock,
            &fs.block_groups,
        )
        .map_err(|_| "Failed to persist deletion groups")?;
        fs.sync()?;
        let key = device_key(fs.device.as_ref());
        let mut quarantine = QUARANTINE.lock();
        quarantine.remove(&(key, true, ino));
        for block in &self.blocks {
            quarantine.remove(&(key, false, *block));
        }
        Ok(true)
    }
}
