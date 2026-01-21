//! Proper Unix fork() implementation with Copy-on-Write optimization
//!
//! This module implements proper fork semantics. Two modes are available:
//!
//! 1. **Copy-on-Write (CoW)** - `setup_cow_pages()`: Marks pages as shared and
//!    read-only. Pages are only copied when written to (handled by page fault).
//!    This is much faster for fork+exec patterns.
//!
//! 2. **Full Copy** - `copy_user_pages()`: Immediately copies all pages. Used
//!    as fallback and for testing.

use crate::memory::frame_allocator::allocate_frame;
use crate::memory::frame_metadata::frame_incref;
use crate::memory::process_memory::{make_cow_flags, ProcessPageTable};
use crate::process::{Process, ProcessId};
use crate::task::thread::Thread;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::VirtAddr;

/// Copy all mapped user pages from parent to child
///
/// This implements proper fork() semantics by:
/// 1. Walking parent's page tables to find all mapped user pages
/// 2. Allocating new physical frames for the child
/// 3. Copying the page contents from parent to child
/// 4. Mapping the new frames in child's page table with same flags
///
/// Returns the number of pages copied.
pub fn copy_user_pages(
    parent_page_table: &ProcessPageTable,
    child_page_table: &mut ProcessPageTable,
) -> Result<usize, &'static str> {
    let phys_offset = crate::memory::physical_memory_offset();
    let mut pages_copied = 0;
    let mut copy_error: Option<&'static str> = None;

    log::info!("copy_user_pages: starting to copy parent's address space to child");

    // Walk parent's page tables and copy each mapped page
    parent_page_table.walk_mapped_pages(|virt_addr, parent_phys, flags| {
        // Skip if we've already encountered an error
        if copy_error.is_some() {
            return;
        }

        // Only copy user-accessible pages (skip any kernel pages that slipped through)
        if !flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            log::trace!(
                "copy_user_pages: skipping non-user page at {:#x}",
                virt_addr.as_u64()
            );
            return;
        }

        // Allocate a new physical frame for the child
        let child_frame = match allocate_frame() {
            Some(frame) => frame,
            None => {
                log::error!("copy_user_pages: out of memory allocating frame for {:#x}", virt_addr.as_u64());
                copy_error = Some("Out of memory during fork");
                return;
            }
        };

        // Copy the page contents from parent to child
        // Access both pages via the kernel's physical memory offset
        let parent_virt = phys_offset + parent_phys.as_u64();
        let child_virt = phys_offset + child_frame.start_address().as_u64();

        unsafe {
            core::ptr::copy_nonoverlapping(
                parent_virt.as_ptr::<u8>(),
                child_virt.as_mut_ptr::<u8>(),
                4096, // PAGE_SIZE
            );
        }

        // Map the new frame in child's page table with the same flags
        let page = Page::<Size4KiB>::containing_address(virt_addr);
        if let Err(e) = child_page_table.map_page(page, child_frame, flags) {
            log::error!(
                "copy_user_pages: failed to map page {:#x} in child: {}",
                virt_addr.as_u64(),
                e
            );
            copy_error = Some("Failed to map page in child");
            return;
        }

        pages_copied += 1;
        log::trace!(
            "copy_user_pages: copied page {:#x} (parent phys {:#x} -> child phys {:#x})",
            virt_addr.as_u64(),
            parent_phys.as_u64(),
            child_frame.start_address().as_u64()
        );
    })?;

    if let Some(err) = copy_error {
        return Err(err);
    }

    log::info!("copy_user_pages: copied {} pages from parent to child", pages_copied);
    Ok(pages_copied)
}

/// Set up Copy-on-Write sharing between parent and child
///
/// Instead of copying all pages immediately (expensive), this function:
/// 1. Marks writable pages as read-only in BOTH parent and child (CoW flag)
/// 2. Maps the same physical frames in both page tables
/// 3. Tracks shared frames via reference counting
///
/// When either process later writes to a CoW page, a page fault occurs,
/// the page is copied, and the writing process gets a private copy.
///
/// Read-only pages (like code sections) are shared directly without CoW
/// overhead since they can never be written.
///
/// Returns the number of pages set up for sharing.
#[allow(dead_code)]
pub fn setup_cow_pages(
    parent_page_table: &mut ProcessPageTable,
    child_page_table: &mut ProcessPageTable,
) -> Result<usize, &'static str> {
    let mut pages_shared = 0;
    let mut cow_error: Option<&'static str> = None;

    log::info!("setup_cow_pages: setting up CoW sharing between parent and child");

    // First pass: collect all pages we need to process
    // (We can't modify parent while iterating, so we collect first)
    let mut pages_to_share: alloc::vec::Vec<(VirtAddr, x86_64::PhysAddr, PageTableFlags)> =
        alloc::vec::Vec::new();

    parent_page_table.walk_mapped_pages(|virt_addr, phys_addr, flags| {
        // Only process user-accessible pages
        if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
            pages_to_share.push((virt_addr, phys_addr, flags));
        }
    })?;

    log::info!(
        "setup_cow_pages: found {} user pages to share",
        pages_to_share.len()
    );

    // Second pass: set up CoW for each page
    for (virt_addr, phys_addr, flags) in pages_to_share {
        if cow_error.is_some() {
            break;
        }

        let page = Page::<Size4KiB>::containing_address(virt_addr);
        let frame = PhysFrame::containing_address(phys_addr);

        if flags.contains(PageTableFlags::WRITABLE) {
            // Writable page - needs CoW protection
            let cow_flags = make_cow_flags(flags);

            // Mark parent page as CoW (read-only + COW flag)
            if let Err(e) = parent_page_table.update_page_flags(page, cow_flags) {
                log::error!(
                    "setup_cow_pages: failed to mark parent page {:#x} as CoW: {}",
                    virt_addr.as_u64(),
                    e
                );
                cow_error = Some("Failed to mark parent page as CoW");
                continue;
            }

            // CRITICAL: Flush the TLB entry for this page in the parent's address space
            // Without this, the parent's TLB may still have the old WRITABLE entry,
            // allowing writes without triggering page faults. This causes memory
            // corruption since parent and child would write to the same physical frame.
            x86_64::instructions::tlb::flush(virt_addr);

            // Map same frame in child with CoW flags
            if let Err(e) = child_page_table.map_page(page, frame, cow_flags) {
                log::error!(
                    "setup_cow_pages: failed to map CoW page {:#x} in child: {}",
                    virt_addr.as_u64(),
                    e
                );
                cow_error = Some("Failed to map CoW page in child");
                continue;
            }

            // Increment reference count (frame is now shared)
            frame_incref(frame);

            log::trace!(
                "setup_cow_pages: CoW page {:#x} -> frame {:#x}",
                virt_addr.as_u64(),
                frame.start_address().as_u64()
            );
        } else {
            // Read-only page (e.g., code) - share directly without CoW flag
            // These pages can never be written, so no fault handling needed
            if let Err(e) = child_page_table.map_page(page, frame, flags) {
                log::error!(
                    "setup_cow_pages: failed to share read-only page {:#x} in child: {}",
                    virt_addr.as_u64(),
                    e
                );
                cow_error = Some("Failed to share read-only page");
                continue;
            }

            // Still track reference for cleanup when process exits
            frame_incref(frame);

            log::trace!(
                "setup_cow_pages: shared RO page {:#x} -> frame {:#x}",
                virt_addr.as_u64(),
                frame.start_address().as_u64()
            );
        }

        pages_shared += 1;
    }

    if let Some(err) = cow_error {
        return Err(err);
    }

    log::info!(
        "setup_cow_pages: set up {} pages for CoW sharing",
        pages_shared
    );
    Ok(pages_shared)
}

/// Copy memory from parent process to child process
///
/// This implements full copy for fork() semantics including:
/// 1. All program code and data pages
/// 2. Stack contents
/// 3. Heap (if any)
/// 4. Other mapped regions
#[allow(dead_code)]
pub fn copy_process_memory(
    parent_pid: ProcessId,
    child_process: &mut Process,
    parent_page_table: &ProcessPageTable,
    child_page_table: &mut ProcessPageTable,
    parent_thread: &Thread,
    child_thread: &mut Thread,
) -> Result<(), &'static str> {
    log::info!(
        "copy_process_memory: copying from parent {} to child {}",
        parent_pid.as_u64(),
        child_process.id.as_u64()
    );

    // Copy all user pages from parent to child
    let pages_copied = copy_user_pages(parent_page_table, child_page_table)?;
    log::info!("copy_process_memory: copied {} user pages", pages_copied);

    // Copy heap metadata
    child_process.heap_start = child_process.heap_start; // Already set, but be explicit
    child_process.heap_end = child_process.heap_end;

    // Adjust child's stack pointer relative to parent's
    copy_stack_state(parent_thread, child_thread)?;

    log::info!("copy_process_memory: completed successfully");
    Ok(())
}

/// Copy stack state from parent to child
///
/// The actual stack pages are already copied by copy_user_pages().
/// This function adjusts the child's RSP to the same relative position
/// in its stack as the parent's RSP is in the parent's stack.
fn copy_stack_state(
    parent_thread: &Thread,
    child_thread: &mut Thread,
) -> Result<(), &'static str> {
    let parent_stack_top = parent_thread.stack_top.as_u64();
    let child_stack_top = child_thread.stack_top.as_u64();
    let parent_rsp = parent_thread.context.rsp;

    // Calculate how far the parent's RSP is from the top of its stack
    let stack_offset_from_top = parent_stack_top.saturating_sub(parent_rsp);

    // Set child's RSP to the same relative position in its stack
    let child_rsp = child_stack_top.saturating_sub(stack_offset_from_top);
    child_thread.context.rsp = child_rsp;

    log::debug!(
        "copy_stack_state: parent RSP={:#x} (offset {} from top {:#x})",
        parent_rsp,
        stack_offset_from_top,
        parent_stack_top
    );
    log::debug!(
        "copy_stack_state: child RSP={:#x} (top={:#x})",
        child_rsp,
        child_stack_top
    );

    Ok(())
}

/// Copy stack contents from parent to child (for when stacks are at different addresses)
///
/// This is used when the child has a separate stack allocation from the parent.
/// The stack pages need to be copied separately from the main copy_user_pages
/// because they may be at different virtual addresses.
#[allow(dead_code)]
pub fn copy_stack_contents(
    parent_thread: &Thread,
    child_thread: &mut Thread,
    parent_page_table: &ProcessPageTable,
    child_page_table: &ProcessPageTable,
) -> Result<(), &'static str> {
    let phys_offset = crate::memory::physical_memory_offset();

    let parent_stack_top = parent_thread.stack_top.as_u64();
    let parent_stack_bottom = parent_thread.stack_bottom.as_u64();
    let parent_rsp = parent_thread.context.rsp;

    let child_stack_top = child_thread.stack_top.as_u64();
    let child_stack_bottom = child_thread.stack_bottom.as_u64();

    // Calculate stack usage
    let stack_used = parent_stack_top.saturating_sub(parent_rsp);
    let parent_stack_size = parent_stack_top.saturating_sub(parent_stack_bottom);
    let child_stack_size = child_stack_top.saturating_sub(child_stack_bottom);

    log::debug!(
        "copy_stack_contents: parent stack [{:#x}..{:#x}], RSP={:#x}, used={}",
        parent_stack_bottom,
        parent_stack_top,
        parent_rsp,
        stack_used
    );
    log::debug!(
        "copy_stack_contents: child stack [{:#x}..{:#x}]",
        child_stack_bottom,
        child_stack_top
    );

    if parent_stack_size != child_stack_size {
        log::warn!(
            "Stack size mismatch: parent={}, child={}",
            parent_stack_size,
            child_stack_size
        );
    }

    // Set child's RSP to same relative position
    let child_rsp = child_stack_top.saturating_sub(stack_used);
    child_thread.context.rsp = child_rsp;

    // Copy stack pages that are actually in use
    // Start from the page containing RSP to the top of stack
    let start_page_addr = parent_rsp & !0xFFF; // Page-align down
    let mut parent_page_addr = start_page_addr;

    while parent_page_addr < parent_stack_top {
        // Calculate corresponding child page address
        let offset_from_top = parent_stack_top - parent_page_addr;
        let child_page_addr = child_stack_top - offset_from_top;

        // Translate parent page to physical
        let parent_phys = match parent_page_table.translate_page(VirtAddr::new(parent_page_addr)) {
            Some(phys) => phys,
            None => {
                log::warn!(
                    "copy_stack_contents: parent stack page {:#x} not mapped, skipping",
                    parent_page_addr
                );
                parent_page_addr += 4096;
                continue;
            }
        };

        // Translate child page to physical
        let child_phys = match child_page_table.translate_page(VirtAddr::new(child_page_addr)) {
            Some(phys) => phys,
            None => {
                log::error!(
                    "copy_stack_contents: child stack page {:#x} not mapped!",
                    child_page_addr
                );
                return Err("Child stack page not mapped");
            }
        };

        // Copy via kernel physical mapping
        let parent_virt = phys_offset + parent_phys.as_u64();
        let child_virt = phys_offset + child_phys.as_u64();

        unsafe {
            core::ptr::copy_nonoverlapping(
                parent_virt.as_ptr::<u8>(),
                child_virt.as_mut_ptr::<u8>(),
                4096,
            );
        }

        log::trace!(
            "copy_stack_contents: copied stack page {:#x} -> {:#x}",
            parent_page_addr,
            child_page_addr
        );

        parent_page_addr += 4096;
    }

    log::debug!(
        "copy_stack_contents: set child RSP to {:#x}",
        child_thread.context.rsp
    );

    Ok(())
}

/// Copy other process state that should be inherited by fork()
///
/// This function copies all process state that a child should inherit from its parent
/// according to POSIX fork() semantics:
///
/// - **File descriptors**: Child inherits all open FDs with shared file positions.
///   The FdTable::clone() handles incrementing reference counts for pipes and other
///   shared resources, ensuring proper cleanup when either process closes an FD.
///
/// - **Signal handlers**: Child inherits parent's signal handlers and signal mask.
///   Pending signals are NOT inherited (child starts with empty pending set per POSIX).
///   The SignalState::fork() method handles this correctly.
///
/// - **Process group ID (pgid)**: Already copied during Process::new() creation in the
///   fork path. Verified here for consistency.
///
/// - **Session ID (sid)**: Already copied during Process::new() creation in the fork path.
///   Verified here for consistency.
///
/// - **umask**: Not yet tracked per-process (uses global default). TODO when implemented.
///
/// - **Current working directory**: Inherited from parent in fork_internal().
///
/// Note: Memory (pages, heap bounds) and stack are copied separately by copy_user_pages()
/// and copy_stack_contents() before this function is called.
pub fn copy_process_state(
    parent_process: &Process,
    child_process: &mut Process,
) -> Result<(), &'static str> {
    log::debug!(
        "copy_process_state: copying state from parent {} to child {}",
        parent_process.id.as_u64(),
        child_process.id.as_u64()
    );

    // 1. Copy file descriptor table
    // FdTable::clone() properly handles:
    // - Cloning all FD entries
    // - Incrementing pipe reader/writer reference counts
    // - Arc cloning for shared sockets and files
    child_process.fd_table = parent_process.fd_table.clone();
    log::debug!(
        "copy_process_state: cloned fd_table from parent {} to child {}",
        parent_process.id.as_u64(),
        child_process.id.as_u64()
    );

    // 2. Copy signal state (handlers and mask, NOT pending signals)
    // SignalState::fork() creates a new state with:
    // - pending = 0 (empty, per POSIX)
    // - blocked = parent.blocked (inherited)
    // - handlers = parent.handlers.clone() (inherited)
    child_process.signals = parent_process.signals.fork();
    log::debug!(
        "copy_process_state: forked signal state from parent {} to child {}",
        parent_process.id.as_u64(),
        child_process.id.as_u64()
    );

    // 3. Verify process group ID was already set correctly
    // The fork path should have already set child.pgid = parent.pgid
    // We verify this here rather than overwriting to catch bugs
    if child_process.pgid != parent_process.pgid {
        log::warn!(
            "copy_process_state: pgid mismatch! child={}, parent={}. Correcting.",
            child_process.pgid.as_u64(),
            parent_process.pgid.as_u64()
        );
        child_process.pgid = parent_process.pgid;
    }

    // 4. Verify session ID was already set correctly
    if child_process.sid != parent_process.sid {
        log::warn!(
            "copy_process_state: sid mismatch! child={}, parent={}. Correcting.",
            child_process.sid.as_u64(),
            parent_process.sid.as_u64()
        );
        child_process.sid = parent_process.sid;
    }

    // 5. Copy umask (when per-process umask is implemented)
    // TODO: child_process.umask = parent_process.umask;

    // 6. Current working directory: inherited from parent in fork_internal()
    //    (before copy_process_state is called)

    log::debug!(
        "copy_process_state: completed state copy for child {}",
        child_process.id.as_u64()
    );
    Ok(())
}

/// Copy page table contents from parent to child (legacy wrapper)
///
/// This is kept for compatibility but now uses the proper copy_user_pages().
#[allow(dead_code)]
pub fn copy_page_table_contents(
    parent_page_table: &ProcessPageTable,
    child_page_table: &mut ProcessPageTable,
) -> Result<(), &'static str> {
    log::info!("copy_page_table_contents: using proper page copying");
    copy_user_pages(parent_page_table, child_page_table)?;
    Ok(())
}
