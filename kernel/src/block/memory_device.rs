//! Sparse, boot-local storage for the volatile BreenFS volume.
use super::{BlockDevice, BlockError};
use alloc::vec::Vec;
use core::{
    ops::Range,
    sync::atomic::{AtomicUsize, Ordering},
};
use spin::Mutex;

#[cfg(all(not(test), target_arch = "aarch64"))]
use crate::memory::arch_stub::PhysFrame;
#[cfg(all(not(test), target_arch = "x86_64"))]
use x86_64::structures::paging::PhysFrame;

const SECTOR_SIZE: usize = 512;
const PAGE_SIZE: usize = 4096;
const BYTES: usize = 64 * 1024 * 1024;
static NEXT_ID: AtomicUsize = AtomicUsize::new(0x4d52_0000_0000_0000);

pub(crate) struct MemoryDevice {
    id: usize,
    pages: Mutex<Vec<Option<Page>>>,
}

impl MemoryDevice {
    pub(crate) fn new() -> Result<Self, BlockError> {
        let mut pages = Vec::new();
        pages
            .try_reserve_exact(BYTES / PAGE_SIZE)
            .map_err(|_| BlockError::OutOfMemory)?;
        pages.resize_with(BYTES / PAGE_SIZE, || None);
        Ok(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            pages: Mutex::new(pages),
        })
    }

    fn range(start: u64, count: usize, len: usize) -> Result<Range<usize>, BlockError> {
        let start = usize::try_from(start)
            .ok()
            .and_then(|n| n.checked_mul(SECTOR_SIZE))
            .ok_or(BlockError::OutOfBounds)?;
        let bytes = count
            .checked_mul(SECTOR_SIZE)
            .ok_or(BlockError::OutOfBounds)?;
        let end = start
            .checked_add(bytes)
            .filter(|&end| end <= BYTES)
            .ok_or(BlockError::OutOfBounds)?;
        if bytes > len {
            return Err(BlockError::IoError);
        }
        Ok(start..end)
    }

    fn with_page<T>(&self, index: usize, f: impl FnOnce(&mut Option<Page>) -> T) -> T {
        // The device can be used from preemptible threads. Keep local
        // scheduling out of the lock for a single page copy.
        crate::arch_without_interrupts(|| f(&mut self.pages.lock()[index]))
    }
}

impl BlockDevice for MemoryDevice {
    fn identity(&self) -> usize {
        self.id
    }

    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn num_blocks(&self) -> u64 {
        (BYTES / SECTOR_SIZE) as u64
    }

    fn read_block(&self, sector: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        self.read_blocks(sector, 1, buf)
    }

    fn write_block(&self, sector: u64, buf: &[u8]) -> Result<(), BlockError> {
        self.write_blocks(sector, 1, buf)
    }

    fn read_blocks(&self, start: u64, count: usize, buf: &mut [u8]) -> Result<(), BlockError> {
        let range = Self::range(start, count, buf.len())?;
        let mut out = &mut buf[..range.len()];
        let mut at = range.start;
        while !out.is_empty() {
            let offset = at % PAGE_SIZE;
            let len = out.len().min(PAGE_SIZE - offset);
            let (chunk, rest) = out.split_at_mut(len);
            self.with_page(at / PAGE_SIZE, |page| match page {
                Some(page) => chunk.copy_from_slice(&page.bytes()[offset..offset + len]),
                None => chunk.fill(0),
            });
            out = rest;
            at += len;
        }
        Ok(())
    }

    fn write_blocks(&self, start: u64, count: usize, buf: &[u8]) -> Result<(), BlockError> {
        let range = Self::range(start, count, buf.len())?;
        let mut input = &buf[..range.len()];
        let mut at = range.start;
        while !input.is_empty() {
            let offset = at % PAGE_SIZE;
            let len = input.len().min(PAGE_SIZE - offset);
            let (chunk, rest) = input.split_at(len);
            let index = at / PAGE_SIZE;
            let write = |page: &mut Page| {
                page.bytes_mut()[offset..offset + len].copy_from_slice(chunk);
            };
            if self
                .with_page(index, |page| page.as_mut().map(write))
                .is_none()
            {
                // Allocate outside the lock and recheck in case another writer
                // installed the page. The losing frame is returned outside it.
                let mut fresh = Some(Page::new()?);
                self.with_page(index, |page| {
                    write(page.get_or_insert_with(|| fresh.take().unwrap()));
                });
            }
            input = rest;
            at += len;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), BlockError> {
        // Each write reaches its backing frames before returning.
        Ok(())
    }
}

#[cfg(not(test))]
struct Page(PhysFrame);

#[cfg(not(test))]
impl Page {
    fn new() -> Result<Self, BlockError> {
        use crate::memory::frame_allocator::{allocate_frame_for, FrameOwner};
        let mut page =
            Self(allocate_frame_for(FrameOwner::Buffers).ok_or(BlockError::OutOfMemory)?);
        page.bytes_mut().fill(0);
        Ok(page)
    }

    fn ptr(&self) -> *mut [u8; PAGE_SIZE] {
        (crate::memory::physical_memory_offset().as_u64() + self.0.start_address().as_u64())
            as *mut [u8; PAGE_SIZE]
    }

    fn bytes(&self) -> &[u8; PAGE_SIZE] {
        // SAFETY: this page owns the frame, and access is protected by the device lock.
        unsafe { &*self.ptr() }
    }

    fn bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        // SAFETY: this page solely owns the frame and this borrow is exclusive.
        unsafe { &mut *self.ptr() }
    }
}

#[cfg(not(test))]
impl Drop for Page {
    fn drop(&mut self) {
        crate::memory::frame_allocator::deallocate_frame(self.0);
    }
}

#[cfg(test)]
struct Page(alloc::boxed::Box<[u8; PAGE_SIZE]>);

#[cfg(test)]
impl Page {
    fn new() -> Result<Self, BlockError> {
        alloc::boxed::Box::try_new([0; PAGE_SIZE])
            .map(Self)
            .map_err(|_| BlockError::OutOfMemory)
    }

    fn bytes(&self) -> &[u8; PAGE_SIZE] {
        &self.0
    }

    fn bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        &mut self.0
    }
}
