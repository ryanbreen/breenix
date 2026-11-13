//! Pinned per-inode transfers. Namespace guards protect allocation and pointer
//! publication. File bytes use the pinned device outside those guards. A reserved bitmap bit is
//! visible to other allocators before the namespace is released; no pointer to
//! a new block is published until its complete initialized image was written.

use super::{
    file, live_inode::FileHandle, mount_device::MountDevice, Ext2Fs, Ext2Inode, Ext2Superblock,
};
use crate::block::BlockDevice;
use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::Ordering;

pub(crate) struct DataFile {
    handle: FileHandle,
    device: Arc<MountDevice>,
    inode: Ext2Inode,
    superblock: Ext2Superblock,
    _custody: crate::task::thread::KillCustody,
}

impl DataFile {
    pub(crate) fn open(handle: &FileHandle) -> Result<Self, &'static str> {
        let mut guard = super::fs_sync_guard(super::pin_is_home(handle.object.mount)?)?
            .wait_for_inode(handle.object.key.inode)?;
        let fs = guard.as_mut().ok_or("Missing mount")?;
        let ino = handle.verify(fs)?;
        fs.check_shrink(ino)?;
        let inode = fs.read_inode(ino)?;
        if !inode.is_file() { return Err("Not a regular file"); }
        let custody = crate::task::thread::KillCustody::try_enter()
            .expect("data transfer under mount custody");
        assert!(!handle.object.data_io.swap(true, Ordering::AcqRel));
        Ok(Self {
            handle: handle.clone(),
            device: fs.device.clone(),
            inode,
            superblock: fs.superblock,
            _custody: custody,
        })
    }

    pub(crate) fn size(&self) -> u64 {
        self.inode.size()
    }

    pub(crate) fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>, &'static str> {
        file::read_file_range(
            self.device.as_ref(),
            &self.inode,
            &self.superblock,
            offset,
            length,
        )
        .map_err(|_| "Failed to populate mapped file")
    }

    pub(crate) fn needs_flush(&self) -> bool {
        let guard = super::data_commit_guard(self.handle.object.mount);
        let fs = guard.as_ref().expect("pinned mount");
        self.handle.object.map.disk_epoch() == fs.durable_epoch()
    }

    /// Certify only this inode's candidates. A mount-wide barrier can run
    /// concurrently with another inode's transfer, so its epoch is posted by
    /// that transfer's commit, after its last device completion.
    pub(crate) fn sync(&self, first: u64, last: u64) -> Result<(), &'static str> {
        let before = {
            let _guard = super::data_commit_guard(self.handle.object.mount);
            self.handle.object.map.disk_write_seq()
        };
        self.device
            .flush()
            .map_err(|_| "Failed to flush mapped file")?;
        let _guard = super::data_commit_guard(self.handle.object.mount);
        self.handle.object.map.record_data_durable(before);
        self.handle.object.map.complete_writeback(first, last);
        Ok(())
    }

    pub(crate) fn write(&self, offset: u64, bytes: &[u8]) -> Result<usize, &'static str> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or("File too large")?;
        if end > self.size() {
            return Err("Mapped write exceeds EOF");
        }
        let size = self.superblock.block_size();
        let first = offset / size as u64;
        let last = end.div_ceil(size as u64);
        let mut blocks = Vec::new();
        blocks
            .try_reserve_exact((last - first) as usize)
            .map_err(|_| "No memory for mapped transfer")?;
        let mut commit_groups;
        {
            let mut guard = super::data_commit_guard(self.handle.object.mount);
            let fs = guard.as_mut().expect("pinned mount");
            fs.ensure_writable()?;
            if end > fs.max_file_size() { return Err("File too large"); }
            let inode = fs.read_inode(self.handle.object.key.inode)?;
            let before = super::directory_io::copy(&fs.block_groups)?;
            commit_groups = super::directory_io::copy(&fs.block_groups)?;
            let metadata =
                file::MetadataBatch::new(fs.device.as_ref(), &fs.superblock, &fs.block_groups)
                    .map_err(|_| "No memory for allocation batch")?;
            let planned = (|| {
                for logical in first..last {
                    let existing =
                        file::get_block_num(&metadata, &inode, &fs.superblock, logical as u32)
                            .map_err(|_| "Failed to read mapped block pointer")?;
                    let (physical, allocated) = match existing {
                        Some(block) => (block, false),
                        None => (
                            super::block_group::allocate_data_block(
                                &metadata,
                                &fs.superblock,
                                &mut fs.block_groups,
                            )?,
                            true,
                        ),
                    };
                    blocks.push((logical as u32, physical, allocated));
                }
                Ok::<(), &'static str>(())
            })();
            if planned.is_err() {
                for (_, physical, allocated) in &blocks {
                    if *allocated
                        && super::block_group::free_block(
                            &metadata,
                            *physical,
                            &fs.superblock,
                            &mut fs.block_groups,
                        )
                        .is_err()
                    {
                        drop(metadata);
                        fs.fail_writes();
                        return Err("Failed to release mapped reservation");
                    }
                }
            }
            if metadata.publish().is_err() {
                drop(metadata);
                fs.fail_writes();
                return Err("Failed to reserve mapped blocks");
            }
            drop(metadata);
            persist_counts(fs, &before)?;
            planned?;
        }
        // No namespace, map, allocator or process guard survives this scope.
        let mut completed = 0;
        let mut changed = false;
        let transfer = (|| {
            let mut image = [0u8; 4096];
            for (logical, physical, allocated) in &blocks {
                let block_offset = *logical as u64 * size as u64;
                let start = offset.saturating_sub(block_offset) as usize;
                let stop = size.min((end - block_offset) as usize);
                let at = (block_offset + start as u64 - offset) as usize;
                if *allocated {
                    image[..size].fill(0);
                } else {
                    file::read_ext2_block(
                        self.device.as_ref(),
                        *physical,
                        size,
                        &mut image[..size],
                    )
                    .map_err(|_| "Failed to read mapped data")?;
                }
                if *allocated || image[start..stop] != bytes[at..at + stop - start] {
                    image[start..stop].copy_from_slice(&bytes[at..at + stop - start]);
                    // A failed write may have landed too; retain dirty custody.
                    changed = true;
                    file::write_ext2_block(self.device.as_ref(), *physical, size, &image[..size])
                        .map_err(|_| "Failed to write mapped data")?;
                }
                completed += 1;
            }
            Ok::<(), &'static str>(())
        })();
        let mut guard = super::data_commit_guard(self.handle.object.mount);
        let fs = guard.as_mut().expect("pinned mount");
        let ino = self
            .handle
            .verify(fs)
            .expect("transfer retains pinned inode");
        // Namespace operations may have changed links, ownership or timestamps.
        // Reload those fields instead of overwriting them with the snapshot.
        let mut inode = match fs.read_inode(ino) {
            Ok(inode) => inode,
            Err(error) => {
                fs.fail_writes();
                return Err(error);
            }
        };
        commit_groups.copy_from_slice(&fs.block_groups);
        let before = commit_groups;
        let metadata =
            match file::MetadataBatch::new(fs.device.as_ref(), &fs.superblock, &fs.block_groups) {
                Ok(batch) => batch,
                Err(_) => {
                    fs.fail_writes();
                    return Err("No memory to publish mapped transfer");
                }
            };
        let published = (|| {
            for (index, (logical, physical, allocated)) in blocks.iter().enumerate() {
                if !allocated {
                    continue;
                }
                if index < completed {
                    file::set_block_num(
                        &metadata,
                        &mut inode,
                        &fs.superblock,
                        &mut fs.block_groups,
                        *logical,
                        *physical,
                    )
                    .map_err(|_| "Failed to publish mapped pointer")?;
                    inode.i_blocks += (size / 512) as u32;
                } else {
                    super::block_group::free_block(
                        &metadata,
                        *physical,
                        &fs.superblock,
                        &mut fs.block_groups,
                    )?;
                }
            }
            Ok::<(), &'static str>(())
        })();
        if metadata.publish().is_err() || published.is_err() {
            // An uncertain partial pointer/bitmap write fences the mount,
            // retaining reservations and refusing subsequent writers.
            drop(metadata);
            fs.fail_writes();
            return Err("Failed to publish mapped allocation");
        }
        drop(metadata);
        persist_counts(fs, &before)?;
        let mode_before = inode.i_mode;
        inode.i_mode &= !super::content_privilege(inode.i_mode);
        if changed {
            inode.update_timestamps(false, true, true);
        }
        if changed || inode.i_mode != mode_before || blocks.iter().any(|(_, _, allocated)| *allocated) {
            if fs.write_inode(ino, &inode).is_err() {
                fs.fail_writes();
                return Err("Failed to publish mapped inode");
            }
            self.handle.object.map.note_disk_write(fs.durable_epoch());
        }
        transfer?;
        Ok(bytes.len())
    }
}

/// Sync's staged handles are in inode order, so concurrent global syncs acquire
/// their leases in the same order. The device barrier holds no namespace guard.
pub(crate) fn sync_mount(home: bool, handles: &[FileHandle]) -> Result<(), &'static str> {
    let mut files = Vec::new();
    files
        .try_reserve_exact(handles.len())
        .map_err(|_| "No memory for sync leases")?;
    for handle in handles {
        files.push(DataFile::open(handle)?);
    }
    let mut sequences = Vec::new();
    sequences
        .try_reserve_exact(files.len())
        .map_err(|_| "No memory for sync sequences")?;
    let (device, pin) = {
        let guard = super::fs_sync_guard(home)?;
        let Some(fs) = guard.as_ref() else {
            return Ok(());
        };
        for file in &files {
            sequences.push(file.handle.object.map.disk_write_seq());
        }
        (fs.device.clone(), fs.mount_pin)
    };
    device.flush().map_err(|_| "Failed to sync mapped files")?;
    let _guard = super::data_commit_guard(pin);
    for (file, sequence) in files.iter().zip(sequences) {
        file.handle.object.map.record_data_durable(sequence);
        file.handle.object.map.complete_writeback(0, u64::MAX);
    }
    Ok(())
}

fn persist_counts(
    fs: &mut Ext2Fs,
    before: &[super::Ext2BlockGroupDesc],
) -> Result<(), &'static str> {
    let total = fs
        .block_groups
        .iter()
        .map(|g| u32::from(g.bg_free_blocks_count))
        .sum();
    let total_changed = fs.superblock.s_free_blocks_count != total;
    fs.superblock.s_free_blocks_count = total;
    if super::Ext2BlockGroupDesc::write_table_changed(
        fs.device.as_ref(),
        &fs.superblock,
        &fs.block_groups,
        before,
    )
    .is_err()
        || (total_changed && fs.superblock.write_to(fs.device.as_ref()).is_err())
    {
        fs.fail_writes();
        return Err("Failed to persist mapped allocation counts");
    }
    Ok(())
}

impl Drop for DataFile {
    fn drop(&mut self) {
        self.handle.object.data_io.store(false, Ordering::Release);
        if let Ok(home) = super::pin_is_home(self.handle.object.mount) {
            let waiters = if home {
                &super::HOME_EXT2_WAITERS
            } else {
                &super::ROOT_EXT2_WAITERS
            };
            waiters.wake_up_deferred();
        }
    }
}

/// Transfer a VFS writeback batch through the existing pinned mapped-I/O path.
pub(crate) fn write_mapped_range(handle: &FileHandle, offset: u64, bytes: &[u8]) -> Result<usize, &'static str> {
    DataFile::open(handle)?.write(offset, bytes)
}
