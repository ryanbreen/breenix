//! Resident ext2 file pages used by file-backed mmap on both architectures.
//!
//! The registry holds weak references: VMAs own page lifetimes, independently
//! of descriptors. A page also pins its frame in the allocator's leaf ledger,
//! so private CoW faults cannot turn the cached file frame writable in place.
//! Registry locks never span filesystem I/O or process-manager operations.

use alloc::collections::BTreeMap;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::Mutex;

#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::PhysFrame;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::PhysFrame;

use super::frame_allocator::{
    acquire_leaf_mapping, allocate_frame, deallocate_frame, deallocate_leaf_frame,
};
use super::frame_metadata::frame_decref;
use crate::fs::ext2;
use crate::syscall::errno::{EIO, ENOMEM};

const PAGE_SIZE: usize = 4096;
type Key = (usize, u64, u64);
static PAGES: Mutex<BTreeMap<Key, Weak<FilePage>>> = Mutex::new(BTreeMap::new());

#[derive(Debug)]
pub struct FilePage {
    mount_id: usize,
    inode_num: u64,
    offset: u64,
    pub frame: PhysFrame,
}

impl FilePage {
    fn ptr(&self) -> *mut u8 {
        (super::physical_memory_offset().as_u64() + self.frame.start_address().as_u64()) as *mut u8
    }

    /// Snapshot through the direct map, including writes made by any mapping.
    /// Volatile accesses avoid constructing aliased Rust references to memory
    /// userspace can modify, and leave later concurrent writes resident.
    pub fn writeback(&self) -> Result<(), u64> {
        let mut data = Vec::new();
        data.try_reserve_exact(PAGE_SIZE)
            .map_err(|_| ENOMEM as u64)?;
        for i in 0..PAGE_SIZE {
            data.push(unsafe { self.ptr().add(i).read_volatile() });
        }
        let write = |fs: &mut ext2::Ext2Fs| -> Result<(), u64> {
            let inode = fs
                .read_inode(self.inode_num as u32)
                .map_err(|_| EIO as u64)?;
            // The trailing bytes of the last page must not extend the file.
            let len = inode
                .size()
                .saturating_sub(self.offset)
                .min(PAGE_SIZE as u64) as usize;
            if len != 0 {
                let existing = fs
                    .read_file_range(&inode, self.offset, len)
                    .map_err(|_| EIO as u64)?;
                if existing == data[..len] {
                    return Ok(());
                }
                let written = fs
                    .write_file_range_uncached(self.inode_num as u32, self.offset, &data[..len])
                    .map_err(|_| EIO as u64)?;
                if written != len {
                    return Err(EIO as u64);
                }
            }
            Ok(())
        };
        if ext2::home_mount_id() == Some(self.mount_id) {
            write(ext2::home_fs_write().as_mut().ok_or(EIO as u64)?)
        } else {
            write(ext2::root_fs_write().as_mut().ok_or(EIO as u64)?)
        }
    }
}

impl Drop for FilePage {
    fn drop(&mut self) {
        if frame_decref(self.frame) {
            deallocate_leaf_frame(self.frame);
        }
    }
}

/// Load and publish while the filesystem read guard still excludes file writes.
/// Two concurrent loaders choose the same published page; the loser drops its
/// unused frame. Only the short registry lookup/publication holds its mutex.
pub fn get_page(mount_id: usize, inode_num: u64, offset: u64) -> Result<Arc<FilePage>, u64> {
    let key = (mount_id, inode_num, offset);
    if let Some(page) = PAGES.lock().get(&key).and_then(Weak::upgrade) {
        return Ok(page);
    }
    let load = |fs: &ext2::Ext2Fs| -> Result<Arc<FilePage>, u64> {
        let inode = fs.read_inode(inode_num as u32).map_err(|_| EIO as u64)?;
        let data = fs
            .read_file_range(&inode, offset, PAGE_SIZE)
            .map_err(|_| EIO as u64)?;
        let frame = allocate_frame().ok_or(ENOMEM as u64)?;
        if acquire_leaf_mapping(frame).is_err() {
            deallocate_frame(frame);
            return Err(ENOMEM as u64);
        }
        let page = Arc::new(FilePage {
            mount_id,
            inode_num,
            offset,
            frame,
        });
        unsafe {
            core::ptr::write_bytes(page.ptr(), 0, PAGE_SIZE);
            core::ptr::copy_nonoverlapping(data.as_ptr(), page.ptr(), data.len());
        }
        let mut pages = PAGES.lock();
        if let Some(existing) = pages.get(&key).and_then(Weak::upgrade) {
            return Ok(existing);
        }
        pages.retain(|_, page| page.strong_count() != 0);
        pages.insert(key, Arc::downgrade(&page));
        Ok(page)
    };
    if ext2::home_mount_id() == Some(mount_id) {
        load(ext2::home_fs_read().as_ref().ok_or(EIO as u64)?)
    } else {
        load(ext2::root_fs_read().as_ref().ok_or(EIO as u64)?)
    }
}

/// Keep ordinary ext2 reads/writes coherent with resident mapped pages. The
/// caller holds the filesystem guard, establishing fs -> registry lock order.
/// Writeback uses the uncached write so a snapshot never overwrites subsequent
/// stores to a live mapping.
pub fn read_resident(mount_id: usize, inode_num: u64, offset: u64, data: &mut [u8]) {
    transfer_resident(
        mount_id,
        inode_num,
        offset,
        data.len(),
        |page, page_offset, data_offset, len| {
            for i in 0..len {
                data[data_offset + i] = unsafe { page.ptr().add(page_offset + i).read_volatile() };
            }
        },
    );
}

pub fn write_resident(mount_id: usize, inode_num: u64, offset: u64, data: &[u8]) {
    transfer_resident(
        mount_id,
        inode_num,
        offset,
        data.len(),
        |page, page_offset, data_offset, len| {
            for i in 0..len {
                unsafe {
                    page.ptr()
                        .add(page_offset + i)
                        .write_volatile(data[data_offset + i]);
                }
            }
        },
    );
}

fn transfer_resident(
    mount_id: usize,
    inode_num: u64,
    offset: u64,
    len: usize,
    mut transfer: impl FnMut(&FilePage, usize, usize, usize),
) {
    let mut done = 0;
    while done < len {
        let position = offset + done as u64;
        let page_offset = (position % PAGE_SIZE as u64) as usize;
        let count = (PAGE_SIZE - page_offset).min(len - done);
        let key = (mount_id, inode_num, position - page_offset as u64);
        // Hold an Arc across the transfer, but never the registry lock.
        let page = PAGES.lock().get(&key).and_then(Weak::upgrade);
        if let Some(page) = page {
            transfer(&page, page_offset, done, count);
        }
        done += count;
    }
}
