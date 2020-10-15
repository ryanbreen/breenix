

use bootloader::BootInfo;
use bootloader::bootinfo::MemoryMap;
use bootloader::bootinfo::MemoryRegionType;

use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB,
        frame::PhysFrame, frame::PhysFrameRange, OffsetPageTable, PageTable,
    },
    VirtAddr, addr::PhysAddr
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
        let usable_regions = regions
            .filter(|r| r.region_type == MemoryRegionType::Usable);
        // map each region to its address range
        let addr_ranges = usable_regions
            .map(|r| r.range.start_addr()..r.range.end_addr());
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
pub unsafe fn active_level_4_table(physical_memory_offset: VirtAddr)
    -> &'static mut PageTable
{
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr // unsafe
}

pub unsafe fn map_to(page: Page, frame: PhysFrame, flags: PageTableFlags, frame_allocator: &mut impl FrameAllocator<Size4KiB>) ->
    Result<(), MapToError<Size4KiB>> {
    let mut map = MEMORY_MAPPER
            .try_get()
            .expect("memory mapper not initialized").lock();
    map.map_to(page, frame, flags, frame_allocator)?.flush();
    Ok(())
}

pub unsafe fn identity_map(frame: PhysFrame, flags: PageTableFlags, frame_allocator: &mut impl FrameAllocator<Size4KiB>) ->
    Result<(), MapToError<Size4KiB>> {
    let mut map = MEMORY_MAPPER
            .try_get()
            .expect("memory mapper not initialized").lock();
    map.identity_map(frame, flags, frame_allocator)?.flush();
    Ok(())
}

static MEMORY_MAPPER: OnceCell<Mutex<OffsetPageTable<'static>>> = OnceCell::uninit();

pub struct MemoryMapper {
    _private: (),
}

impl MemoryMapper {
    pub fn new(boot_info: &'static BootInfo) -> Self {
        let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);

        let mapper = unsafe { init_page_table(phys_mem_offset) };

        MEMORY_MAPPER.try_init_once(|| Mutex::new(mapper))
            .expect("MemoryMapper should only be called once!");
        
        MemoryMapper { _private: () }
    }
}

pub fn init(boot_info: &'static BootInfo) {

    let mapper = MemoryMapper::new(&boot_info);

    println!("{:?}", &boot_info.memory_map);

    let mut frame_allocator = unsafe {
        BootInfoFrameAllocator::init(&boot_info.memory_map)
    };

    allocator::init_heap(&mut frame_allocator)
        .expect("heap initialization failed");
}
