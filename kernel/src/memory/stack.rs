use crate::memory::layout::{
    PERCPU_STACK_REGION_BASE, PERCPU_STACK_REGION_SIZE, USER_STACK_REGION_END,
    USER_STACK_REGION_START,
};
#[cfg(target_arch = "x86_64")]
use crate::task::thread::ThreadPrivilege;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::ThreadPrivilege;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{
    Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB, VirtAddr,
};

/// Base address for kernel stack allocation area
/// Must be in kernel space (high canonical addresses)
/// This is placed AFTER the per-CPU stack region to avoid conflicts.
/// Per-CPU stacks use 0xFFFF_C900_0000_0000 + (256 CPUs * 2 MiB stride) = 512 MiB.
pub const KERNEL_STACK_ALLOC_START: u64 =
    PERCPU_STACK_REGION_BASE + PERCPU_STACK_REGION_SIZE as u64;

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
    ///
    /// This function uses a simple incrementing allocator. The bounds checking is
    /// performed BEFORE modifying state, ensuring that on error, the allocation
    /// pointer is not corrupted.
    ///
    /// The bounds check uses `>=` (not `>`) because the boundary addresses are
    /// non-canonical: USER_STACK_REGION_END (0x8000_0000_0000) is the first
    /// non-canonical address in the lower half of the address space.
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
                    // Use the extracted bounds check function to verify allocation is valid
                    // This check happens BEFORE modifying NEXT_USER_STACK_ADDR
                    let proposed_end = check_user_stack_bounds(NEXT_USER_STACK_ADDR, size as u64)?;

                    // Bounds check passed - now safe to update state
                    let addr = VirtAddr::new(NEXT_USER_STACK_ADDR);
                    NEXT_USER_STACK_ADDR = proposed_end;
                    Ok(addr)
                }
                ThreadPrivilege::Kernel => {
                    // Use the extracted bounds check function to verify allocation is valid
                    // This check happens BEFORE modifying NEXT_KERNEL_STACK_ADDR
                    let proposed_end = check_kernel_stack_bounds(NEXT_KERNEL_STACK_ADDR, size as u64)?;

                    // Bounds check passed - now safe to update state
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

// ============================================================================
// Bounds Check Logic - Extracted for Testing
// ============================================================================
//
// The bounds checking logic is critical for preventing:
// 1. Stack allocations that would extend into non-canonical address space
// 2. Integer overflow when computing proposed_end
// 3. Off-by-one errors at the boundary (>= not > to prevent allocation AT boundary)
//
// These helper functions extract the pure logic for unit testing.

/// Check if a proposed user stack allocation is valid.
///
/// This function encapsulates the bounds checking logic used by `find_free_virtual_space`
/// for user stacks. It verifies:
/// 1. The proposed_end doesn't reach or exceed USER_STACK_REGION_END
/// 2. Integer overflow didn't occur (saturating_add wraparound detection)
///
/// # Arguments
/// * `current_addr` - The current allocation pointer (where the next stack would start)
/// * `size` - The size of the stack allocation (including guard page)
///
/// # Returns
/// * `Ok(proposed_end)` - The end address if allocation would be valid
/// * `Err(&'static str)` - Error message if allocation would be invalid
#[inline]
pub fn check_user_stack_bounds(current_addr: u64, size: u64) -> Result<u64, &'static str> {
    let proposed_end = current_addr.saturating_add(size);

    // Check 1: Would the allocation extend to or past the boundary?
    // We use >= because USER_STACK_REGION_END (0x8000_0000_0000) is non-canonical
    // and we cannot allocate AT that address
    if proposed_end >= USER_STACK_REGION_END {
        return Err("Out of virtual address space for user stacks");
    }

    // Check 2: Did saturating_add wrap around (overflow)?
    // If proposed_end < current_addr, we overflowed
    if proposed_end < current_addr {
        return Err("Out of virtual address space for user stacks");
    }

    Ok(proposed_end)
}

/// Check if a proposed kernel stack allocation is valid.
///
/// Similar to `check_user_stack_bounds` but for kernel stack region.
/// The kernel stack region ends at 0xFFFF_CA00_0000_0000.
///
/// # Arguments
/// * `current_addr` - The current allocation pointer
/// * `size` - The size of the stack allocation
///
/// # Returns
/// * `Ok(proposed_end)` - The end address if allocation would be valid
/// * `Err(&'static str)` - Error message if allocation would be invalid
#[inline]
pub fn check_kernel_stack_bounds(current_addr: u64, size: u64) -> Result<u64, &'static str> {
    const KERNEL_STACK_REGION_END: u64 = 0xFFFF_CA00_0000_0000;

    let proposed_end = current_addr.saturating_add(size);

    if proposed_end >= KERNEL_STACK_REGION_END {
        return Err("Out of virtual address space for kernel stacks");
    }

    if proposed_end < current_addr {
        return Err("Out of virtual address space for kernel stacks");
    }

    Ok(proposed_end)
}

// ============================================================================
// Unit Tests for Stack Allocation Bounds Checking
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Test constants - these mirror the actual values from layout.rs
    const TEST_USER_STACK_REGION_START: u64 = 0x7FFF_FF00_0000;
    const TEST_USER_STACK_REGION_END: u64 = 0x8000_0000_0000;
    const TEST_KERNEL_STACK_REGION_START: u64 = 0xFFFF_C900_0000_0000;
    const TEST_KERNEL_STACK_REGION_END: u64 = 0xFFFF_CA00_0000_0000;

    // Standard stack sizes for testing
    const PAGE_SIZE: u64 = 4096;
    const STACK_64K: u64 = 64 * 1024;  // 64 KiB stack
    const STACK_WITH_GUARD: u64 = STACK_64K + PAGE_SIZE;  // Stack + guard page

    // ========================================================================
    // User Stack Bounds Check Tests
    // ========================================================================

    #[test]
    fn test_user_stack_bounds_normal_allocation() {
        // Normal allocation well within bounds should succeed
        let current = TEST_USER_STACK_REGION_START;
        let size = STACK_WITH_GUARD;

        let result = check_user_stack_bounds(current, size);
        assert!(result.is_ok(), "Normal allocation should succeed");

        let proposed_end = result.unwrap();
        assert_eq!(proposed_end, current + size);
        assert!(proposed_end < TEST_USER_STACK_REGION_END);
    }

    #[test]
    fn test_user_stack_bounds_exactly_at_boundary_fails() {
        // Allocation that would end EXACTLY at the boundary should FAIL
        // because >= check means we can't allocate AT 0x8000_0000_0000
        let remaining_space = TEST_USER_STACK_REGION_END - TEST_USER_STACK_REGION_START;
        let current = TEST_USER_STACK_REGION_START;
        let size = remaining_space;  // Would put proposed_end exactly at boundary

        let result = check_user_stack_bounds(current, size);
        assert!(result.is_err(), "Allocation exactly at boundary should fail");
        assert_eq!(result.unwrap_err(), "Out of virtual address space for user stacks");
    }

    #[test]
    fn test_user_stack_bounds_one_byte_before_boundary_succeeds() {
        // Allocation that ends one byte BEFORE the boundary should succeed
        let remaining_space = TEST_USER_STACK_REGION_END - TEST_USER_STACK_REGION_START;
        let current = TEST_USER_STACK_REGION_START;
        let size = remaining_space - 1;  // One byte less than boundary

        let result = check_user_stack_bounds(current, size);
        assert!(result.is_ok(), "Allocation one byte before boundary should succeed");

        let proposed_end = result.unwrap();
        assert_eq!(proposed_end, TEST_USER_STACK_REGION_END - 1);
    }

    #[test]
    fn test_user_stack_bounds_overflow_protection() {
        // Test that integer overflow is caught
        // Start near the end and try to allocate a huge size
        let current = TEST_USER_STACK_REGION_END - PAGE_SIZE;  // Near the end
        let size = u64::MAX;  // Huge size that would overflow

        let result = check_user_stack_bounds(current, size);
        assert!(result.is_err(), "Overflow should be caught");
    }

    #[test]
    fn test_user_stack_bounds_past_boundary_fails() {
        // Allocation that would extend past the boundary should fail
        let remaining_space = TEST_USER_STACK_REGION_END - TEST_USER_STACK_REGION_START;
        let current = TEST_USER_STACK_REGION_START;
        let size = remaining_space + PAGE_SIZE;  // Past the boundary

        let result = check_user_stack_bounds(current, size);
        assert!(result.is_err(), "Allocation past boundary should fail");
    }

    #[test]
    fn test_user_stack_bounds_state_not_modified_on_error() {
        // This test verifies the SEMANTIC requirement that state is not modified on error.
        // Since check_user_stack_bounds is a pure function (no side effects),
        // this test validates that design: the function returns Result without
        // modifying any external state.
        //
        // The actual find_free_virtual_space function only updates NEXT_USER_STACK_ADDR
        // AFTER the bounds check passes, ensuring state consistency on error.

        let current = TEST_USER_STACK_REGION_START;
        let size = u64::MAX;  // Would overflow

        // Call the function multiple times with invalid input
        let result1 = check_user_stack_bounds(current, size);
        let result2 = check_user_stack_bounds(current, size);

        // Both should return the same error
        assert!(result1.is_err());
        assert!(result2.is_err());
        assert_eq!(result1.unwrap_err(), result2.unwrap_err());
    }

    #[test]
    fn test_user_stack_bounds_consecutive_allocations() {
        // Simulate consecutive allocations until exhaustion
        let mut current = TEST_USER_STACK_REGION_START;
        let size = STACK_WITH_GUARD;
        let mut allocation_count = 0;

        loop {
            match check_user_stack_bounds(current, size) {
                Ok(proposed_end) => {
                    allocation_count += 1;
                    current = proposed_end;
                    // Safety: don't run forever
                    if allocation_count > 10000 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        // Verify we made some allocations before exhaustion
        assert!(allocation_count > 0, "Should have made at least one allocation");

        // Verify we stopped before or at the boundary
        assert!(current <= TEST_USER_STACK_REGION_END,
                "Current pointer should not exceed boundary");
    }

    // ========================================================================
    // Kernel Stack Bounds Check Tests
    // ========================================================================

    #[test]
    fn test_kernel_stack_bounds_normal_allocation() {
        let current = TEST_KERNEL_STACK_REGION_START;
        let size = STACK_WITH_GUARD;

        let result = check_kernel_stack_bounds(current, size);
        assert!(result.is_ok(), "Normal kernel allocation should succeed");
    }

    #[test]
    fn test_kernel_stack_bounds_at_boundary_fails() {
        let remaining_space = TEST_KERNEL_STACK_REGION_END - TEST_KERNEL_STACK_REGION_START;
        let current = TEST_KERNEL_STACK_REGION_START;
        let size = remaining_space;

        let result = check_kernel_stack_bounds(current, size);
        assert!(result.is_err(), "Kernel allocation at boundary should fail");
    }

    // ========================================================================
    // Edge Case Tests
    // ========================================================================

    #[test]
    fn test_zero_size_allocation() {
        // Zero-size allocation should technically succeed (proposed_end == current)
        let current = TEST_USER_STACK_REGION_START;
        let size = 0;

        let result = check_user_stack_bounds(current, size);
        assert!(result.is_ok(), "Zero-size allocation should succeed");
        assert_eq!(result.unwrap(), current);
    }

    #[test]
    fn test_single_page_allocation() {
        let current = TEST_USER_STACK_REGION_START;
        let size = PAGE_SIZE;

        let result = check_user_stack_bounds(current, size);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), current + PAGE_SIZE);
    }

    #[test]
    fn test_max_valid_allocation() {
        // Maximum valid allocation: fills entire region minus 1 byte
        let current = TEST_USER_STACK_REGION_START;
        let max_size = TEST_USER_STACK_REGION_END - TEST_USER_STACK_REGION_START - 1;

        let result = check_user_stack_bounds(current, max_size);
        assert!(result.is_ok(), "Max valid allocation should succeed");
        assert_eq!(result.unwrap(), TEST_USER_STACK_REGION_END - 1);
    }

    #[test]
    fn test_boundary_address_is_non_canonical() {
        // Verify that USER_STACK_REGION_END is indeed the canonical boundary
        // 0x8000_0000_0000 is the first non-canonical address in the lower half
        assert_eq!(TEST_USER_STACK_REGION_END, 0x8000_0000_0000);

        // Any allocation that would reach this address must fail
        let current = TEST_USER_STACK_REGION_END - PAGE_SIZE;
        let size = PAGE_SIZE;  // Would end exactly at boundary

        let result = check_user_stack_bounds(current, size);
        assert!(result.is_err(), "Allocation reaching boundary must fail");
    }
}
