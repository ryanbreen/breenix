use x86_64::structures::paging::{OffsetPageTable, PageTable};
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

/// Get a new mapper instance for manual page table operations
/// 
/// # Safety
/// Caller must ensure that the complete physical memory is mapped to virtual memory
/// at the provided `physical_memory_offset`.
pub unsafe fn get_mapper(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}