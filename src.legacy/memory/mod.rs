use bootloader::bootinfo::MemoryMap;
use bootloader::bootinfo::MemoryRegionType;
use bootloader::BootInfo;

use x86_64::{
    addr::PhysAddr,
    structures::paging::{
        frame::PhysFrame, mapper::MapToError, FrameAllocator, Mapper, OffsetPageTable, Page,
        PageTable, PageTableFlags, Size4KiB,
    },
    VirtAddr,
};

use conquer_once::spin::OnceCell;

use spin::Mutex;

use crate::println;

pub mod allocator;

/// A FrameAllocator that returns usable frames from the bootloader's memory map.
pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
}

impl BootInfoFrameAllocator {
    /// Create a FrameAllocator from the passed memory map.
    ///
    /// This function is unsafe because the caller must guarantee that the passed
    /// memory map is valid. The main requirement is that all frames that are marked
    /// as `USABLE` in it are really unused.
    pub unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        BootInfoFrameAllocator {
            memory_map,
            next: 0,
        }
    }

    /// Returns an iterator over the usable frames specified in the memory map.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        // get usable regions from memory map
        let regions = self.memory_map.iter();
        let usable_regions = regions.filter(|r| r.region_type == MemoryRegionType::Usable);
        // map each region to its address range
        let addr_ranges = usable_regions.map(|r| r.range.start_addr()..r.range.end_addr());
        // transform to an iterator of frame start addresses
        let frame_addresses = addr_ranges.flat_map(|r| r.step_by(4096));
        // create `PhysFrame` types from the start addresses
        frame_addresses.map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

/// Initialize a new OffsetPageTable.
///
/// This function is unsafe because the caller must guarantee that the
/// complete physical memory is mapped to virtual memory at the passed
/// `physical_memory_offset`. Also, this function must be only called once
/// to avoid aliasing `&mut` references (which is undefined behavior).
unsafe fn init_page_table(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

/// Returns a mutable reference to the active level 4 table.
///
/// This function is unsafe because the caller must guarantee that the
/// complete physical memory is mapped to virtual memory at the passed
/// `physical_memory_offset`. Also, this function must be only called once
/// to avoid aliasing `&mut` references (which is undefined behavior).
pub unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr // unsafe
}

pub unsafe fn map_to(
    page: Page,
    frame: PhysFrame,
    flags: PageTableFlags,
) -> Result<(), MapToError<Size4KiB>> {
    let mut map = MEMORY_MAPPER
        .try_get()
        .expect("memory mapper not initialized")
        .lock();

    let mut frame_allocator_guard = FRAME_ALLOCATOR
        .try_get()
        .expect("frame allocation not initialized!")
        .lock();

    let mut do_map = || -> Result<(), MapToError<Size4KiB>> {
        let frame_allocator = &mut *frame_allocator_guard;
        Ok(map.map_to(page, frame, flags, frame_allocator)?.flush())
    };

    do_map()
}

pub unsafe fn identity_map(
    frame: PhysFrame,
    flags: PageTableFlags,
) -> Result<(), MapToError<Size4KiB>> {
    let mut map = MEMORY_MAPPER
        .try_get()
        .expect("memory mapper not initialized")
        .lock();

    let mut frame_allocator_guard = FRAME_ALLOCATOR
        .try_get()
        .expect("frame allocation not initialized!")
        .lock();

    let mut do_map = || -> Result<(), MapToError<Size4KiB>> {
        let frame_allocator = &mut *frame_allocator_guard;
        Ok(map.identity_map(frame, flags, frame_allocator)?.flush())
    };

    do_map()
}

pub fn identity_map_range(
    addr: u64,
    len: u64,
    flags: PageTableFlags,
) -> Result<(), MapToError<Size4KiB>> {
    let range = PhysFrame::range_inclusive(
        PhysFrame::containing_address(PhysAddr::new(addr)),
        PhysFrame::containing_address(PhysAddr::new(addr + len)),
    );

    println!("Identity map range is {:?}", range);
    for frame in range {
        unsafe { identity_map(frame, flags)? };
    }

    Ok(())
}

pub fn allocate_frame() -> Option<PhysFrame<Size4KiB>> {
    let mut frame_allocator_guard = FRAME_ALLOCATOR
        .try_get()
        .expect("frame allocation not initialized!")
        .lock();
    let mut do_allocate = || -> Option<PhysFrame> {
        let frame_allocator = &mut frame_allocator_guard;
        frame_allocator.allocate_frame()
    };

    do_allocate()
}

static MEMORY_MAPPER: OnceCell<Mutex<OffsetPageTable<'static>>> = OnceCell::uninit();
static FRAME_ALLOCATOR: OnceCell<Locked<BootInfoFrameAllocator>> = OnceCell::uninit();

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

pub fn init(boot_info: &'static BootInfo) {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);

    let mapper = unsafe { init_page_table(phys_mem_offset) };

    let frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };

    MEMORY_MAPPER
        .try_init_once(|| Mutex::new(mapper))
        .expect("MemoryMapper should only be called once!");

    FRAME_ALLOCATOR
        .try_init_once(|| Locked::new(frame_allocator))
        .expect("Frame allocator should only be initted once!");

    allocator::init_heap().expect("heap initialization failed");
}
