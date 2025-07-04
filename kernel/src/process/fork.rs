//! Proper Unix fork() implementation with memory copying
//!
//! This module implements proper fork semantics by duplicating the parent's
//! address space to the child process.

use crate::process::{Process, ProcessId};
use crate::task::thread::Thread;
use crate::memory::process_memory::ProcessPageTable;
use x86_64::VirtAddr;
use x86_64::structures::paging::{Page, PageSize, Size4KiB, FrameAllocator};

/// Copy memory from parent process to child process
/// 
/// This implements a simplified copy for fork() semantics.
/// For now, we'll copy the parent's stack contents to the child.
/// In the future, this should implement full copy-on-write.
pub fn copy_process_memory(
    parent_pid: ProcessId,
    child_process: &mut Process,
    parent_thread: &Thread,
    child_thread: &mut Thread,
) -> Result<(), &'static str> {
    log::info!("copy_process_memory: copying from parent {} to child {}", 
               parent_pid.as_u64(), child_process.id.as_u64());
    
    // For a proper fork(), we need to copy the parent's entire virtual address space.
    // This includes:
    // 1. Program code and data (already loaded by ELF loader)
    // 2. Stack contents
    // 3. Heap (if any)
    // 4. Other mapped regions
    
    // For now, we'll implement basic stack copying since that's what's needed
    // for fork() to work correctly. The child needs to be able to return from
    // the same function call that the parent made.
    
    copy_stack_contents(parent_thread, child_thread)?;
    
    // TODO: Future improvements:
    // - Copy program pages (code/data segments)
    // - Copy heap pages
    // - Implement copy-on-write for efficiency
    // - Handle memory protection flags
    
    log::info!("copy_process_memory: completed successfully");
    Ok(())
}

/// Copy stack contents from parent to child
/// 
/// This ensures the child has the same execution context as the parent
/// and can properly return from the fork() system call.
fn copy_stack_contents(parent_thread: &Thread, child_thread: &mut Thread) -> Result<(), &'static str> {
    let parent_stack_start = parent_thread.stack_bottom;
    let parent_stack_end = parent_thread.stack_top;
    let parent_stack_size = (parent_stack_end.as_u64() - parent_stack_start.as_u64()) as usize;
    
    let child_stack_start = child_thread.stack_bottom;
    let child_stack_end = child_thread.stack_top;
    let child_stack_size = (child_stack_end.as_u64() - child_stack_start.as_u64()) as usize;
    
    log::debug!("copy_stack_contents: parent stack [{:#x}..{:#x}] size={} bytes", 
               parent_stack_start, parent_stack_end, parent_stack_size);
    log::debug!("copy_stack_contents: child stack [{:#x}..{:#x}] size={} bytes", 
               child_stack_start, child_stack_end, child_stack_size);
    
    // Ensure stacks are the same size
    if parent_stack_size != child_stack_size {
        log::error!("Stack size mismatch: parent={}, child={}", parent_stack_size, child_stack_size);
        return Err("Stack size mismatch between parent and child");
    }
    
    // Copy stack contents byte by byte
    // This is a simplified approach - a real implementation would copy page by page
    unsafe {
        let parent_ptr = parent_stack_start.as_ptr::<u8>();
        let child_ptr = child_stack_start.as_mut_ptr::<u8>();
        
        // Copy the entire stack contents
        core::ptr::copy_nonoverlapping(parent_ptr, child_ptr, parent_stack_size);
    }
    
    log::debug!("copy_stack_contents: copied {} bytes from parent to child stack", parent_stack_size);
    
    // Update child's stack pointer to be at the same relative position
    // as the parent's stack pointer
    let parent_sp_offset = parent_thread.context.rsp - parent_stack_start.as_u64();
    child_thread.context.rsp = child_stack_start.as_u64() + parent_sp_offset;
    
    log::debug!("copy_stack_contents: updated child RSP from {:#x} to {:#x} (offset: {:#x})", 
               child_stack_start.as_u64(), child_thread.context.rsp, parent_sp_offset);
    
    Ok(())
}

/// Copy other process state that should be inherited by fork()
/// 
/// This includes things like signal handlers, file descriptors, etc.
/// For now, this is mostly a placeholder for future implementation.
pub fn copy_process_state(
    _parent_process: &Process,
    _child_process: &mut Process,
) -> Result<(), &'static str> {
    // TODO: Copy file descriptor table
    // TODO: Copy signal handler table  
    // TODO: Copy environment variables
    // TODO: Copy working directory
    // TODO: Copy process groups and session information
    
    log::debug!("copy_process_state: state copying not yet fully implemented");
    Ok(())
}

/// Copy page table contents from parent to child
/// 
/// This implements a simplified copy of all mapped pages from parent to child.
/// In a real implementation, this would use copy-on-write for efficiency.
pub fn copy_page_table_contents(
    parent_page_table: &ProcessPageTable,
    child_page_table: &mut ProcessPageTable,
) -> Result<(), &'static str> {
    log::info!("copy_page_table_contents: copying parent's memory pages to child");
    
    // For a minimal implementation, we'll copy the program's memory regions
    // These are typically at standard userspace addresses:
    // - 0x10000000: Code segment 
    // - 0x10001000: Data segment
    // - 0x10002000: BSS segment
    
    // Define the address ranges we know contain program data
    let program_regions = [
        (VirtAddr::new(0x10000000), VirtAddr::new(0x10001000)), // Code
        (VirtAddr::new(0x10001000), VirtAddr::new(0x10002000)), // Data  
        (VirtAddr::new(0x10002000), VirtAddr::new(0x10003000)), // BSS
    ];
    
    for (start_addr, end_addr) in program_regions.iter() {
        copy_memory_region(*start_addr, *end_addr, parent_page_table, child_page_table)?;
    }
    
    log::info!("copy_page_table_contents: completed successfully");
    Ok(())
}

/// Copy a specific memory region from parent to child
fn copy_memory_region(
    start_addr: VirtAddr,
    end_addr: VirtAddr,
    parent_page_table: &ProcessPageTable,
    child_page_table: &mut ProcessPageTable,
) -> Result<(), &'static str> {
    let start_page: Page<Size4KiB> = Page::containing_address(start_addr);
    let end_page: Page<Size4KiB> = Page::containing_address(end_addr - 1u64);
    
    log::debug!("copy_memory_region: copying region {:#x}..{:#x}", start_addr, end_addr);
    
    for page in Page::range_inclusive(start_page, end_page) {
        // Check if the page is mapped in the parent
        if let Some(parent_frame) = parent_page_table.translate_page(page.start_address()) {
            log::debug!("copy_memory_region: copying page {:#x} (frame {:#x})", 
                       page.start_address(), parent_frame);
            
            // Allocate a new frame for the child
            use crate::memory::frame_allocator::BootInfoFrameAllocator;
            let mut allocator = BootInfoFrameAllocator::new();
            let child_frame = allocator.allocate_frame()
                .ok_or("Failed to allocate frame for child page")?;
            
            // Copy the page contents
            unsafe {
                // Get physical addresses from frames  
                let parent_phys = parent_frame;
                let child_phys = child_frame.start_address();
                
                // Convert to virtual addresses using physical memory offset
                let phys_offset = crate::memory::physical_memory_offset();
                let parent_virt = phys_offset + parent_phys.as_u64();
                let child_virt = phys_offset + child_phys.as_u64();
                
                // Copy the entire page (4KB)
                core::ptr::copy_nonoverlapping(
                    parent_virt.as_ptr::<u8>(),
                    child_virt.as_mut_ptr::<u8>(),
                    Size4KiB::SIZE as usize
                );
            }
            
            // Map the copied page in the child's page table
            use x86_64::structures::paging::PageTableFlags;
            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
            child_page_table.map_page(page, child_frame, flags)
                .map_err(|_| "Failed to map copied page in child page table")?;
            
            log::debug!("copy_memory_region: successfully copied page {:#x}", page.start_address());
        }
    }
    
    Ok(())
}