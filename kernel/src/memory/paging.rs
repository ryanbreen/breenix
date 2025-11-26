use crate::task::thread::ThreadPrivilege;
use conquer_once::spin::OnceCell;
use spin::Mutex;
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::VirtAddr;

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

/// Get the global mapper instance
///
/// # Safety
/// Caller must ensure that init() has been called first.
pub unsafe fn get_mapper() -> OffsetPageTable<'static> {
    let physical_memory_offset = crate::memory::physical_memory_offset();
    get_mapper_with_offset(physical_memory_offset)
}

/// Get a new mapper instance for manual page table operations
///
/// # Safety
/// Caller must ensure that the complete physical memory is mapped to virtual memory
/// at the provided `physical_memory_offset`.
pub unsafe fn get_mapper_with_offset(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

/// Base address for the kernel/user split
/// Addresses >= this value are kernel-only
#[allow(dead_code)]
pub const KERNEL_BASE: u64 = 0xFFFF_8000_0000_0000;

/// Check if an address is in kernel space
#[allow(dead_code)]
pub fn is_kernel_address(addr: VirtAddr) -> bool {
    addr.as_u64() >= KERNEL_BASE
}

/// Get appropriate page flags based on privilege level
#[allow(dead_code)]
pub fn get_page_flags(privilege: ThreadPrivilege, writable: bool) -> PageTableFlags {
    let mut flags = PageTableFlags::PRESENT;

    if writable {
        flags |= PageTableFlags::WRITABLE;
    }

    match privilege {
        ThreadPrivilege::User => flags | PageTableFlags::USER_ACCESSIBLE,
        ThreadPrivilege::Kernel => flags,
    }
}

/// Map a single page with appropriate permissions
///
/// # Safety
/// Caller must ensure the mapper is valid and the frame is not already mapped
#[allow(dead_code)]
pub unsafe fn map_page(
    mapper: &mut OffsetPageTable,
    page: Page<Size4KiB>,
    frame: PhysFrame<Size4KiB>,
    privilege: ThreadPrivilege,
    writable: bool,
) -> Result<(), &'static str> {
    let flags = get_page_flags(privilege, writable);

    mapper
        .map_to(
            page,
            frame,
            flags,
            &mut crate::memory::frame_allocator::GlobalFrameAllocator,
        )
        .map_err(|_| "Failed to map page")?
        .flush();

    Ok(())
}

/// Enable global pages support (CR4.PGE)
/// 
/// This allows the CPU to keep kernel pages in the TLB across CR3 changes,
/// significantly improving performance during context switches.
/// 
/// # Safety
/// Should be called after kernel page tables are set up but before userspace processes start.
pub unsafe fn enable_global_pages() {
    use x86_64::registers::control::{Cr4, Cr4Flags};
    
    // Read current CR4 value
    let mut cr4 = Cr4::read();
    
    // Check if PGE is already enabled
    if cr4.contains(Cr4Flags::PAGE_GLOBAL) {
        log::info!("CR4.PGE already enabled");
        return;
    }
    
    // Enable the PGE bit
    cr4 |= Cr4Flags::PAGE_GLOBAL;
    Cr4::write(cr4);
    
    log::info!("PHASE2: Enabled global pages support (CR4.PGE)");
}
