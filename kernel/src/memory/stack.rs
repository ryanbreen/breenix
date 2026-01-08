use crate::memory::layout::{USER_STACK_REGION_START, USER_STACK_REGION_END};
use crate::task::thread::ThreadPrivilege;
use x86_64::structures::paging::{Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

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
    /// Privilege level of the stack
    #[allow(dead_code)]
    privilege: ThreadPrivilege,
}

impl GuardedStack {
    /// Create a new guarded stack
    ///
    /// # Arguments
    /// * `stack_size` - Size of the usable stack in bytes (must be page-aligned)
    /// * `mapper` - Page table mapper for allocating pages
    /// * `privilege` - Privilege level for the stack (kernel or user)
    ///
    /// # Returns
    /// A new GuardedStack with a guard page at the bottom
    pub fn new(
        stack_size: usize,
        mapper: &mut OffsetPageTable,
        privilege: ThreadPrivilege,
    ) -> Result<Self, &'static str> {
        // Ensure stack size is page-aligned
        if stack_size % 4096 != 0 {
            return Err("Stack size must be page-aligned");
        }

        // Calculate total allocation size (guard page + stack)
        let total_pages = (stack_size / 4096) + 1; // +1 for guard page
        let total_size = total_pages * 4096;

        // Find available virtual address space based on privilege
        let allocation_start = Self::find_free_virtual_space(total_size, privilege)?;

        log::debug!(
            "Allocating guarded stack at {:#x}, size {} KiB",
            allocation_start.as_u64(),
            total_size / 1024
        );

        // Map the stack pages (excluding guard page)
        let stack_start = allocation_start + 4096u64; // Skip guard page
        let stack_top = stack_start + stack_size as u64;

        Self::map_stack_pages(stack_start, stack_size, mapper, privilege)?;

        // Guard page is intentionally left unmapped at allocation_start
        log::debug!("Guard page at {:#x} (unmapped)", allocation_start.as_u64());
        log::debug!(
            "Stack region: {:#x} - {:#x} ({} KiB)",
            stack_start.as_u64(),
            stack_top.as_u64(),
            stack_size / 1024
        );

        Ok(GuardedStack {
            allocation_start,
            stack_top,
            stack_size,
            privilege,
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
    fn find_free_virtual_space(
        size: usize,
        privilege: ThreadPrivilege,
    ) -> Result<VirtAddr, &'static str> {
        // For now, use a simple incrementing allocator
        // TODO: Implement proper virtual memory management
        static mut NEXT_USER_STACK_ADDR: u64 = USER_STACK_REGION_START;
        static mut NEXT_KERNEL_STACK_ADDR: u64 = KERNEL_STACK_ALLOC_START;

        unsafe {
            match privilege {
                ThreadPrivilege::User => {
                    // Check bounds BEFORE allocating to prevent overflow
                    // USER_STACK_REGION_END is the canonical boundary (0x8000_0000_0000)
                    // We need to ensure the entire stack fits STRICTLY BELOW the boundary
                    // because 0x8000_0000_0000 is non-canonical
                    let proposed_end = NEXT_USER_STACK_ADDR.saturating_add(size as u64);
                    if proposed_end >= USER_STACK_REGION_END || proposed_end < NEXT_USER_STACK_ADDR {
                        return Err("Out of virtual address space for user stacks");
                    }

                    let addr = VirtAddr::new(NEXT_USER_STACK_ADDR);
                    NEXT_USER_STACK_ADDR = proposed_end;
                    Ok(addr)
                }
                ThreadPrivilege::Kernel => {
                    // Check bounds BEFORE allocating
                    let proposed_end = NEXT_KERNEL_STACK_ADDR.saturating_add(size as u64);
                    if proposed_end >= 0xFFFF_CA00_0000_0000 || proposed_end < NEXT_KERNEL_STACK_ADDR {
                        return Err("Out of virtual address space for kernel stacks");
                    }

                    let addr = VirtAddr::new(NEXT_KERNEL_STACK_ADDR);
                    NEXT_KERNEL_STACK_ADDR = proposed_end;
                    Ok(addr)
                }
            }
        }
    }

    /// Map stack pages with appropriate permissions
    fn map_stack_pages(
        start: VirtAddr,
        size: usize,
        mapper: &mut OffsetPageTable,
        privilege: ThreadPrivilege,
    ) -> Result<(), &'static str> {
        let start_page = Page::<Size4KiB>::containing_address(start);
        let end_page = Page::<Size4KiB>::containing_address(start + size as u64 - 1u64);

        log::trace!(
            "map_stack_pages: start_page={:#x}, end_page={:#x}",
            start_page.start_address(),
            end_page.start_address()
        );

        let flags = match privilege {
            ThreadPrivilege::Kernel => PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
            ThreadPrivilege::User => {
                PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE
            }
        };

        log::trace!("map_stack_pages: About to iterate over page range");
        for page in Page::range_inclusive(start_page, end_page) {
            log::trace!("map_stack_pages: Got page from iterator, about to log address");
            log::trace!("map_stack_pages: Mapping page {:#x}", page.start_address());

            log::trace!("map_stack_pages: About to call allocate_frame()");
            let frame = crate::memory::frame_allocator::allocate_frame().ok_or("out of memory")?;
            log::trace!("map_stack_pages: allocate_frame() returned successfully");

            log::trace!(
                "map_stack_pages: Allocated frame {:#x} for page {:#x}",
                frame.start_address(),
                page.start_address()
            );

            unsafe {
                let mut frame_allocator = crate::memory::frame_allocator::GlobalFrameAllocator;
                log::trace!("map_stack_pages: About to call mapper.map_to...");

                mapper
                    .map_to(page, frame, flags, &mut frame_allocator)
                    .map_err(|_| "failed to map stack page")?
                    .flush();

                log::trace!(
                    "map_stack_pages: Successfully mapped page {:#x}",
                    page.start_address()
                );
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

/// Allocate a new guarded stack with default kernel privilege
pub fn allocate_stack(size: usize) -> Result<GuardedStack, &'static str> {
    allocate_stack_with_privilege(size, ThreadPrivilege::Kernel)
}

/// Allocate a new guarded stack with specified privilege
pub fn allocate_stack_with_privilege(
    size: usize,
    privilege: ThreadPrivilege,
) -> Result<GuardedStack, &'static str> {
    let mut mapper = unsafe { crate::memory::paging::get_mapper() };
    GuardedStack::new(size, &mut mapper, privilege)
}

/// Check if a page fault is due to guard page access
#[allow(dead_code)]
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
