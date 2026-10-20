//! ext2 filesystem implementation
//!
//! The Second Extended Filesystem (ext2) is a classic Linux filesystem.
//! This module provides structures and functions for parsing ext2 filesystems.

pub mod block_group;
pub mod create_node;
pub mod dir;
mod directory_io;
mod directory_extension;
mod fifo_times;
pub mod file;
pub mod generation;
pub mod inode;
mod inode_index;
pub mod live_inode;
mod mount_device;
mod namespace_alloc;
mod path;
mod reclaim;
mod rename;
mod sector_cache;
pub mod superblock;
pub mod writeback;

pub use block_group::*;
pub use dir::*;
pub use file::*;
pub use inode::*;
pub use rename::RenameError;
pub use superblock::*;

use crate::block::BlockDevice;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use spin::RwLock;

use create_node::dir_entry_type;

/// What statfs reports for a mounted ext2 filesystem.
#[derive(Clone, Copy, Debug)]
pub struct FsStats {
    pub block_size: u64,
    pub blocks: u64,
    pub reserved_blocks: u64,
    pub free_blocks: u64,
    pub inodes: u64,
    pub free_inodes: u64,
}

/// A mounted ext2 filesystem instance
///
/// Holds the superblock, block group descriptors, and a reference
/// to the underlying block device for filesystem operations.
pub struct Ext2Fs {
    namespace: generation::Namespace,
    /// The filesystem superblock
    pub superblock: Ext2Superblock,
    /// Block group descriptors
    pub block_groups: Vec<Ext2BlockGroupDesc>,
    /// The underlying block device, through the mount's write fence and its
    /// copy of recently transferred blocks
    pub device: alloc::boxed::Box<mount_device::MountDevice>,
    /// Set when the tree was left inconsistent; the device then refuses
    /// every write.
    failed: alloc::sync::Arc<AtomicBool>,
    /// Mount ID for VFS integration
    pub mount_id: usize,
    pub mount_pin: live_inode::MountPin,
    live_inodes: live_inode::LiveInodes,
    orphan_reclaims: alloc::collections::BTreeMap<u32, Option<reclaim::Reclaim>>,
    finalization_cursor: u32,
    /// The last inode the cache service reached with budget left; the next
    /// pass starts after it.
    eviction_cursor: u32,
    shrink_reclaims: alloc::collections::BTreeMap<u32, PendingShrink>,
}

struct PendingShrink {
    progress: reclaim::Reclaim,
    owner: alloc::sync::Weak<()>,
    failed: bool,
}

/// A live truncate caller owns its suffix work across mount-guard releases.
/// Dropping the ticket makes unfinished work available to the finalizer.
pub struct ShrinkTicket {
    inode: u32,
    mount: live_inode::MountPin,
    owner: alloc::sync::Arc<()>,
    // Drop the owner before allowing a pending kill to abandon this stack.
    _custody: crate::task::thread::KillCustody,
}

impl Drop for ShrinkTicket {
    fn drop(&mut self) { writeback::request(); }
}

pub(crate) const EXT2_READ_ONLY: &str = "Read-only filesystem";

/// Bitmap transitions suffix/orphan reclamation performs per write-guard hold.
const RECLAIM_BUDGET: usize = 128;

/// What a finalizer pass left behind.
pub(super) enum Finalize {
    /// No queued orphan remains.
    Idle,
    /// The budget ran out; continue after releasing the guard.
    More,
    /// An I/O step failed; retry after the service's delay.
    Retry,
}

impl Finalize {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Retry, _) | (_, Self::Retry) => Self::Retry,
            (Self::More, _) | (_, Self::More) => Self::More,
            _ => Self::Idle,
        }
    }
}

static ROOT_NAMESPACE: spin::Once<generation::Namespace> = spin::Once::new();

/// Root inode selected at mount time; unchanged throughout the boot.
pub fn root_inode() -> u32 { root_namespace().root }

pub fn root_namespace() -> generation::Namespace {
    ROOT_NAMESPACE.get().copied().unwrap_or(generation::Namespace::RAW)
}

impl Ext2Fs {
    pub fn namespace(&self) -> generation::Namespace { self.namespace }
}

impl generation::Tree for Ext2Fs {
    fn directory(&self, parent: u32, name: &str) -> Result<u32, &'static str> {
        let parent = self.read_inode(parent)?;
        let ino = self.lookup_in_dir(&parent, name)?.ok_or("missing userland directory")?;
        if !self.read_inode(ino)?.is_dir() { return Err("userland mount point is not a directory"); }
        Ok(ino)
    }

    fn contents(&self, parent: u32, name: &str) -> Result<Vec<u8>, &'static str> {
        let parent = self.read_inode(parent)?;
        let ino = self.lookup_in_dir(&parent, name)?.ok_or("missing userland completion record")?;
        let inode = self.read_inode(ino)?;
        if !inode.is_file() || inode.size() > 128 { return Err("invalid userland completion record"); }
        self.read_file_content(&inode)
    }
}

impl Ext2Fs {
    /// Create a new ext2 filesystem instance from a block device
    ///
    /// Reads and validates the superblock and block group descriptors. The
    /// mount keeps a write-through copy of the blocks it transfers.
    pub fn new(
        device: alloc::boxed::Box<dyn BlockDevice>,
        mount_id: usize,
    ) -> Result<Self, &'static str> {
        Self::mount(device, mount_id, true)
    }

    /// A mount that reads every block from the device: the fault-injection
    /// leg arms faults on blocks the mount has already read.
    #[cfg(feature = "fs_fault_inject")]
    pub fn new_uncached(
        device: alloc::boxed::Box<dyn BlockDevice>,
        mount_id: usize,
    ) -> Result<Self, &'static str> {
        Self::mount(device, mount_id, false)
    }

    fn mount(
        device: alloc::boxed::Box<dyn BlockDevice>,
        mount_id: usize,
        cached: bool,
    ) -> Result<Self, &'static str> {
        // Read the superblock
        let superblock = Ext2Superblock::read_from(device.as_ref())
            .map_err(|_| "Failed to read ext2 superblock")?;

        if !superblock.is_valid() {
            return Err("Invalid ext2 magic number");
        }

        // Read block group descriptors
        let block_groups = Ext2BlockGroupDesc::read_table(device.as_ref(), &superblock)
            .map_err(|_| "Failed to read block group descriptors")?;

        let failed = alloc::sync::Arc::new(AtomicBool::new(false));
        Ok(Self {
            namespace: generation::Namespace::RAW,
            superblock,
            block_groups,
            device: alloc::boxed::Box::new(mount_device::MountDevice::new(device, failed.clone(), cached)),
            failed,
            mount_id,
            mount_pin: live_inode::MountPin::new(mount_id)?,
            live_inodes: live_inode::LiveInodes::new(),
            orphan_reclaims: alloc::collections::BTreeMap::new(),
            finalization_cursor: 0,
            eviction_cursor: 0,
            shrink_reclaims: alloc::collections::BTreeMap::new(),
        })
    }

    pub fn is_read_only(&self) -> bool { self.device.is_read_only() }

    pub fn ensure_writable(&self) -> Result<(), &'static str> {
        if self.is_read_only() { Err(EXT2_READ_ONLY) } else { Ok(()) }
    }

    /// Stop writing after a failure left the tree inconsistent: record the
    /// error in the superblock, then refuse every later write. Reads continue.
    fn fail_writes(&mut self) {
        self.superblock.s_state |= EXT2_ERROR_FS;
        let _ = self.superblock.write_to(self.device.as_ref());
        self.failed.store(true, Ordering::Release);
    }

    /// Capacity and free counts for statfs. The free counts are the sums of
    /// the group descriptors' counts, which each allocation and release posts
    /// with its bitmap bit, as Linux ext2 reports them.
    pub fn statfs(&self) -> FsStats {
        let (free_blocks, free_inodes) =
            self.block_groups.iter().fold((0u64, 0u64), |(b, i), g| {
                (
                    b + u64::from(g.bg_free_blocks_count),
                    i + u64::from(g.bg_free_inodes_count),
                )
            });
        FsStats {
            block_size: self.superblock.block_size() as u64,
            blocks: u64::from(self.superblock.s_blocks_count),
            reserved_blocks: u64::from(self.superblock.s_r_blocks_count),
            free_blocks,
            inodes: u64::from(self.superblock.s_inodes_count),
            free_inodes,
        }
    }

    /// The superblock and group descriptors as the device holds them, read
    /// past the mount's copy of recent blocks, for checks that compare the
    /// mount's counts with what reached the disk.
    pub fn persisted_metadata(&self) -> Result<(Ext2Superblock, Vec<Ext2BlockGroupDesc>), &'static str> {
        let device = self.device.uncached();
        let superblock = Ext2Superblock::read_from(device).map_err(|_| "Failed to read superblock")?;
        let groups = Ext2BlockGroupDesc::read_table(device, &superblock)
            .map_err(|_| "Failed to read block group descriptors")?;
        Ok((superblock, groups))
    }

    pub fn pin_inode(&self, inode_num: u32) -> Result<live_inode::FileHandle, &'static str> {
        let inode = self.read_inode(inode_num)?;
        self.pin_loaded_inode(inode_num, inode.size())
    }

    /// The caller read this inode under the same FS guard that protects the pin.
    pub(crate) fn pin_loaded_inode(
        &self,
        inode_num: u32,
        size: u64,
    ) -> Result<live_inode::FileHandle, &'static str> {
        self.live_inodes.pin(self.mount_pin, inode_num, size)
    }

    /// Service mapped data and failed shrinks before the orphan commit window.
    fn finalize_writeback(&mut self) -> Finalize {
        if self.is_read_only() { return Finalize::Idle; }
        let mut budget = RECLAIM_BUDGET;
        let mut outcome = self.finalize_shrinks(&mut budget);
        let mut after = None;
        for _ in 0..live_inode::EVICTION_BATCH {
            let Some(object) = self.live_inodes.next_times(after) else {
                break;
            };
            after = Some(object.key.inode);
            if self.flush_inode_times(object.key.inode).is_err() {
                outcome = Finalize::Retry;
            }
        }
        if self.live_inodes.next_times(after).is_some() {
            outcome = outcome.merge(Finalize::More);
        }
        let mut cache_budget = crate::memory::file_map::EVICT_BUDGET;
        let evictions = self.live_inodes.evictions(self.eviction_cursor);
        let mut write_budget = crate::memory::file_map::EVICT_BUDGET;
        for object in &evictions {
            if write_budget != 0 && cache_budget != 0 {
                self.eviction_cursor = object.key.inode;
            }
            match object
                .map
                .service_writeback(self, object.key.inode, &mut write_budget)
            {
                Ok(true) => outcome = outcome.merge(Finalize::More),
                Err(_) => outcome = Finalize::Retry,
                Ok(false) => {}
            }
            if object.map.evict(&mut cache_budget) {
                outcome = outcome.merge(Finalize::More);
            }
        }
        // One pass takes a bounded batch of inodes; any left pending need
        // another pass even when every inode in this batch was emptied.
        if evictions.len() == live_inode::EVICTION_BATCH && self.live_inodes.evictions_pending() {
            outcome = outcome.merge(Finalize::More);
        }
        outcome
    }

    /// Stage and release a bounded allocation batch, retaining quarantine
    /// and the detachment barrier across guard releases.
    fn finalize_pending_orphans(&mut self) -> Finalize {
        if self.is_read_only() { return Finalize::Idle; }
        let mut budget = RECLAIM_BUDGET;
        let objects = self.live_inodes.pending(self.finalization_cursor);
        let mut outcome = self.finalize_orphans(&objects, &mut budget);
        if matches!(outcome, Finalize::Idle)
            && !self
                .live_inodes
                .pending(self.finalization_cursor)
                .is_empty()
        {
            outcome = Finalize::More;
        }
        outcome
    }

    /// Advance only this captured cohort, preserving the transition budget.
    fn finalize_orphans(
        &mut self,
        objects: &[alloc::sync::Arc<live_inode::LiveInode>],
        budget: &mut usize,
    ) -> Finalize {
        if *budget == 0 {
            return Finalize::More;
        }
        let mut outcome = Finalize::Idle;
        let mut ready = Vec::new();
        if ready.try_reserve_exact(objects.len().min(32)).is_err() {
            return Finalize::Retry;
        }
        let mut barrier = false;
        let mut staging_budget = RECLAIM_BUDGET;
        let mut visited = 0;
        for object in objects {
            if !self.live_inodes.is_pending(object) {
                continue;
            }
            if staging_budget == 0 || visited == 32 {
                outcome = outcome.merge(Finalize::More);
                break;
            }
            visited += 1;
            match self.stage_orphan(object, &mut staging_budget) {
                Ok(needs_barrier) => {
                    barrier |= needs_barrier;
                    ready.push(object.clone());
                }
                Err(reclaim::ReclaimError::Retry) => outcome = Finalize::Retry,
                Err(reclaim::ReclaimError::Abandon(reason)) => {
                    log::warn!(
                        "ext2: leaving orphan inode {} allocated: {}",
                        object.key.inode,
                        reason
                    );
                    self.orphan_reclaims.remove(&object.key.inode);
                    self.live_inodes.remove(object);
                }
            }
        }
        if barrier && self.sync().is_err() {
            // No staged orphan may expose a free bit after a failed barrier.
            return Finalize::Retry;
        }
        for object in ready {
            if !object.unused() {
                continue;
            }
            if *budget == 0 {
                return outcome.merge(Finalize::More);
            }
            self.finalization_cursor = object.key.inode;
            match self.reclaim_orphan(&object, budget) {
                Ok(true) | Err(reclaim::ReclaimError::Abandon(_)) => {}
                Ok(false) => return outcome.merge(Finalize::More),
                Err(reclaim::ReclaimError::Retry) => outcome = Finalize::Retry,
            }
        }
        if objects
            .iter()
            .any(|object| self.live_inodes.is_pending(object))
        {
            outcome = outcome.merge(Finalize::More);
        }
        outcome
    }

    /// Retry abandoned or failed shrinks without taking work from a live caller.
    fn finalize_shrinks(&mut self, budget: &mut usize) -> Finalize {
        let inodes: Vec<u32> = self.shrink_reclaims.keys().copied().collect();
        let mut outcome = Finalize::Idle;
        for ino in inodes {
            if *budget == 0 {
                return outcome.merge(Finalize::More);
            }
            if self.shrink_reclaims[&ino].owner.strong_count() != 0 {
                outcome = outcome.merge(Finalize::More);
                continue;
            }
            let mut pending = self.shrink_reclaims.remove(&ino).expect("queued shrink");
            match pending.progress.finish(self, ino, budget) {
                Ok(true) => {}
                result => {
                    pending.failed |= result.is_err();
                    self.shrink_reclaims.insert(ino, pending);
                    if matches!(result, Ok(false)) {
                        return outcome.merge(Finalize::More);
                    }
                    outcome = Finalize::Retry;
                }
            }
        }
        outcome
    }

    /// Retain each detachment until the shared barrier and bitmap phase finish.
    fn stage_orphan(
        &mut self,
        object: &alloc::sync::Arc<live_inode::LiveInode>,
        budget: &mut usize,
    ) -> Result<bool, reclaim::ReclaimError> {
        let ino = object.key.inode;
        if self.shrink_reclaims.contains_key(&ino) {
            return Err(reclaim::ReclaimError::Retry);
        }
        // Allocate the custody slot before staging can modify disk pointers.
        self.orphan_reclaims.entry(ino).or_insert(None);
        let mut progress = match self.orphan_reclaims.get_mut(&ino).and_then(Option::take) {
            Some(progress) => progress,
            None => reclaim::Reclaim::prepare(self, ino)?,
        };
        let result = progress.stage(self, ino, budget);
        *self
            .orphan_reclaims
            .get_mut(&ino)
            .expect("reserved orphan custody") = Some(progress);
        result
    }

    /// Release an unobserved orphan's blocks and inode under the write guard,
    /// spending at most `budget` bitmap transitions. Returns `Ok(false)` when
    /// the budget ran out first; that and a retryable failure keep the staged
    /// progress for the finalizer to resume.
    fn reclaim_orphan(
        &mut self,
        object: &alloc::sync::Arc<live_inode::LiveInode>,
        budget: &mut usize,
    ) -> Result<bool, reclaim::ReclaimError> {
        let ino = object.key.inode;
        if self.shrink_reclaims.contains_key(&ino) {
            return Err(reclaim::ReclaimError::Retry);
        }
        self.orphan_reclaims.entry(ino).or_insert(None);
        let prepared = match self.orphan_reclaims.get_mut(&ino).and_then(Option::take) {
            Some(progress) => Ok(progress),
            None => reclaim::Reclaim::prepare(self, ino),
        };
        let result = prepared.and_then(|mut progress| {
            let result = progress.finish(self, ino, budget);
            if !matches!(result, Ok(true)) {
                *self
                    .orphan_reclaims
                    .get_mut(&ino)
                    .expect("reserved orphan custody") = Some(progress);
            }
            result
        });
        match &result {
            Ok(true) => {
                self.orphan_reclaims.remove(&ino);
                self.live_inodes.remove(object);
            }
            Err(reclaim::ReclaimError::Abandon(reason)) => {
                // The inode stays allocated on disk; stop retrying it.
                log::warn!("ext2: leaving orphan inode {} allocated: {}", ino, reason);
                self.live_inodes.remove(object);
            }
            Ok(false) | Err(reclaim::ReclaimError::Retry) => {}
        }
        result
    }

    /// Failed shrink custody belongs to this inode; unrelated files remain usable.
    pub fn check_shrink(&self, inode_num: u32) -> Result<(), &'static str> {
        if self.shrink_reclaims.contains_key(&inode_num) {
            Err("Unfinished ext2 shrink after I/O failure")
        } else {
            Ok(())
        }
    }

    /// Read an inode from the filesystem
    pub fn read_inode(&self, inode_num: u32) -> Result<Ext2Inode, &'static str> {
        let mut inode = Ext2Inode::read_from(
            self.device.as_ref(),
            inode_num,
            &self.superblock,
            &self.block_groups,
        )
        .map_err(|_| "Failed to read inode")?;
        if let Some(object) = self.live_inodes.get(inode_num) {
            object.overlay_times(&mut inode);
        }
        Ok(inode)
    }

    pub fn flush_inode_times(&mut self, inode_num: u32) -> Result<(), &'static str> {
        let Some(object) = self.live_inodes.get(inode_num) else {
            return Ok(());
        };
        let atime = object.atime_pending.swap(0, Ordering::AcqRel);
        let mtime = object.mtime_pending.swap(0, Ordering::AcqRel);
        let ctime = object.ctime_pending.swap(0, Ordering::AcqRel);
        if atime == 0 && mtime == 0 && ctime == 0 {
            return Ok(());
        }
        let result = (|| {
            let mut inode = Ext2Inode::read_from(
                self.device.as_ref(),
                inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to read inode")?;
            fifo_times::apply(&mut inode, atime, mtime, ctime);
            self.write_inode_disk(inode_num, &inode)
        })();
        if result.is_err() {
            let _ = object.atime_pending.compare_exchange(
                0,
                atime,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            let _ = object.mtime_pending.compare_exchange(
                0,
                mtime,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            let _ = object.ctime_pending.compare_exchange(
                0,
                ctime,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        result
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

    /// Read file content from an inode
    fn read_file_content(&self, inode: &Ext2Inode) -> Result<Vec<u8>, &'static str> {
        read_file(self.device.as_ref(), inode, &self.superblock)
            .map_err(|_| "Failed to read file content")
    }

    /// Read a range of file content from an inode
    pub(crate) fn read_file_range(
        &self,
        inode: &Ext2Inode,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, &'static str> {
        read_file_range(
            self.device.as_ref(),
            inode,
            &self.superblock,
            offset,
            length,
        )
        .map_err(|_| "Failed to read file range")
    }

    /// External readers, including exec, use the resident inode cache while
    /// holding the same mount guard that protects the disk read.
    pub fn read_file_range_coherent(
        &self,
        ino: u32,
        inode: &Ext2Inode,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, &'static str> {
        let mut bytes = self.read_file_range(inode, offset, length)?;
        if let Some(object) = self.live_inodes.get(ino) {
            object.map.overlay(offset, &mut bytes)?;
        }
        Ok(bytes)
    }

    pub fn read_file_content_coherent(
        &self,
        ino: u32,
        inode: &Ext2Inode,
    ) -> Result<Vec<u8>, &'static str> {
        let mut bytes = self.read_file_content(inode)?;
        if let Some(object) = self.live_inodes.get(ino) {
            object.map.overlay(0, &mut bytes)?;
        }
        Ok(bytes)
    }

    /// `read_file_content_coherent`, read 64 KiB at a time. Before each
    /// chunk it asks `stop`, and gives up with `Ok(None)` when the reader no
    /// longer wants the content. The whole file's buffer is reserved once, at
    /// its exact size: a heap that cannot hold it is ENOMEM, never a panic.
    /// Other failures are EIO.
    pub fn read_file_content_coherent_unless(
        &self,
        ino: u32,
        inode: &Ext2Inode,
        stop: impl Fn() -> bool,
    ) -> Result<Option<Vec<u8>>, i32> {
        use crate::syscall::errno::{EIO, ENOMEM};
        const CHUNK: u64 = 64 * 1024;
        let size = inode.size();
        // The whole-file read's bound: no file holds more than the filesystem.
        let capacity = unsafe {
            core::ptr::read_unaligned(core::ptr::addr_of!(self.superblock.s_blocks_count)) as u64
        }
        .saturating_mul(self.superblock.block_size() as u64);
        if size > capacity {
            return Err(EIO);
        }
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(size as usize).map_err(|_| ENOMEM)?;
        while (bytes.len() as u64) < size {
            if stop() {
                return Ok(None);
            }
            let offset = bytes.len() as u64;
            let chunk = self
                .read_file_range_coherent(ino, inode, offset, CHUNK.min(size - offset) as usize)
                .map_err(|_| EIO)?;
            if chunk.is_empty() {
                break;
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Some(bytes))
    }

    /// Raw writeback bypasses cache overlays and size transitions. The
    /// snapshot is clipped to EOF, so this never extends the inode. Writeback
    /// has no writer credentials and conservatively strips execution privilege.
    pub(crate) fn write_mapped_range(
        &mut self,
        ino: u32,
        offset: u64,
        bytes: &[u8],
    ) -> Result<usize, &'static str> {
        self.ensure_writable()?;
        self.write_file_range_disk(ino, offset, bytes, true, true)
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
        self.write_file_range_as(inode_num, offset, data, false)
    }

    /// User content changes clear execution privilege bits for unprivileged writers.
    pub fn write_file_range_as(
        &mut self,
        inode_num: u32,
        offset: u64,
        data: &[u8],
        unprivileged: bool,
    ) -> Result<usize, &'static str> {
        self.ensure_writable()?;
        // Private mappings: give bound holes cache frames and reserve poison
        // room before the disk changes; afterwards only apply what ext2
        // published, which allocates nothing.
        let mapped = self.live_inodes.get(inode_num);
        if let Some(object) = &mapped {
            object.map.prepare_write(offset, data.len())?;
        }
        let result = self.write_file_range_disk(inode_num, offset, data, unprivileged, false);
        if let Some(object) = mapped {
            object.map.transition(object.size.load(Ordering::Acquire));
            match result {
                Ok(written) => object.map.commit_write(offset, &data[..written]),
                Err(_) => object.map.invalidate(offset, data.len() as u64),
            }
            object.map.repair(self, inode_num);
        }
        result
    }

    fn write_file_range_disk(
        &mut self,
        inode_num: u32,
        offset: u64,
        data: &[u8],
        unprivileged: bool,
        mapped: bool,
    ) -> Result<usize, &'static str> {
        self.check_shrink(inode_num)?;
        if data.is_empty() {
            return Ok(0);
        }

        if offset
            .checked_add(data.len() as u64)
            .map_or(true, |end| end > self.max_file_size())
        {
            return Err("File too large");
        }

        // Read the inode
        let mut inode = self.read_inode(inode_num)?;

        // Verify it's a regular file
        if !inode.is_file() {
            return Err("Not a regular file");
        }

        let mode_before = inode.i_mode;
        let times_before = (inode.i_mtime, inode.i_ctime);
        if unprivileged {
            let kill = 0o4000
                | if inode.i_mode & 0o0010 != 0 {
                    0o2000
                } else {
                    0
                };
            inode.i_mode &= !kill;
        }

        // Allocation updates the group counters. Publish their aggregate and
        // the descriptors once per write, including partial allocation failure.
        let free_before: u32 = self
            .block_groups
            .iter()
            .map(|group| group.bg_free_blocks_count as u32)
            .sum();
        let object = self.live_inodes.get(inode_num);
        let previous_epoch = object
            .as_ref()
            .map(|object| object.map.note_disk_write(self.durable_epoch()));
        let written = if mapped {
            file::write_mapped_file_range(
                self.device.as_ref(),
                &mut inode,
                &self.superblock,
                &mut self.block_groups,
                offset,
                data,
            )
        } else {
            file::write_file_range_tracked(
                self.device.as_ref(),
                &mut inode,
                &self.superblock,
                &mut self.block_groups,
                offset,
                data,
            )
        };
        if written == Err(crate::block::BlockError::MetadataPublishFailed) {
            // The bitmap may have landed only in part. Do not publish inode
            // pointers or build another allocation on an uncertain bitmap.
            self.fail_writes();
            return Err("Failed to publish allocation metadata");
        }
        let free_after: u32 = self
            .block_groups
            .iter()
            .map(|group| group.bg_free_blocks_count as u32)
            .sum();
        let mut persisted = Ok(());
        let counts_changed =
            free_before != free_after || self.superblock.s_free_blocks_count != free_after;
        if counts_changed {
            self.superblock.s_free_blocks_count = free_after;
            persisted = self
                .superblock
                .write_to(self.device.as_ref())
                .map_err(|_| "Failed to persist allocation count")
                .and_then(|_| {
                    Ext2BlockGroupDesc::write_table(
                        self.device.as_ref(),
                        &self.superblock,
                        &self.block_groups,
                    )
                    .map_err(|_| "Failed to persist allocation groups")
                });
        }

        // The bitmaps already hold every block this write allocated, and the
        // inode links them even when the write failed part way (its size and
        // times change only on success). Persist those links on either
        // outcome, so a retry reuses the blocks instead of leaking them.
        let linked = inode
            .write_to(
                self.device.as_ref(),
                inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write inode");
        let changed = written.map_err(|_| "Failed to write file data")?;
        persisted?;
        linked?;

        if !changed
            && mode_before == inode.i_mode
            && times_before == (inode.i_mtime, inode.i_ctime)
            && !counts_changed
        {
            if let (Some(object), Some(epoch)) = (object, previous_epoch) {
                object.map.restore_disk_epoch(epoch);
            }
        }
        self.live_inodes.publish_size(inode_num, inode.size());
        Ok(data.len())
    }

    /// Write a modified inode back to disk
    ///
    /// # Arguments
    /// * `inode_num` - The inode number to write
    /// * `inode` - The modified inode data
    pub fn write_inode(&mut self, inode_num: u32, inode: &Ext2Inode) -> Result<(), &'static str> {
        self.ensure_writable()?;
        if let Some(object) = self.live_inodes.get(inode_num) {
            object.map.note_disk_write(self.durable_epoch());
            fifo_times::write_metadata(&object.ctime_pending, || {
                self.write_inode_disk(inode_num, inode)
            })
        } else {
            self.write_inode_disk(inode_num, inode)
        }
    }

    fn write_inode_disk(&self, inode_num: u32, inode: &Ext2Inode) -> Result<(), &'static str> {
        inode
            .write_to(
                self.device.as_ref(),
                inode_num,
                &self.superblock,
                &self.block_groups,
            )
            .map_err(|_| "Failed to write inode")
    }

    /// Create a new file in the filesystem
    ///
    /// # Arguments
    /// * `parent_inode_num` - Inode number of the parent directory
    /// * `name` - Name of the new file
    /// * `mode` - File permission bits (0o644, 0o755, etc.)
    /// * `uid`, `gid` - Owner and group of the new file
    ///
    /// # Returns
    /// * `Ok(inode_num)` - The inode number of the newly created file
    /// * `Err(msg)` - Error message if creation failed
    pub fn create_file(
        &mut self,
        parent_inode_num: u32,
        name: &str,
        mode: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u32, &'static str> {
        self.ensure_writable()?;
        // Validate name
        if name.is_empty() || name.len() > 255 {
            return Err("Invalid filename length");
        }
        if name.contains('/') || name == "." || name == ".." {
            return Err("Invalid filename");
        }

        // Read the parent directory inode
        let mut parent_inode = self.read_inode(parent_inode_num)?;
        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Read the parent directory data
        let mut dir_data = self.read_directory(&parent_inode)?;

        // Check if the file already exists
        if find_entry(&dir_data, name).is_some() {
            return Err("File already exists");
        }
        self.make_directory_room(parent_inode_num, &mut parent_inode, &mut dir_data, name.len())?;

        let mut new_inode = Ext2Inode::new_regular_file(mode, uid as u16, gid as u16);
        new_inode.set_owner(uid, gid);
        let new_inode_num = self.create_entry(
            parent_inode_num,
            &parent_inode,
            &dir_data,
            name.as_bytes(),
            new_inode,
            None,
            |_, ino| Ok(ino),
        )?;

        log::debug!("ext2: created file '{}' with inode {}", name, new_inode_num);
        Ok(new_inode_num)
    }

    pub(crate) fn dirty_handles(&self) -> Result<Vec<live_inode::FileHandle>, &'static str> {
        self.live_inodes.dirty_handles()
    }

    /// Synchronize this filesystem's synchronous data and metadata writes.
    pub fn sync(&self) -> Result<(), &'static str> {
        self.device
            .flush()
            .map_err(|_| "Failed to flush filesystem")?;
        self.live_inodes.record_durable();
        Ok(())
    }

    pub(crate) fn durable_epoch(&self) -> u64 {
        self.live_inodes.durable_epoch()
    }

    /// Bound addressing by the sector counter and LARGE_FILE feature.
    pub fn max_file_size(&self) -> u64 {
        let block_size = self.superblock.block_size() as u64;
        let fanout = block_size / 4;
        let addressable = 12 + fanout + fanout.pow(2) + fanout.pow(3);
        // Reserve room for pointer blocks and an external ACL block.
        let sector_limit = u32::MAX as u64 / (block_size / 512);
        let pointer_blocks = 3 + 2 * fanout + fanout.pow(2);
        let limit = addressable.min(sector_limit.saturating_sub(pointer_blocks + 1)) * block_size;
        if self.superblock.s_rev_level == 0 || self.superblock.s_feature_ro_compat & 0x2 == 0 {
            limit.min(i32::MAX as u64)
        } else {
            limit
        }
    }

    /// Publish the new size and queue suffix reclamation. The caller must
    /// complete the returned ShrinkTicket before reporting success.
    pub fn begin_resize_file_as(
        &mut self,
        inode_num: u32,
        length: u64,
        unprivileged: bool,
    ) -> Result<Option<ShrinkTicket>, &'static str> {
        self.ensure_writable()?;
        let custody = crate::task::thread::KillCustody::try_enter()
            .expect("ext2 resize requires its write guard custody");
        let owner = alloc::sync::Arc::new(());
        let mapped = self.live_inodes.get(inode_num);
        let before = mapped
            .as_ref()
            .map(|object| object.size.load(Ordering::Acquire));
        if let (Some(object), Some(before)) = (&mapped, before) {
            object.map.prepare_resize(before, length)?;
        }
        let result = self.resize_file_disk(inode_num, length, unprivileged).map(|progress| {
            progress.map(|progress| {
                let ticket = ShrinkTicket { inode: inode_num, mount: self.mount_pin, owner, _custody: custody };
                self.shrink_reclaims.insert(inode_num, PendingShrink {
                    progress,
                    owner: alloc::sync::Arc::downgrade(&ticket.owner),
                    failed: false,
                });
                ticket
            })
        });
        if let (Some(object), Some(before)) = (mapped, before) {
            // A shrink publishes its EOF before it can fail, so the published
            // size is followed on failure too.
            object.map.transition(object.size.load(Ordering::Acquire));
            let boundary = before.min(length);
            if result.is_err() && boundary % 4096 != 0 {
                // The tail zeroing of the boundary block may not have landed.
                object.map.invalidate(boundary, 1);
            }
            object.map.repair(self, inode_num);
        }
        result
    }

    fn resize_file_disk(
        &mut self,
        inode_num: u32,
        length: u64,
        unprivileged: bool,
    ) -> Result<Option<reclaim::Reclaim>, &'static str> {
        self.check_shrink(inode_num)?;
        let mut inode = self.read_inode(inode_num)?;
        if !inode.is_file() || length > self.max_file_size() {
            return Err("Invalid file size or type");
        }
        if unprivileged {
            let kill = 0o4000
                | if inode.i_mode & 0o0010 != 0 {
                    0o2000
                } else {
                    0
                };
            inode.i_mode &= !kill;
        }
        if length < inode.size() {
            // Publish EOF before modifying any content that used to be visible.
            let mut smaller = inode;
            smaller.i_size = length as u32;
            smaller.i_dir_acl = (length >> 32) as u32;
            smaller.update_timestamps(false, true, true);
            self.write_inode(inode_num, &smaller)?;
            self.live_inodes.publish_size(inode_num, length);
        }
        let shrinking = length < inode.size();
        file::resize_file(self.device.as_ref(), &mut inode, &self.superblock, length)
            .map_err(|_| "Failed to resize file")?;
        self.write_inode(inode_num, &inode)?;
        self.live_inodes.publish_size(inode_num, length);
        if shrinking {
            let progress = reclaim::Reclaim::shrink(
                inode,
                length.div_ceil(self.superblock.block_size() as u64),
            );
            return Ok(Some(progress));
        }
        Ok(None)
    }

    /// Relatime: update after modification/change or once a day. Reload under
    /// the filesystem write lock rather than overwriting a reader's stale inode.
    pub fn update_atime(&mut self, inode_num: u32) -> Result<(), &'static str> {
        if self.is_read_only() { return Ok(()); }
        let mut inode = self.read_inode(inode_num)?;
        let now = crate::time::current_unix_time() as u32;
        if inode.needs_atime_update() {
            inode.i_atime = now;
            // Relatime changes no mapped bytes or addressing. It must not
            // invalidate a snapshot's existing data durability certificate.
            self.write_inode_disk(inode_num, &inode)?;
        }
        Ok(())
    }

    /// Unlink (delete) a file from the filesystem
    ///
    /// This removes the directory entry and decrements the inode's link count.
    /// A zero-link inode remains allocated while external handles exist;
    /// task-context finalization reclaims it after the last observer leaves.
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
        self.unlink_in_dir(parent_inode_num, filename)
    }

    pub fn unlink_in_dir(
        &mut self,
        parent_inode_num: u32,
        filename: &str,
    ) -> Result<(), &'static str> {
        self.ensure_writable()?;
        let parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        // Read the parent directory data
        let dir_data = self.read_directory(&parent_inode)?;

        // Find the entry to verify it exists and get its inode
        let target_inode_num = dir::find_inode(&dir_data, filename).ok_or("File not found")?;

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

        // The disk changes from here on; nothing below allocates. After a
        // failed write the inode is not marked an orphan, so nothing a name
        // may still reach is freed.
        self.commit_writes(&writes)
            .map_err(|_| "Failed to write directory entry removal")?;
        if links == 0 {
            handle.object.orphan.store(true, Ordering::Release);
            // The last observer queues reclamation; writers settle pending
            // allocations under space pressure before trying new allocation.
        }
        drop(handle);

        log::debug!("ext2: unlinked {} (inode {})", filename, target_inode_num);
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
    #[cfg(feature = "ext2_lock_race")]
    pub fn create_directory(&mut self, path: &str, mode: u16) -> Result<u32, &'static str> {
        self.create_directory_owned(path, mode, 0, 0)
    }

    pub fn create_directory_owned(
        &mut self,
        path: &str,
        mode: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u32, &'static str> {
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
        self.create_directory_in(parent_inode_num, dirname, mode, uid, gid)
    }

    pub fn create_directory_in(
        &mut self,
        parent_inode_num: u32,
        dirname: &str,
        mode: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u32, &'static str> {
        self.ensure_writable()?;
        // Validate name
        if dirname.is_empty() || dirname.len() > 255 {
            return Err("Invalid directory name length");
        }
        if dirname.contains('/') || dirname == "." || dirname == ".." {
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
        if find_entry(&parent_dir_data, dirname).is_some() {
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
            dirname.as_bytes(),
            new_inode,
            Some(&entries),
            |_, ino| Ok(ino),
        )?;

        log::debug!(
            "ext2: created directory '{}' with inode {}",
            dirname,
            new_inode_num
        );
        Ok(new_inode_num)
    }

    /// Remove an empty directory from the filesystem
    ///
    /// This removes the directory if it is empty (contains only "." and "..").
    /// The entry is removed from the parent, the parent loses the link the
    /// directory's ".." gave it, and the directory, left with no link, is
    /// handed to the ext2 finalizer, which frees its blocks and its inode once
    /// and posts them to the group and superblock counts. Every write is
    /// planned, and everything allocated, before the disk changes; a failed
    /// write is undone as rename undoes one.
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

        let parent_inode_num = self.resolve_path(parent_path)?;
        self.remove_directory_in(parent_inode_num, dir_name)
    }

    pub fn remove_directory_in(
        &mut self,
        parent_inode_num: u32,
        dir_name: &str,
    ) -> Result<(), &'static str> {
        self.ensure_writable()?;
        let parent_inode = self.read_inode(parent_inode_num)?;
        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }
        let target_inode_num = self
            .lookup_in_dir(&parent_inode, dir_name)?
            .ok_or("Directory not found")?;
        if target_inode_num == EXT2_ROOT_INO || self.namespace.is_mount(target_inode_num)
            || self.namespace.mount_entry(parent_inode_num, dir_name.as_bytes()) {
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

        // The disk changes from here on; nothing below allocates. After a
        // failed write the directory is not handed to the finalizer, so
        // nothing a name may still reach is freed.
        self.commit_writes(&writes)
            .map_err(|_| "Failed to write directory removal")?;

        handle.object.orphan.store(true, Ordering::Release);
        match handle.release_sole() {
            Ok(object) => object.defer(),
            // Another observer holds it; its last close queues the orphan.
            Err(handle) => drop(handle),
        }

        log::debug!(
            "ext2: removed directory '{}' (inode {})",
            dir_name,
            target_inode_num
        );
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

        // Resolve the source path to get the inode. As on Linux, a final
        // symlink is linked itself.
        let source_inode_num = self.resolve_path_no_follow(oldpath)?;
        let (parent, name) = match newpath.rfind('/') {
            Some(0) => ("/", &newpath[1..]),
            Some(index) => (&newpath[..index], &newpath[index + 1..]),
            None => return Err("Invalid newpath"),
        };
        let parent = self.resolve_path(parent)?;
        self.link_in_dir(source_inode_num, parent, name)
    }

    pub fn link_in_dir(
        &mut self,
        source_inode_num: u32,
        new_parent_inode_num: u32,
        new_filename: &str,
    ) -> Result<(), &'static str> {
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
            || new_filename.contains('/')
            || new_filename == "."
            || new_filename == ".."
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

        if dir::find_inode(&dir_data, new_filename).is_some() {
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

        // The count rises before the name it counts is written, so a crash
        // leaves a count too high, which a check lowers, and never a name on
        // an inode whose count could reach zero while it is named.
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

        // The disk changes from here on; nothing below allocates.
        self.commit_writes(&writes)
            .map_err(|_| "Failed to write new directory entry")?;

        log::debug!(
            "ext2: created hard link {} (inode {})",
            new_filename,
            source_inode_num
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
        self.create_symlink_as(target, linkpath, 0, 0)
    }

    pub fn create_symlink_as(
        &mut self,
        target: &str,
        linkpath: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), &'static str> {
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
        // A target and its NUL must fit the one block a symlink holds (Linux
        // ext2 refuses a longer one the same way).
        if target.len() >= self.superblock.block_size() {
            return Err("Symlink target too long");
        }

        // Resolve parent directory
        let parent_inode_num = self.resolve_path(parent_path)?;
        self.create_symlink_in(target, parent_inode_num, link_name, uid, gid)
    }

    pub fn create_symlink_in(
        &mut self,
        target: &str,
        parent_inode_num: u32,
        link_name: &str,
        uid: u32,
        gid: u32,
    ) -> Result<(), &'static str> {
        self.ensure_writable()?;
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
        // A target and its NUL must fit the one block a symlink holds (Linux
        // ext2 refuses a longer one the same way).
        if target.len() >= self.superblock.block_size() {
            return Err("Symlink target too long");
        }

        let mut parent_inode = self.read_inode(parent_inode_num)?;

        if !parent_inode.is_dir() {
            return Err("Parent is not a directory");
        }

        if self.lookup_in_dir(&parent_inode, link_name)?.is_some() {
            return Err("File already exists");
        }

        // Read the parent directory before the first disk mutation, so a
        // failed read or allocation leaves no inode without an entry.
        let mut dir_data = self.read_directory(&parent_inode)?;
        self.make_directory_room(parent_inode_num, &mut parent_inode, &mut dir_data, link_name.len())?;

        // A target of more than 60 bytes is stored in a data block; a shorter
        // one in the inode itself (a fast symlink).
        let mut new_inode = Ext2Inode::new_symlink(target);
        new_inode.set_owner(uid, gid);
        let target_block = |_: u32, block: &mut [u8]| {
            block[..target.len()].copy_from_slice(target.as_bytes());
        };
        let block: Option<&dyn Fn(u32, &mut [u8])> =
            if target.len() > 60 { Some(&target_block) } else { None };
        self.create_entry(
            parent_inode_num,
            &parent_inode,
            &dir_data,
            link_name.as_bytes(),
            new_inode,
            block,
            |_, _| Ok(()),
        )?;

        log::debug!("ext2: created symlink '{}' -> '{}'", link_name, target);
        Ok(())
    }
}

// =============================================================================
// #728 — ext2 lock-discipline observability
// =============================================================================
//
// `ROOT_EXT2`/`HOME_EXT2` are `spin::RwLock`s whose contended acquisition
// busy-spins with no `try_*`-then-park fallback (spin 0.9.8's default relax
// strategy is a hardware pause, never an OS yield or park). The mechanism
// that makes a contended spin un-interruptible differs by arch, and both
// halves matter:
//   - On aarch64, every syscall dispatch that can contend these locks runs
//     with `preempt_count() > 0`, which is the same counter the timer
//     interrupt's own preemption decision gates on
//     (`can_schedule() == (preempt_count() == 0)`) — IRQs are normally
//     *unmasked* in an aarch64 syscall, so the timer still fires, it just
//     declines to switch away from the spinner.
//   - On x86, the syscall entry path (`syscall/entry.asm:29`, `cli`, no
//     `sti` before `rust_syscall_handler`) runs the entire syscall with
//     RFLAGS.IF = 0, so the timer interrupt cannot fire *at all* — a
//     stronger, more direct cause than the preempt_count mechanism (which
//     is also true on x86, but not the reason the spin cannot be preempted).
// Either way: the moment a contended acquisition's own holder is genuinely
// parked for block-device I/O completion (see
// `Completion::wait_timeout_uninterruptible`, `task/completion.rs`) while
// still holding the guard, a spinning contender is, for the duration of its
// spin, structurally exempt from the mechanism that would otherwise let the
// timer ISR preempt it and let the actual holder's completion get
// dispatched. See #728 for the full analysis.
//
// `ext2_spin_wait`/`ext2_spin_wait_write` below make that spin *observable*
// without changing its behavior: they still busy-spin forever with no yield
// and no park when contended — the same defect, just no longer silent. A
// spin that crosses `EXT2_SPIN_STALL_THRESHOLD_NS` prints one
// `EXT2_LOCK_SPIN_STALL` line (once per stall episode) and increments
// `EXT2_LOCK_SPIN_STALLS`, so a gate script can detect the livelock without
// a userspace watchdog — during the pathological case in question nothing
// userspace can run at all, so the spinner has to be its own observer (see
// `kernel::fs::ext2_lock_race` for the in-kernel repro leg that exercises
// this).

/// Wall-clock threshold (monotonic ns) past which a contended ext2 lock
/// acquisition is reported as stalled. Ordinary contention resolves in well
/// under a millisecond (either the fast `try_*` path succeeds immediately,
/// or a genuine holder releases quickly); a spin still running half a second
/// later is not "briefly contended," it is the #728 livelock shape.
const EXT2_SPIN_STALL_THRESHOLD_NS: u64 = 500_000_000;

/// Check the clock every 65536 spin iterations rather than every iteration,
/// so the observability itself never becomes the bottleneck it's measuring.
const EXT2_SPIN_POLL_MASK: u32 = 0xFFFF;

/// Running count of observed acquisition stalls (test/gate use). Read via
/// `ext2_lock_spin_stalls()`.
static EXT2_LOCK_SPIN_STALLS: AtomicU64 = AtomicU64::new(0);

/// Read the running count of #728 spin-stall episodes observed since boot.
///
/// Used by the `ext2_lock_race` repro leg and its gate script: on unfixed
/// (spin-only) lock code this reliably increments under a forced read/write
/// collision; on fixed code the same collision resolves by parking instead,
/// so the counter should stay flat.
pub fn ext2_lock_spin_stalls() -> u64 {
    EXT2_LOCK_SPIN_STALLS.load(Ordering::Relaxed)
}

#[inline]
fn ext2_now_ns() -> u64 {
    let (secs, nanos) = crate::time::get_monotonic_time_ns();
    secs as u64 * 1_000_000_000 + nanos as u64
}

/// Per-acquisition-attempt stall tracker shared by the read and write spin
/// loops below. Reports at most once per loop (`ext2_spin_wait`/
/// `ext2_spin_wait_write` each construct a fresh tracker per call).
struct Ext2SpinStallTracker {
    start_ns: u64,
    iterations: u32,
    reported: bool,
}

impl Ext2SpinStallTracker {
    fn new() -> Self {
        Self {
            start_ns: ext2_now_ns(),
            iterations: 0,
            reported: false,
        }
    }

    /// Call once per spin iteration. No-op after the first report, and cheap
    /// (no clock read) on all but 1-in-65536 iterations.
    fn tick(&mut self, lock_name: &str) {
        if self.reported {
            return;
        }
        self.iterations = self.iterations.wrapping_add(1);
        if self.iterations & EXT2_SPIN_POLL_MASK != 0 {
            return;
        }
        let elapsed = ext2_now_ns().saturating_sub(self.start_ns);
        if elapsed > EXT2_SPIN_STALL_THRESHOLD_NS {
            self.reported = true;
            EXT2_LOCK_SPIN_STALLS.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!(
                "EXT2_LOCK_SPIN_STALL lock={} elapsed_ns={}",
                lock_name,
                elapsed
            );
        }
    }
}

/// Busy-spin until `try_acquire` succeeds. Behaviorally identical to calling
/// `spin::RwLock`'s own blocking `.read()` — no yield, no park — except that
/// a spin exceeding `EXT2_SPIN_STALL_THRESHOLD_NS` is reported once so the
/// condition is gate-visible. This is the fallback path both before and
/// after the #728 fix: pre-fix it is the *only* path (this function's
/// behavior on unfixed code, unchanged from the plain spin), post-fix it is
/// reached only when parking is unsafe (`ext2_lock_can_sleep()` false) or
/// the scheduler refuses to publish the wait. A pending SIGKILL ends the spin
/// as it ends a park: nothing is owned yet, so the syscall can unwind.
fn ext2_spin_wait<T>(
    lock_name: &str,
    mut try_acquire: impl FnMut() -> Option<T>,
) -> Result<T, &'static str> {
    let mut tracker = Ext2SpinStallTracker::new();
    loop {
        if let Some(v) = try_acquire() {
            return Ok(v);
        }
        if crate::task::thread::current_kill_pending() {
            return Err(EXT2_ACQUIRE_KILLED);
        }
        tracker.tick(lock_name);
        core::hint::spin_loop();
    }
}

/// Busy-spin to acquire the write (upgraded) guard: first the upgradeable
/// slot, then the upgrade itself. Two phases because `spin::RwLock` only
/// allows one upgradeable-read guard at a time, and `try_upgrade()` returns
/// the guard back on failure (`Err(Self)`) rather than losing the slot.
/// Same behavioral contract as `ext2_spin_wait`: unchanged spin, now observed.
fn ext2_spin_wait_write(
    lock: &'static RwLock<Option<Ext2Fs>>,
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
    lock_name: &str,
) -> Result<spin::RwLockWriteGuard<'static, Option<Ext2Fs>>, &'static str> {
    let upgradeable = ext2_spin_wait(lock_name, || lock.try_upgradeable_read())?;
    ext2_spin_wait_upgrade(upgradeable, waiters, lock_name)
}

/// Phase 2 of the write spin: busy-spin `try_upgrade()` starting from an
/// already-held upgradeable guard, until every existing reader has drained.
/// Factored out so the park path (`ext2_acquire_write`, below) can fall back
/// to spinning on an upgrade it already holds the slot for, without
/// releasing and re-racing for the upgradeable slot itself. A pending SIGKILL
/// gives the slot back and wakes the contenders it was holding off.
fn ext2_spin_wait_upgrade(
    mut upgradeable: spin::RwLockUpgradableGuard<'static, Option<Ext2Fs>>,
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
    lock_name: &str,
) -> Result<spin::RwLockWriteGuard<'static, Option<Ext2Fs>>, &'static str> {
    let mut tracker = Ext2SpinStallTracker::new();
    loop {
        match upgradeable.try_upgrade() {
            Ok(write_guard) => return Ok(write_guard),
            Err(back) => upgradeable = back,
        }
        if crate::task::thread::current_kill_pending() {
            drop(upgradeable);
            waiters.wake_up();
            return Err(EXT2_ACQUIRE_KILLED);
        }
        tracker.tick(lock_name);
        core::hint::spin_loop();
    }
}

// =============================================================================
// #728 — park instead of spin when it is safe to
// =============================================================================
//
// A contended acquisition parks on a per-lock WaitQueueHead instead of
// spinning whenever `ext2_lock_can_sleep()` is true — mirroring the
// established parking-on-contention precedent in this kernel
// (`drivers/virtio/block.rs`'s `BlockRequestGate`) and the syscall sleep
// path's own can-sleep check (`task/completion.rs`'s
// `syscall_sleep_path_available`), tightened to an exact `preempt_count()` of
// 1 so `schedule_current_wait()`'s unconditional enable-then-disable pairing
// (`task/waitqueue.rs`) can neither underflow nor leave preemption wedged on
// return. When it is not safe — no current thread, interrupt context,
// interrupts masked, more than one nested preempt-disable, or (aarch64) the
// timer not yet initialized — the accessor falls back to exactly the same
// spin `ext2_spin_wait`/`ext2_spin_wait_write` performed before this commit,
// which is always correct, just not livelock-proof. This is why the
// observer commit came first: the fallback spin path is unchanged code, and
// the same instrument measures both the pre-fix state (spin, always) and
// this fix's fallback edges (spin, sometimes).
//
// Each park is a timed wait of `EXT2_LOCK_PARK_TIMEOUT_NS`, using
// `prepare_to_wait_checked`'s enqueue-under-the-waitqueue-lock recheck (never
// the untimed `prepare_to_wait`), so a missed wake degrades to a bounded
// retry rather than a permanent hang, matching this kernel's own lost-wake
// history (#584, #586, #589). A contender that may park keeps parking until
// it acquires the lock, rather than falling back to the spin after a fixed
// number of rounds. The spin holds the
// CPU with preemption disabled, and the holder it waits for can be asleep on
// block I/O with its guard held (the residual below), so a spin can only
// help a holder that is already on another CPU. Once one contender per CPU
// spins, the holder can never run again: under milestone 6's concurrent
// filesystem tests, readers woken ahead of an upgrade-waiting writer used up a
// fixed 32-round budget within seconds, and all four CPUs ended in the spin.
//
// Guards returned by the four accessors below are wrapped in
// `Ext2ReadGuard`/`Ext2WriteGuard`, which `Deref`/`DerefMut` transparently to
// `Option<Ext2Fs>` (every call site already only ever calls `.as_ref()`/
// `.as_mut()` on the guard, so no call site changes). Their `Drop` releases
// the inner `spin::RwLock` guard *first* and only then calls
// `wake_up()` — never holding ext2 state across the wake — so the lock order
// this file participates in is exactly `EXT2_STATE -> WAITQUEUE ->
// SCHEDULER`, matching `WaitQueueHead::wake_up_one`'s own
// `WAITQUEUE -> SCHEDULER` order (`task/waitqueue.rs`,
// `task/scheduler.rs`'s `with_scheduler`).
//
// Residual, disclosed rather than silently left: this fix closes the
// *contention* half of #728 (a contender no longer denies the CPU the actual
// holder's completion needs) but does not remove "ext2 guard held across a
// park" itself — `Completion::wait_timeout()`'s own documented precondition
// ("no locks are held") is still violated at every read/write-family call
// site enumerated in the #728 fix PR. Removing that pattern entirely
// (drop the guard before the block-device wait, re-validate on reacquire) is
// Option A in the #728 analysis, deliberately deferred to a follow-on: it
// touches the filesystem's core read/write/mutate paths and needs a real
// revalidation story for the in-memory allocator state
// (`Ext2Fs::block_groups`/`superblock`), which is real filesystem-correctness
// work, not a locking-primitive change.

/// Per-round park timeout. Bounded per C6 of the #728 pre-check: a missed
/// wake degrades to a retry at this granularity rather than a permanent
/// hang.
const EXT2_LOCK_PARK_TIMEOUT_NS: u64 = 200_000_000;

/// Running count of acquisition attempts that actually parked (queued on a
/// `WaitQueueHead` and called `schedule_current_wait()`), as opposed to
/// resolving on the fast `try_*` path or falling back to the spin. Read via
/// `ext2_lock_parks()`.
///
/// Exists so a green race-leg run can prove the park path was *entered*,
/// not merely that no stall was observed — the absence of a stall is also
/// what a contender that parked and then never woke up looks like before
/// its round timeout elapses, so "no stall" alone does not distinguish
/// "the fix worked" from "the fix parked and got lucky on the timeout."
/// (B4 of the #728 review.)
///
/// This is a single global counter, not per-caller: a race-leg run reads it
/// as a *delta* across its own window (see `ext2_lock_race.rs`'s
/// `run_one()`), which proves "some ext2 acquisition parked somewhere
/// during this window," not "this specific race's own contenders parked."
/// On the race-leg's dedicated boot profile nothing else touches ext2
/// concurrently, so a nonzero delta is strong corroboration in practice —
/// but it is corroboration, not per-thread proof (review round-2 finding
/// M5). Making it per-race (e.g. recording the contenders' own tids)
/// remains a real improvement, not done here.
static EXT2_LOCK_PARKS: AtomicU64 = AtomicU64::new(0);

/// Read the running count of successful park entries since boot. See
/// `EXT2_LOCK_PARKS`.
pub fn ext2_lock_parks() -> u64 {
    EXT2_LOCK_PARKS.load(Ordering::Relaxed)
}

/// #748: set once the very first successful ext2 lock park has been
/// observed since boot. Companion to `EXT2_LOCK_PARKS` above, not a
/// replacement for it — this exists purely so a gate script (or a human
/// tailing the serial log) can grep for the `EXT2_LOCK_PARK_FIRST` line
/// `ext2_record_park()` prints below and stop watching the instant it
/// appears, instead of waiting for `ext2_lock_race`'s own leg to fully
/// resolve and join. On x86 under #748's pathological in-leg pace that join
/// can take on the order of hours, even though the underlying park event
/// this line reports is expected (by the leg's own construction: a 100ms
/// head start against a 3s hold) to happen within the leg's first second of
/// real progress.
static EXT2_LOCK_PARK_FIRST_LOGGED: AtomicBool = AtomicBool::new(false);

/// Record one successful park entry (`EXT2_LOCK_PARKS`), and — the first
/// time only, across the whole kernel's boot — print a marker line proving
/// the park path was actually entered at least once. Called from both
/// `ext2_acquire` and `ext2_acquire_write`'s `Queued` arms instead of a raw
/// `EXT2_LOCK_PARKS.fetch_add` (`tests/ext2_lock_structure.rs` pins that
/// routing) so the marker can never be silently reintroduced-missing on one
/// path while present on the other.
/// #748 (`ext2_lock_race` test leg only): re-arm `EXT2_LOCK_PARK_FIRST_LOGGED`
/// so the *next* park anywhere in the kernel prints again, scoping the
/// marker to a specific window instead of "the whole boot's first-ever
/// park" — see `ext2_lock_race.rs`'s `run_one()`, which calls this
/// immediately before spawning each race attempt's holder/contender
/// kthreads. This exists as defense-in-depth against a *hypothetical*
/// pre-leg park: `ext2_lock_race`'s `boot_tests` profile runs a battery of
/// other tests before this leg's own call site, and if any of them ever
/// contends the ext2 lock incidentally, it would otherwise consume the
/// one-shot marker before the leg's own holder/contender ever run, leaving
/// the leg's own construction — the actual #728 repro shape this oracle
/// exists to prove — with no independent signal of its own. (No such
/// pre-leg park has actually been observed on x86 to date: reading
/// `kernel/src/main.rs`, the tombstone-join gate that was once suspected of
/// producing one is the literal last call in the `boot_tests` battery,
/// immediately preceding this leg's own entry point — adjacent, not
/// "earlier in the same boot" as a prior draft of this comment claimed.
/// Kept for the next test the battery grows, or a reordering, since the
/// re-arm is unconditionally correct and costs one atomic store per race
/// attempt.)
/// Test-only in practice (the only caller is behind the `ext2_lock_race`
/// feature) but left ordinarily compiled, matching `ext2_lock_parks()`'s
/// own always-available pattern.
pub fn ext2_reset_lock_park_first_marker() {
    EXT2_LOCK_PARK_FIRST_LOGGED.store(false, Ordering::Relaxed);
}

#[inline]
fn ext2_record_park(lock_name: &str) {
    let parks = EXT2_LOCK_PARKS.fetch_add(1, Ordering::Relaxed) + 1;
    if EXT2_LOCK_PARK_FIRST_LOGGED
        .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        crate::serial_println!("EXT2_LOCK_PARK_FIRST lock={} parks={}", lock_name, parks);
    }
}

/// Returns true when it is safe for a contended ext2 lock acquisition to
/// park instead of spin. See the module docs above for the full rationale;
/// this mirrors `block_request_gate_can_sleep()`
/// (`drivers/virtio/block.rs`) and `syscall_sleep_path_available()`
/// (`task/completion.rs`) — including the arch split those two precedents
/// already use: aarch64 gates on `interrupts_enabled()` (see below), x86
/// does not.
///
/// # Why x86 does *not* check `interrupts_enabled()`
///
/// Every x86 syscall enters through `INT 0x80` on an interrupt gate with an
/// explicit `cli` (`syscall/entry.asm:29`) and there is no `sti` anywhere on
/// the path down into this file — RFLAGS.IF is unconditionally 0 for the
/// entire duration of every x86 syscall, including the ~44 fs/syscall sites
/// that call the four accessors below. An `interrupts_enabled()` conjunct
/// here would make this predicate false at every one of them, permanently,
/// which reduces the fix to a no-op on the arch #728 was reported on. IF=0
/// is not a no-park condition on x86 the way DAIR.I-masked is on aarch64
/// (C3): the x86 park primitive `schedule_current_wait()` calls on every
/// loop iteration is `arch_halt_with_interrupts()` ==
/// `X86Cpu::halt_with_interrupts()` == `enable_and_hlt` — the atomic
/// `sti; hlt` sequence — so parking *from* IF=0 is exactly the normal,
/// expected entry state; the primitive itself is what turns interrupts back
/// on, at the halt, so the timer ISR can wake the thread. Every block-device
/// read already parks this same way, from this same IF=0 syscall state, via
/// `block_request_gate_can_sleep()` (x86: `preempt_count() > 0`, no IF
/// check) and `syscall_sleep_path_available()` (x86: `preempt_count() > 0`,
/// no IF check) — this predicate now matches both.
///
/// aarch64 syscalls are the opposite: `syscall_entry.S` unmasks DAIF before
/// `rust_syscall_handler` runs, so IRQs are normally *unmasked* in an
/// aarch64 syscall, and a caller that masks them anyway (C3) needs exactly
/// this check to stay a hard no-park. That is why the check is aarch64-only rather than deleted
/// outright: on aarch64 it is load-bearing; on x86 it is never anything but
/// unconditionally false, so keeping it there defeats the fix.
///
/// # x86's liveness dependency: why this predicate requires `== 1`, not `> 0`
///
/// (Review round-2 finding M2.) A parked x86 thread is `BlockedOnIO`
/// (`schedule_current_wait()`, `task/waitqueue.rs`), and
/// `per_cpu::can_schedule()` (`kernel/src/per_cpu.rs:1051`) deliberately
/// does **not** include `BlockedOnIO` in the small set of thread states
/// that bypass its `current_preempt == 0` guard — only `Blocked`,
/// `BlockedOnSignal`, `BlockedOnChildExit`, `BlockedOnTimer`, and
/// `Terminated` do (see that function's own long-form comment, and #666/
/// #508, the tracked gap this leaves for the general case). So the timer
/// ISR's `check_need_resched_and_switch` can only take a parked x86 ext2
/// thread off the CPU through `can_schedule()`'s
/// `(current_preempt == 0 && need_resched_set)` clause — the
/// exception-cleanup and returning-to-userspace/idle clauses do not apply
/// to a thread halted mid-syscall. That clause needs `preempt_count() == 0`
/// exactly, and the only reason it ever reaches 0 here is that
/// `schedule_current_wait()` calls `preempt_enable()` before its halt loop,
/// unwinding the *one* level of nesting this predicate insists on via its
/// own `== 1` (not `>= 1` or `> 0`) check below. Weakening this check to
/// `> 0` (admitting a caller with, say, two nested `preempt_disable()`
/// guards) would still enter the park loop, but `preempt_enable()`'s single
/// decrement would leave `preempt_count() == 1` on return — a park that can
/// never be scheduled away by the only clause that can reach it, silently
/// turning every round into a full `EXT2_LOCK_PARK_TIMEOUT_NS` timeout.
/// Pinned in the structural ratchet (`tests/ext2_lock_structure.rs`).
#[inline]
fn ext2_lock_can_sleep() -> bool {
    if crate::task::scheduler::current_thread_id().is_none() {
        return false;
    }

    #[cfg(target_arch = "aarch64")]
    {
        if crate::per_cpu_aarch64::in_interrupt() {
            return false;
        }
        if !crate::arch_impl::aarch64::cpu::interrupts_enabled() {
            return false;
        }
        if crate::per_cpu_aarch64::preempt_count() != 1 {
            return false;
        }
        crate::arch_impl::aarch64::timer_interrupt::is_initialized()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        if crate::per_cpu::in_interrupt() {
            return false;
        }
        crate::per_cpu::preempt_count() == 1
    }
}

fn ext2_schedule_current_wait() {
    crate::task::waitqueue::schedule_current_wait();
}

/// Try the fast (uncontended) path, then park until acquired when it's safe
/// to, otherwise fall back to the unchanged spin. A pending SIGKILL ends
/// acquisition before any mount state is owned, so the syscall can unwind.
/// Shared by the read and
/// write acquisition paths below via the closures they pass.
/// `lock_name` is used only for the `ext2_record_park()` marker below (the
/// same name each caller already passes to its own `ext2_spin_wait*` spin
/// fallback).
fn ext2_acquire<T>(
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
    lock_name: &str,
    mut try_acquire: impl FnMut() -> Option<T>,
    spin_fallback: impl FnOnce() -> Result<T, &'static str>,
) -> Result<T, &'static str> {
    if crate::task::thread::current_kill_pending() { return Err(EXT2_ACQUIRE_KILLED); }
    if let Some(v) = try_acquire() {
        return Ok(v);
    }

    if ext2_lock_can_sleep() {
        loop {
            if crate::task::thread::current_kill_pending() { return Err(EXT2_ACQUIRE_KILLED); }
            if let Some(v) = try_acquire() {
                return Ok(v);
            }

            let deadline = ext2_now_ns() + EXT2_LOCK_PARK_TIMEOUT_NS;
            // The recheck closure runs under the waitqueue lock, atomically
            // with publishing our BlockedOnIO state: a release that lands
            // between our failed `try_acquire()` above and this enqueue is
            // not lost, because either the closure now observes the lock
            // free (Mismatch — loop back and retry `try_acquire()`
            // immediately, uncontended) or it still observes it held, in
            // which case we are safely enqueued before the holder can
            // possibly release again. The closure's own successful
            // `try_acquire()` result (if any) is deliberately dropped here —
            // `prepare_to_wait_checked`'s `cond` can only report a bool, not
            // hand a value back — and reacquired uncontended by the very
            // next loop iteration's `try_acquire()` above.
            let outcome = waiters.prepare_to_wait_checked(
                crate::task::thread::ThreadState::BlockedOnIO,
                Some(deadline),
                || try_acquire().is_none(),
            );

            match outcome {
                crate::task::waitqueue::PrepareOutcome::Mismatch => continue,
                crate::task::waitqueue::PrepareOutcome::PublishFailed
                | crate::task::waitqueue::PrepareOutcome::OutOfMemory => break,
                crate::task::waitqueue::PrepareOutcome::Queued => {
                    ext2_record_park(lock_name);
                    ext2_schedule_current_wait();
                    waiters.finish_wait();
                }
            }
        }
    }

    spin_fallback()
}

pub(crate) const EXT2_ACQUIRE_KILLED: &str = "ext2 acquisition interrupted by SIGKILL";

/// How long a thread a kill has claimed parks between checks of its claim.
const EXT2_KILL_CLAIM_RECHECK_NS: u64 = 1_000_000;

/// Open the calling thread's kill custody before it acquires an ext2 mount
/// lock (#1025). A thread a kill has already claimed must not acquire the
/// lock at all: its termination discards its kernel continuation, guard
/// included, and the lock would stay held. On SMP the claim can land while
/// the thread runs on another CPU, so it waits here, parked on the mount's
/// queue, until either the termination takes it off the CPU for good or the
/// claim is withdrawn because the kill was deferred.
fn ext2_kill_custody(
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
) -> crate::task::thread::KillCustody {
    loop {
        if let Some(custody) = crate::task::thread::KillCustody::try_enter() {
            return custody;
        }
        if !ext2_lock_can_sleep() {
            core::hint::spin_loop();
            continue;
        }
        let outcome = waiters.prepare_to_wait_checked(
            crate::task::thread::ThreadState::BlockedOnIO,
            Some(ext2_now_ns() + EXT2_KILL_CLAIM_RECHECK_NS),
            || true,
        );
        if let crate::task::waitqueue::PrepareOutcome::Queued = outcome {
            ext2_schedule_current_wait();
            waiters.finish_wait();
        }
    }
}

/// Write-acquisition variant of `ext2_acquire`: acquires the upgradeable
/// slot via `ext2_acquire` (park-capable, generic), then parks waiting for
/// the upgrade itself while *continuing to hold* the upgradeable guard
/// across every retry round — never releasing and re-racing for the
/// upgradeable slot between rounds. This is what preserves the writer
/// fairness `root_fs_write()`'s doc comment promises (the UPGRADED bit keeps
/// blocking new readers for the whole wait, not just between our park
/// rounds) — C8 of the #728 pre-check.
fn ext2_acquire_write(
    lock: &'static RwLock<Option<Ext2Fs>>,
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
    lock_name: &str,
) -> Result<spin::RwLockWriteGuard<'static, Option<Ext2Fs>>, &'static str> {
    let mut upgradeable = ext2_acquire(
        waiters,
        lock_name,
        || lock.try_upgradeable_read(),
        || ext2_spin_wait(lock_name, || lock.try_upgradeable_read()),
    )?;

    // Fast path: no readers in the way at all.
    upgradeable = match upgradeable.try_upgrade() {
        Ok(w) => return Ok(w),
        Err(back) => back,
    };

    if !ext2_lock_can_sleep() {
        return ext2_spin_wait_upgrade(upgradeable, waiters, lock_name);
    }

    loop {
        if crate::task::thread::current_kill_pending() {
            drop(upgradeable);
            waiters.wake_up();
            return Err(EXT2_ACQUIRE_KILLED);
        }
        // `slot` lets the `FnOnce` recheck closure below hand the
        // upgradeable guard back out (its own signature can only return
        // `bool`) via a captured-by-reference `Option`, so we never drop it
        // and re-race for the upgradeable position between rounds.
        let mut slot = Some(upgradeable);
        let mut upgraded: Option<spin::RwLockWriteGuard<'static, Option<Ext2Fs>>> = None;
        let deadline = ext2_now_ns() + EXT2_LOCK_PARK_TIMEOUT_NS;
        let outcome = waiters.prepare_to_wait_checked(
            crate::task::thread::ThreadState::BlockedOnIO,
            Some(deadline),
            || {
                let held = slot.take().expect("slot populated at recheck");
                match held.try_upgrade() {
                    Ok(w) => {
                        upgraded = Some(w);
                        false // Mismatch: got it, don't enqueue.
                    }
                    Err(back) => {
                        slot = Some(back);
                        true // readers still present: enqueue and block.
                    }
                }
            },
        );

        if let Some(w) = upgraded {
            return Ok(w);
        }
        upgradeable = slot
            .take()
            .expect("slot repopulated by cond on Mismatch/Queued");

        match outcome {
            crate::task::waitqueue::PrepareOutcome::Mismatch => continue,
            crate::task::waitqueue::PrepareOutcome::PublishFailed
            | crate::task::waitqueue::PrepareOutcome::OutOfMemory => {
                return ext2_spin_wait_upgrade(upgradeable, waiters, lock_name);
            }
            crate::task::waitqueue::PrepareOutcome::Queued => {
                ext2_record_park(lock_name);
                ext2_schedule_current_wait();
                waiters.finish_wait();
            }
        }
    }
}

/// Wraps a `spin::RwLockReadGuard` on `ROOT_EXT2`/`HOME_EXT2`. Transparent
/// `Deref` to `Option<Ext2Fs>` — every call site only ever calls `.as_ref()`
/// on the guard it gets back, so this needs no call-site changes. Releases
/// the inner guard before waking parked contenders (see module docs above).
pub struct Ext2ReadGuard {
    inner: Option<spin::RwLockReadGuard<'static, Option<Ext2Fs>>>,
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
    /// Opened before the acquisition and closed after `Drop` releases the
    /// lock, so a SIGKILL never abandons a holder or a queued waiter (#1025).
    _custody: crate::task::thread::KillCustody,
}

impl core::ops::Deref for Ext2ReadGuard {
    type Target = Option<Ext2Fs>;
    fn deref(&self) -> &Option<Ext2Fs> {
        self.inner.as_ref().expect("Ext2ReadGuard used after drop")
    }
}

impl Drop for Ext2ReadGuard {
    fn drop(&mut self) {
        self.inner = None;
        // A dropped *read* guard only ever unblocks the single upgradeable
        // holder waiting on `try_upgrade()` (releasing a reader never
        // admits another reader -- spin's RwLock never blocks reader vs.
        // reader), so at most one waiter can actually make progress from
        // this event: wake_up_one() (never queue every waiter for a
        // thundering herd), not the full broadcast `wake_up()`.
        //
        // Correction (review round-2 finding M1, re-found in the closure
        // round): an earlier version of this comment additionally claimed
        // `has_waiters()` "skips the lock entirely in the common
        // uncontended case." That was false -- `has_waiters()` calls
        // `with_waiters()` (`task/waitqueue.rs`), the *same*
        // `arch_without_interrupts`-guarded `spin::Mutex` `wake_up_one()`
        // itself takes. Gating on it saved nothing on the uncontended path
        // (one lock acquisition either way) and cost an extra one on the
        // contended path (two acquisitions instead of one), so it has been
        // removed; `wake_up_one()` is now called unconditionally.
        //
        // Residual, disclosed rather than fixed: `wake_up_one()` pops the
        // waitqueue's *front* entry, which may be a parked reader rather
        // than the upgrade-waiting writer that could actually proceed --
        // that reader retries `try_read()`, is refused while UPGRADED is
        // set, and re-parks, deferring the writer's own wake to its next
        // bounded per-round timeout (C6, `EXT2_LOCK_PARK_TIMEOUT_NS`). This
        // is bounded added latency, not a hang, and it existed before this
        // correction too (`has_waiters()` never changed which waiter gets popped) --
        // recorded here since it was under-stated originally. The queue is
        // shared rather than writer-priority (module docs above); making it
        // writer-priority is a real option for a future round, not done
        // here.
        self.waiters.wake_up_one();
    }
}

/// Wraps a `spin::RwLockWriteGuard` on `ROOT_EXT2`/`HOME_EXT2`. Transparent
/// `Deref`/`DerefMut` to `Option<Ext2Fs>` — every call site only ever calls
/// `.as_mut()` on the guard it gets back, so this needs no call-site
/// changes. Releases the inner guard before waking parked contenders (see
/// module docs above).
pub struct Ext2WriteGuard {
    inner: Option<spin::RwLockWriteGuard<'static, Option<Ext2Fs>>>,
    waiters: &'static crate::task::waitqueue::WaitQueueHead,
    /// Opened before the acquisition and closed after `Drop` releases the
    /// lock, so a SIGKILL never abandons a holder or a queued waiter (#1025).
    _custody: crate::task::thread::KillCustody,
}

impl Ext2WriteGuard {
    /// Finish this syscall's suffix, keeping errors with their producer.
    /// Readers may run between batches; other mutators wait for the ticket.
    pub fn finish_shrink(mut self, ticket: ShrinkTicket) -> Result<Self, &'static str> {
        let home = core::ptr::eq(self.waiters, &HOME_EXT2_WAITERS);
        loop {
            let fs = self.as_mut().ok_or("Missing ext2 mount")?;
            ticket.mount.verify(fs)?;
            let mut pending = fs.shrink_reclaims.remove(&ticket.inode).ok_or("Missing ext2 shrink")?;
            assert!(pending.owner.ptr_eq(&alloc::sync::Arc::downgrade(&ticket.owner)));
            let mut budget = RECLAIM_BUDGET;
            let result = pending.progress.finish(fs, ticket.inode, &mut budget);
            if matches!(result, Ok(true)) { return Ok(self); }
            pending.failed |= result.is_err();
            fs.shrink_reclaims.insert(ticket.inode, pending);
            if result.is_err() { return Err("Failed to reclaim resized suffix"); }
            if crate::task::thread::current_kill_pending() { return Err(EXT2_ACQUIRE_KILLED); }
            drop(self);
            writeback::pause(1_000_000);
            self = fs_write_raw(home)?;
        }
    }

    /// Preserve writer serialization while allowing unrelated readers through.
    /// A killed caller's healthy work finishes in the service before a new
    /// mutator enters. Failed suffixes retain their per-inode retry policy.
    fn wait_for_shrink_writer(mut self) -> Result<Self, &'static str> {
        let _wait_custody = crate::task::thread::KillCustody::try_enter()
            .expect("ext2 writer wait requires its write guard custody");
        let home = core::ptr::eq(self.waiters, &HOME_EXT2_WAITERS);
        let pin = self.as_ref().map(|fs| fs.mount_pin);
        loop {
            if let Some(fs) = self.as_ref() {
                if let Some(pin) = pin { pin.verify(fs)?; }
                if !fs.shrink_reclaims.values().any(|pending| pending.owner.strong_count() != 0 || !pending.failed) {
                    return Ok(self);
                }
            } else if pin.is_none() {
                return Ok(self);
            } else {
                return Err("ext2 mount changed during shrink wait");
            }
            drop(self);
            writeback::pause(1_000_000);
            self = fs_write_raw(home)?;
        }
    }
}

impl core::ops::Deref for Ext2WriteGuard {
    type Target = Option<Ext2Fs>;
    fn deref(&self) -> &Option<Ext2Fs> {
        self.inner.as_ref().expect("Ext2WriteGuard used after drop")
    }
}

impl core::ops::DerefMut for Ext2WriteGuard {
    fn deref_mut(&mut self) -> &mut Option<Ext2Fs> {
        self.inner.as_mut().expect("Ext2WriteGuard used after drop")
    }
}

impl Drop for Ext2WriteGuard {
    fn drop(&mut self) {
        self.inner = None;
        // Unlike a read-guard drop, releasing the write/upgradeable slot
        // can unblock several distinct waiter kinds at once (every queued
        // try_read(), plus the next try_upgradeable_read()), so this side
        // keeps the full broadcast wake_up().
        //
        // A `has_waiters()`-gated fast path was tried here and removed
        // (see `Ext2ReadGuard::drop`'s comment for the full correction):
        // `has_waiters()` takes the same `WaitQueueHead` lock `wake_up()`
        // does, so gating on it does not skip a lock acquisition on the
        // uncontended path and adds a second one on the contended path.
        self.waiters.wake_up();
    }
}

/// Wait queue for contended `ROOT_EXT2` acquisitions (both read and write —
/// a shared queue is deliberate: every release wakes every waiter, which is
/// always correct for an RwLock, even if occasionally spurious for a waiter
/// whose specific condition a given release didn't satisfy, and the bounded
/// per-round timeout is the backstop regardless).
static ROOT_EXT2_WAITERS: crate::task::waitqueue::WaitQueueHead =
    crate::task::waitqueue::WaitQueueHead::new();

/// Wait queue for contended `HOME_EXT2` acquisitions. See `ROOT_EXT2_WAITERS`.
static HOME_EXT2_WAITERS: crate::task::waitqueue::WaitQueueHead =
    crate::task::waitqueue::WaitQueueHead::new();

/// Global mounted ext2 root filesystem
///
/// Uses RwLock to allow concurrent read access (exec, file reads, getdents)
/// while exclusive write access is needed only for mutations (create, truncate,
/// rename, link, unlink, write). This prevents spinlock contention under slow
/// I/O where a writer holding the lock blocks all readers.
static ROOT_EXT2: RwLock<Option<Ext2Fs>> = RwLock::new(None);

/// VFS mount IDs of the installed root and home filesystems, published under
/// their install guards, so a handle's mount is chosen before any guard is
/// taken. `usize::MAX` means not installed.
static ROOT_MOUNT_ID: AtomicUsize = AtomicUsize::new(usize::MAX);
static HOME_MOUNT_ID: AtomicUsize = AtomicUsize::new(usize::MAX);

fn pin_is_home(pin: live_inode::MountPin) -> Result<bool, &'static str> {
    if pin.mount_id == ROOT_MOUNT_ID.load(Ordering::Acquire) {
        Ok(false)
    } else if pin.mount_id == HOME_MOUNT_ID.load(Ordering::Acquire) {
        Ok(true)
    } else {
        Err("Unknown ext2 mount")
    }
}

/// Route only to an installed mount whose VFS ID and lifetime token match.
/// Only the selected guard is taken, and it stays held for the caller.
pub fn read_mount(pin: live_inode::MountPin) -> Result<Ext2ReadGuard, &'static str> {
    let guard = if pin_is_home(pin)? {
        home_fs_read()
    } else {
        root_fs_read()
    }?;
    pin.verify(guard.as_ref().ok_or("ext2 mount unavailable")?)?;
    Ok(guard)
}

pub fn write_mount(pin: live_inode::MountPin) -> Result<Ext2WriteGuard, &'static str> {
    let guard = if pin_is_home(pin)? {
        home_fs_write()
    } else {
        root_fs_write()
    }?;
    pin.verify(guard.as_ref().ok_or("ext2 mount unavailable")?)?;
    Ok(guard)
}

/// Initialize the root ext2 filesystem
///
/// Mounts the ext2 disk as the root filesystem.
/// x86 selects a mountable ext2 filesystem from 512-byte-sector block devices.
/// ARM64 prefers VirtIO, then scans AHCI disks for an ext2 superblock.
///
/// This should be called during kernel initialization after block
/// device driver initialization.
pub fn init_root_fs() -> Result<(), &'static str> {
    // Try VirtIO block devices first (works on both x86_64 and QEMU ARM64)
    #[cfg(target_arch = "aarch64")]
    let device: alloc::boxed::Box<dyn BlockDevice> = {
        use crate::block::virtio::VirtioBlockWrapper;
        if let Some(dev) = VirtioBlockWrapper::new(2).or_else(|| VirtioBlockWrapper::new(0)) {
            #[cfg(target_arch = "aarch64")]
            crate::serial_println!(
                "[ext2] Using VirtIO block device ({} sectors)",
                dev.num_blocks()
            );
            alloc::boxed::Box::new(dev)
        } else {
            // Fall back to AHCI block devices (Parallels ARM64).
            // Try each SATA device looking for one with a valid ext2 superblock.
            // On Parallels, sata:0 is typically the FAT32 EFI boot disk.
            {
                crate::serial_println!("[ext2] No VirtIO block device, trying AHCI...");
                let count = crate::drivers::ahci::sata_device_count();
                let mut found: Option<crate::drivers::ahci::AhciBlockDevice> = None;
                for i in 0..count {
                    if let Some(dev) = crate::drivers::ahci::get_block_device_by_index(i) {
                        crate::serial_println!(
                            "[ext2] AHCI device {}: {} sectors ({} MB)",
                            i,
                            dev.num_blocks(),
                            dev.num_blocks() * 512 / (1024 * 1024)
                        );
                        // Try reading ext2 superblock (at byte offset 1024, sector 2)
                        let mut buf = [0u8; 512];
                        if dev.read_block(2, &mut buf).is_ok() {
                            let magic = (buf[56] as u16) | ((buf[57] as u16) << 8);
                            if magic == 0xEF53 {
                                crate::serial_println!(
                                    "[ext2] Found ext2 superblock on AHCI device {}",
                                    i
                                );
                                found = Some(dev);
                                break;
                            } else {
                                crate::serial_println!(
                                    "[ext2] AHCI device {}: not ext2 (magic={:#06x})",
                                    i,
                                    magic
                                );
                            }
                        } else {
                            crate::serial_println!("[ext2] AHCI device {}: read failed", i);
                        }
                    }
                }
                let ahci_dev = found.ok_or(
                    "No block device with ext2 filesystem (tried VirtIO and all AHCI devices)",
                )?;
                alloc::boxed::Box::new(ahci_dev)
            }
        }
    };

    // Register with VFS mount system
    let mount_id = crate::fs::vfs::mount("/", "ext2");

    // Create the ext2 filesystem instance
    #[cfg(target_arch = "aarch64")]
    let fs = Ext2Fs::new(device, mount_id)?;
    #[cfg(target_arch = "x86_64")]
    let fs = crate::block::root_devices()
        .into_iter()
        .find_map(|device| {
            if device.block_size() != 512 {
                return None;
            }
            let mut superblock = [0u8; 512];
            if device.read_block(2, &mut superblock).is_err()
                || u16::from_le_bytes([superblock[56], superblock[57]]) != 0xEF53
            {
                return None;
            }
            match Ext2Fs::new(device, mount_id) {
                Ok(fs) => Some(fs),
                Err(error) => {
                    log::warn!("ext2: Root candidate mount failed: {}", error);
                    None
                }
            }
        })
        .ok_or("No mountable 512-byte-sector ext2 block device")?;

    #[cfg(target_arch = "x86_64")]
    let fs = {
        let mut fs = fs;
        if crate::block::root_on_system_partition() {
            if let Some(id) = crate::boot::intent::userland() {
                fs.namespace = generation::select(&fs, id)?;
            }
        }
        ROOT_NAMESPACE.call_once(|| fs.namespace);
        fs
    };

    crate::drivers::device_class::storage_read();

    // Read packed struct fields safely before logging
    let blocks_count =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(fs.superblock.s_blocks_count)) };
    let inodes_count =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(fs.superblock.s_inodes_count)) };
    log::info!(
        "ext2: Mounted root filesystem - {} blocks, {} inodes, block size {}",
        blocks_count,
        inodes_count,
        fs.superblock.block_size()
    );

    // Store globally
    let mut installed = ROOT_EXT2.write();
    if installed
        .as_ref()
        .is_some_and(|old| old.live_inodes.is_pinned() || !old.shrink_reclaims.is_empty())
    {
        return Err("Root ext2 mount is pinned");
    }
    ROOT_MOUNT_ID.store(fs.mount_id, Ordering::Release);
    *installed = Some(fs);

    Ok(())
}

/// Access the root ext2 filesystem for read-only operations
///
/// Multiple readers can hold this lock concurrently, allowing parallel
/// exec, file reads, getdents, and stat operations without contention.
pub fn root_fs_read() -> Result<Ext2ReadGuard, &'static str> {
    let custody = ext2_kill_custody(&ROOT_EXT2_WAITERS);
    let inner = ext2_acquire(
        &ROOT_EXT2_WAITERS,
        "ROOT_EXT2_read",
        || ROOT_EXT2.try_read(),
        || ext2_spin_wait("ROOT_EXT2_read", || ROOT_EXT2.try_read()),
    )?;
    Ok(Ext2ReadGuard {
        inner: Some(inner),
        waiters: &ROOT_EXT2_WAITERS,
        _custody: custody,
    })
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
pub fn root_fs_write() -> Result<Ext2WriteGuard, &'static str> {
    fs_write(false)
}

/// Acquire exclusive access without performing another inode's reclamation.
pub(super) fn fs_write_raw(home: bool) -> Result<Ext2WriteGuard, &'static str> {
    let (lock, waiters, name) = if home {
        (&HOME_EXT2, &HOME_EXT2_WAITERS, "HOME_EXT2_write")
    } else {
        (&ROOT_EXT2, &ROOT_EXT2_WAITERS, "ROOT_EXT2_write")
    };
    let custody = ext2_kill_custody(waiters);
    let inner = ext2_acquire_write(lock, waiters, name)?;
    Ok(Ext2WriteGuard {
        inner: Some(inner),
        waiters,
        _custody: custody,
    })
}

pub(crate) fn fs_sync_guard(home: bool) -> Result<Ext2WriteGuard, &'static str> {
    fs_write_raw(home)?.wait_for_shrink_writer()
}

fn fs_write(home: bool) -> Result<Ext2WriteGuard, &'static str> {
    let guard = fs_write_raw(home)?;
    if let Some(fs) = guard.as_ref() { fs.ensure_writable()?; }
    let guard = guard.wait_for_shrink_writer()?;
    let pressure = guard.as_ref().is_some_and(|fs| {
        let stats = fs.statfs();
        stats.free_blocks <= (stats.blocks / 16).max(128)
            || stats.free_inodes <= 32
    });
    if pressure {
        drop(guard);
        // Last-observer reclamation must not turn rm followed by allocation
        // into ENOSPC solely because the background service has not run yet.
        if let Ok(guard) = accounting_fs_write(home) {
            return guard.wait_for_shrink_writer();
        }
        // Failed I/O keeps allocations quarantined; normal operations report
        // their own errors rather than exposing those allocations for reuse.
        fs_write_raw(home)?.wait_for_shrink_writer()
    } else {
        Ok(guard)
    }
}

/// Settle the orphan cohort present at an allocation-counter observation.
/// Later removals do not extend this operation. Each pass keeps the normal
/// bitmap-transition budget, and pauses happen without the mount guard.
/// The returned guard excludes reclamation while counters and their disk
/// counterparts are read. I/O errors retain custody and reach the reader.
pub(crate) fn accounting_fs_write(home: bool) -> Result<Ext2WriteGuard, &'static str> {
    let mut guard = fs_write_raw(home)?;
    let pin = guard.as_ref().map(|fs| fs.mount_pin);
    let objects = guard
        .as_ref()
        .map(|fs| fs.live_inodes.pending_snapshot())
        .transpose()?
        .unwrap_or_default();
    let mut cohort_at = 0;
    loop {
        let cohort_end = (cohort_at + 32).min(objects.len());
        let result = if let Some(fs) = guard.as_mut() {
            if let Some(pin) = pin {
                pin.verify(fs)?;
            }
            let mut budget = RECLAIM_BUDGET;
            fs.finalize_orphans(&objects[cohort_at..cohort_end], &mut budget)
        } else {
            if pin.is_some() {
                return Err("ext2 mount changed during accounting");
            }
            Finalize::Idle
        };
        match result {
            Finalize::Idle if cohort_end == objects.len() => return Ok(guard),
            Finalize::Idle => {
                cohort_at = cohort_end;
                drop(guard);
                writeback::pause(1_000_000);
                guard = fs_write_raw(home)?;
            }
            Finalize::Retry => {
                writeback::request();
                return Err("Failed to settle ext2 allocation counters");
            }
            Finalize::More => {
                drop(guard);
                writeback::pause(1_000_000);
                guard = fs_write_raw(home)?;
            }
        }
    }
}

/// Persist relatime without making a reader finish unrelated suffix work.
/// This changes only the inode's timestamp under the same exclusive guard;
/// reclaim reloads the inode before each detachment batch.
pub(crate) fn update_read_atime(inode: u32, home: bool) -> Result<(), &'static str> {
    {
        let guard = if home { home_fs_read()? } else { root_fs_read()? };
        if guard.as_ref().ok_or("Missing ext2 mount")?.is_read_only() { return Ok(()); }
    }
    let mut guard = fs_write_raw(home)?;
    guard.as_mut().ok_or("Missing ext2 mount")?.update_atime(inode)
}

/// Check if the root filesystem is mounted
///
/// Read from the published mount ID, not through `root_fs_read()`: it takes
/// no guard, so it can neither spin behind a writer (the #728 shape) nor
/// report "not mounted" to a caller whose acquisition a SIGKILL ended.
pub fn is_mounted() -> bool {
    ROOT_MOUNT_ID.load(Ordering::Acquire) != usize::MAX
}

/// Global mounted ext2 home filesystem (/home)
///
/// Separate from ROOT_EXT2 so that user data lives on a different disk
/// from the system binaries. This allows syncing the system disk to
/// other machines without overwriting their local user data.
static HOME_EXT2: RwLock<Option<Ext2Fs>> = RwLock::new(None);

/// Initialize the home ext2 filesystem
///
/// Mounts a second ext2 disk as the /home filesystem.
/// Noninteractive testing kernels use an embedded fixture when no disk is attached.
/// Device layout:
///   - x86_64: Device 3 (boot=0, test=1, root=2, home=3)
///   - ARM64: Device 1 (root=0, home=1)
///
/// This is non-fatal — if no home disk is attached, /home falls through
/// to the root ext2 filesystem (backward compatible).
pub fn init_home_fs() -> Result<(), &'static str> {
    // Prefer the attached home device so VM acceptance boots exercise the
    // transport's read-only feature. Physical disks remain protected by the
    // hardware write-refusal policy rather than selected by index here.
    // On a physical boot the home is the partition the boot intent names, when it names one.
    #[cfg(target_arch = "x86_64")]
    let device = if crate::block::hardware_read_only() {
        crate::block::named_home()
    } else {
        crate::block::disk(3).or_else(|| crate::block::disk(1))
    };

    #[cfg(target_arch = "aarch64")]
    let device = {
        use crate::block::virtio::VirtioBlockWrapper;
        VirtioBlockWrapper::new(3)
            .or_else(|| VirtioBlockWrapper::new(1))
            .map(|dev| alloc::boxed::Box::new(dev) as alloc::boxed::Box<dyn BlockDevice>)
    };

    // Interactive includes testing, but must retain its writable home disk.
    #[cfg(all(feature = "testing", not(feature = "interactive")))]
    let device = device.or_else(|| {
        log::info!("ext2: using embedded read-only home fixture");
        Some(alloc::boxed::Box::new(crate::block::testing_fixture::ReadOnlyFixture)
            as alloc::boxed::Box<dyn BlockDevice>)
    });
    let device = device.ok_or("No home block device available")?;

    // Register with VFS mount system
    let mount_id = crate::fs::vfs::mount("/home", "ext2");

    // Create the ext2 filesystem instance
    let fs = Ext2Fs::new(device, mount_id)?;

    // Read packed struct fields safely before logging
    let blocks_count =
        unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(fs.superblock.s_blocks_count)) };
    log::info!(
        "ext2: Mounted home filesystem at /home - {} blocks, block size {}",
        blocks_count,
        fs.superblock.block_size()
    );

    // Store globally
    let mut installed = HOME_EXT2.write();
    if installed
        .as_ref()
        .is_some_and(|old| old.live_inodes.is_pinned() || !old.shrink_reclaims.is_empty())
    {
        return Err("Home ext2 mount is pinned");
    }
    HOME_MOUNT_ID.store(fs.mount_id, Ordering::Release);
    *installed = Some(fs);

    Ok(())
}

/// Access the home ext2 filesystem for read-only operations
pub fn home_fs_read() -> Result<Ext2ReadGuard, &'static str> {
    let custody = ext2_kill_custody(&HOME_EXT2_WAITERS);
    let inner = ext2_acquire(
        &HOME_EXT2_WAITERS,
        "HOME_EXT2_read",
        || HOME_EXT2.try_read(),
        || ext2_spin_wait("HOME_EXT2_read", || HOME_EXT2.try_read()),
    )?;
    Ok(Ext2ReadGuard {
        inner: Some(inner),
        waiters: &HOME_EXT2_WAITERS,
        _custody: custody,
    })
}

/// Access the home ext2 filesystem for write operations
///
/// Uses upgradeable_read() + upgrade() to prevent writer starvation (same as root_fs_write).
pub fn home_fs_write() -> Result<Ext2WriteGuard, &'static str> {
    fs_write(true)
}

/// Check if the home filesystem is mounted. Lock-free, as `is_mounted()`.
pub fn is_home_mounted() -> bool {
    home_mount_id().is_some()
}

/// Get the mount_id of the home filesystem, if mounted.
///
/// Used by FD-based syscall dispatch to determine which filesystem a file
/// descriptor belongs to, ahead of the guard the syscall then takes. Read
/// from the published mount ID, as `is_mounted()`.
pub fn home_mount_id() -> Option<usize> {
    let id = HOME_MOUNT_ID.load(Ordering::Acquire);
    (id != usize::MAX).then_some(id)
}

/// Strip the /home prefix from a path, returning the path within the home filesystem.
///
/// "/home/foo.bmp" → "/foo.bmp"
/// "/home" → "/"
/// "/home/" → "/"
pub fn strip_home_prefix(path: &str) -> &str {
    if path == "/home" || path == "/home/" {
        "/"
    } else if path.starts_with("/home/") {
        // rest is "foo.bmp" — we need to return "/foo.bmp"
        // Since we can't allocate, we return the slice starting at the '/' before the rest
        // path = "/home/foo.bmp"
        //         01234567...
        // We want &path[5..] = "/foo.bmp"
        &path[5..]
    } else {
        path
    }
}

/// Check if a resolved path should be routed to the home filesystem.
pub fn is_home_path(path: &str) -> bool {
    (path == "/home" || path.starts_with("/home/")) && is_home_mounted()
}

/// File-mapping cache pages became unbound: have the ext2 service retire them.
/// Safe from binding drops, which can run under the process manager.
pub(crate) fn request_map_eviction() {
    writeback::request();
}
