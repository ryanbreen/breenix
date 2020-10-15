
pub mod bump;

use alloc::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;

use crate::constants::memory::{HEAP_START,HEAP_SIZE};

use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
        frame::PhysFrame, frame::PhysFrameRange
    },
    VirtAddr, addr::PhysAddr
};

use bump::BumpAllocator;

#[global_allocator]
static ALLOCATOR: Locked<BumpAllocator> = Locked::new(BumpAllocator::new());

/// A wrapper around spin::Mutex to permit trait implementations.
pub struct Locked<A> {
    inner: spin::Mutex<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: spin::Mutex::new(inner),
        }
    }

    pub fn lock(&self) -> spin::MutexGuard<A> {
        self.inner.lock()
    }
}

/// Align the given address `addr` upwards to alignment `align`.
///
/// Requires that `align` is a power of two.
fn align_up(addr: usize, align: usize) -> usize {
    (addr + align - 1) & !(align - 1)
}

pub fn init_heap(
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE - 1u64;
        let heap_start_page:Page = Page::containing_address(heap_start);
        let heap_end_page:Page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            crate::memory::map_to(page, frame, flags, frame_allocator)
        };
    }

    let ioreg = PhysFrame::range_inclusive(
        PhysFrame::containing_address(PhysAddr::new(0xfebc0000)),
        PhysFrame::containing_address(PhysAddr::new(0xfebc0000 + 0x2000))
    );
    crate::println!("Range is {:?}", ioreg);
    for frame in ioreg {
        //let frame = frame_allocator
        //    .allocate_frame()
        //    .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        crate::println!("Looking to map {:?}", frame);
        unsafe {
            crate::memory::identity_map(frame, flags, frame_allocator)
        };
    }

    unsafe {
        ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
    }

    Ok(())
}