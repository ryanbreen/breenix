use x86_64::structures::paging::{
    Mapper, Page, PageTableFlags, Size4KiB,
    OffsetPageTable, 
};
use x86_64::VirtAddr;

/// Base address for user stack allocation area
/// Using a high userspace address area to avoid conflicts with heap
pub const USER_STACK_ALLOC_START: u64 = 0x_5555_5555_0000;

/// Base address for kernel stack allocation area
/// Must be in kernel space (high canonical addresses)
pub const KERNEL_STACK_ALLOC_START: u64 = 0xFFFF_C900_0000_0000;

/// Stack with guard page protection
pub struct GuardedStack {
    /// Start of the allocated region (including guard page)
    allocation_start: VirtAddr,
    /// Top of the usable stack (highest address)
    stack_top: VirtAddr,
    /// Size of the usable stack area (excluding guard page)
    stack_size: usize,
}

impl GuardedStack {
    /// Create a new guarded stack
    /// 
    /// # Arguments
    /// * `stack_size` - Size of the usable stack in bytes (must be page-aligned)
    /// * `mapper` - Page table mapper for allocating pages
    /// 
    /// # Returns
    /// A new GuardedStack with a guard page at the bottom
    pub fn new(stack_size: usize, mapper: &mut OffsetPageTable) -> Result<Self, &'static str> {
        // Ensure stack size is page-aligned
        if stack_size % 4096 != 0 {
            return Err("Stack size must be page-aligned");
        }
        
        // Calculate total allocation size (guard page + stack)
        let total_pages = (stack_size / 4096) + 1; // +1 for guard page
        let total_size = total_pages * 4096;
        
        // Find available virtual address space
        let allocation_start = Self::find_free_virtual_space(total_size)?;
        
        
        // Map the stack pages (excluding guard page)
        let stack_start = allocation_start + 4096u64; // Skip guard page
        let stack_top = stack_start + stack_size as u64;
        
        Self::map_stack_pages(stack_start, stack_size, mapper)?;
        
        // Guard page is intentionally left unmapped at allocation_start
        
        Ok(GuardedStack {
            allocation_start,
            stack_top,
            stack_size,
        })
    }
    
    /// Get the top of the stack (highest usable address)
    pub fn top(&self) -> VirtAddr {
        self.stack_top
    }
    
    /// Get the bottom of the stack (lowest usable address, just above guard page)
    pub fn bottom(&self) -> VirtAddr {
        self.allocation_start + 4096u64
    }
    
    /// Get the guard page address
    pub fn guard_page(&self) -> VirtAddr {
        self.allocation_start
    }
    
    /// Check if an address is within the guard page
    pub fn is_guard_page_access(&self, addr: VirtAddr) -> bool {
        let guard_start = self.allocation_start.as_u64();
        let guard_end = guard_start + 4096;
        let access_addr = addr.as_u64();
        
        access_addr >= guard_start && access_addr < guard_end
    }
    
    /// Check if an address is within the stack region
    pub fn contains(&self, addr: VirtAddr) -> bool {
        let stack_start = self.bottom().as_u64();
        let stack_end = self.stack_top.as_u64();
        let access_addr = addr.as_u64();
        
        access_addr >= stack_start && access_addr < stack_end
    }
    
    /// Get the size of the usable stack area
    pub fn size(&self) -> usize {
        self.stack_size
    }
    
    /// Find free virtual address space for stack allocation
    fn find_free_virtual_space(size: usize) -> Result<VirtAddr, &'static str> {
        // For now, use a simple incrementing allocator
        // TODO: Implement proper virtual memory management
        static mut NEXT_USER_STACK_ADDR: u64 = USER_STACK_ALLOC_START;
        
        unsafe {
            // Allocate from user stack area for user processes
            let addr = VirtAddr::new(NEXT_USER_STACK_ADDR);
            NEXT_USER_STACK_ADDR += size as u64;
            
            // Simple bounds check for user stacks
            if NEXT_USER_STACK_ADDR > 0x_7FFF_FFFF_0000 {
                return Err("Out of virtual address space for user stacks");
            }
            
            Ok(addr)
        }
    }
    
    /// Map stack pages with appropriate permissions
    fn map_stack_pages(
        start: VirtAddr, 
        size: usize, 
        mapper: &mut OffsetPageTable
    ) -> Result<(), &'static str> {
        let start_page = Page::<Size4KiB>::containing_address(start);
        let end_page = Page::<Size4KiB>::containing_address(start + size as u64 - 1u64);
        
        // Use user flags for user stacks
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        
        for page in Page::range_inclusive(start_page, end_page) {
            let frame = crate::memory::frame_allocator::allocate_frame()
                .ok_or("out of memory")?;
            
            unsafe {
                let mut frame_allocator = crate::memory::frame_allocator::GlobalFrameAllocator;
                mapper.map_to(page, frame, flags, &mut frame_allocator)
                    .map_err(|_| "failed to map stack page")?
                    .flush();
            }
        }
        
        Ok(())
    }
}

impl Drop for GuardedStack {
    fn drop(&mut self) {
        // TODO: Implement proper cleanup (unmap pages, deallocate frames)
        log::debug!("GuardedStack dropped (cleanup not yet implemented)");
    }
}

/// Global stack registry to track allocated stacks
static mut STACK_REGISTRY: Option<alloc::vec::Vec<GuardedStack>> = None;

/// Initialize the stack allocation system
pub fn init() {
    unsafe {
        STACK_REGISTRY = Some(alloc::vec::Vec::new());
    }
    log::info!("Stack allocation system initialized");
}

/// Allocate a new guarded stack
pub fn allocate_stack(size: usize) -> Result<GuardedStack, &'static str> {
    let mut mapper = unsafe { crate::memory::paging::get_mapper() };
    GuardedStack::new(size, &mut mapper)
}

/// Check if a page fault is due to guard page access
pub fn is_guard_page_fault(fault_addr: VirtAddr) -> Option<&'static GuardedStack> {
    unsafe {
        if let Some(ref stacks) = STACK_REGISTRY {
            for stack in stacks {
                if stack.is_guard_page_access(fault_addr) {
                    return Some(stack);
                }
            }
        }
    }
    None
}