use x86_64::structures::paging::{
    OffsetPageTable, PageTable, PhysFrame, Mapper, Page, PageTableFlags, Size4KiB,
};
use x86_64::VirtAddr;
use conquer_once::spin::OnceCell;
use spin::Mutex;

/// The global page table mapper
static PAGE_TABLE_MAPPER: OnceCell<Mutex<OffsetPageTable<'static>>> = OnceCell::uninit();

/// Initialize paging with the given physical memory offset
///
/// # Safety
/// Caller must ensure that the complete physical memory is mapped to virtual memory
/// at the provided `physical_memory_offset`.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    let mapper = OffsetPageTable::new(level_4_table, physical_memory_offset);
    
    // Store a copy in the global static
    PAGE_TABLE_MAPPER.init_once(|| {
        let level_4_table = active_level_4_table(physical_memory_offset);
        Mutex::new(OffsetPageTable::new(level_4_table, physical_memory_offset))
    });
    
    log::info!("Page table initialized");
    mapper
}

/// Returns a mutable reference to the active level 4 page table
///
/// # Safety
/// Caller must ensure that the complete physical memory is mapped to virtual memory
/// at the provided `physical_memory_offset`.
unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;
    
    let (level_4_table_frame, _) = Cr3::read();
    
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    
    &mut *page_table_ptr
}

/// Map a virtual page to a physical frame
#[allow(dead_code)]
pub fn map_page(
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    let mapper = PAGE_TABLE_MAPPER
        .get()
        .ok_or("page table mapper not initialized")?;
    
    let mut frame_allocator = crate::memory::frame_allocator::GlobalFrameAllocator;
    
    unsafe {
        mapper
            .lock()
            .map_to(page, frame, flags, &mut frame_allocator)
            .map_err(|_| "failed to map page")?
            .flush();
    }
    
    Ok(())
}

/// Identity map a physical frame (map it to the same virtual address)
#[allow(dead_code)]
pub fn identity_map(
    frame: PhysFrame<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), &'static str> {
    let page = Page::containing_address(VirtAddr::new(frame.start_address().as_u64()));
    map_page(page, frame, flags)
}