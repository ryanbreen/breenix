//! Kernel stack allocator with bitmap management
//!
//! Reserves VA range 0xffffc900_0000_0000 – 0xffffc900_00ff_ffff for kernel stacks.
//! Each stack gets 8 KiB RW page + 4 KiB guard page (total 12 KiB per stack).

use crate::memory::frame_allocator::allocate_frame;
use spin::Mutex;
use x86_64::{structures::paging::PageTableFlags, VirtAddr};

/// Base address for kernel stack allocation
const KERNEL_STACK_BASE: u64 = 0xffffc900_0000_0000;

/// End address for kernel stack allocation (16 MiB total space)
const KERNEL_STACK_END: u64 = 0xffffc900_0100_0000;

/// Size of each kernel stack (64 KiB) - increased from 16KB to handle deep call stacks
/// Process creation involves deeply nested function calls and page table manipulation
/// TODO: Investigate stack usage and potentially reduce back to 16KB after optimization
const KERNEL_STACK_SIZE: u64 = 64 * 1024;

/// Size of guard page (4 KiB)
const GUARD_PAGE_SIZE: u64 = 4 * 1024;

/// Total size per stack slot (stack + guard)
const STACK_SLOT_SIZE: u64 = KERNEL_STACK_SIZE + GUARD_PAGE_SIZE;

/// Maximum number of kernel stacks
const MAX_KERNEL_STACKS: usize =
    ((KERNEL_STACK_END - KERNEL_STACK_BASE) / STACK_SLOT_SIZE) as usize;

/// Bitmap to track allocated stacks (1 bit per stack)
/// Using u64 array for efficient bit operations
const BITMAP_SIZE: usize = (MAX_KERNEL_STACKS + 63) / 64;
static STACK_BITMAP: Mutex<[u64; BITMAP_SIZE]> = Mutex::new([0; BITMAP_SIZE]);

/// A kernel stack allocation
#[derive(Debug)]
pub struct KernelStack {
    /// Index in the bitmap
    index: usize,
    /// Bottom of the stack (lowest address, above guard page)
    bottom: VirtAddr,
    /// Top of the stack (highest address)
    top: VirtAddr,
}

impl KernelStack {
    /// Get the top of the stack (for RSP initialization)
    pub fn top(&self) -> VirtAddr {
        self.top
    }

    /// Get the bottom of the stack
    pub fn bottom(&self) -> VirtAddr {
        self.bottom
    }

    /// Get the guard page address
    pub fn guard_page(&self) -> VirtAddr {
        VirtAddr::new(self.bottom.as_u64() - GUARD_PAGE_SIZE)
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        // Mark the stack as free in the bitmap
        let mut bitmap = STACK_BITMAP.lock();
        let word_index = self.index / 64;
        let bit_index = self.index % 64;
        bitmap[word_index] &= !(1u64 << bit_index);

        log::trace!("Freed kernel stack slot {}", self.index);
    }
}

/// Allocate a new kernel stack
///
/// This allocates 8 KiB for the stack + 4 KiB guard page.
/// The stack is immediately mapped in the global kernel page tables.
pub fn allocate_kernel_stack() -> Result<KernelStack, &'static str> {
    // Find a free slot in the bitmap
    let mut bitmap = STACK_BITMAP.lock();

    let mut slot_index = None;
    for (word_idx, word) in bitmap.iter_mut().enumerate() {
        if *word != u64::MAX {
            // This word has at least one free bit
            for bit_idx in 0..64 {
                let global_idx = word_idx * 64 + bit_idx;
                if global_idx >= MAX_KERNEL_STACKS {
                    break;
                }

                if (*word & (1u64 << bit_idx)) == 0 {
                    // Found a free slot
                    *word |= 1u64 << bit_idx;
                    slot_index = Some(global_idx);
                    break;
                }
            }

            if slot_index.is_some() {
                break;
            }
        }
    }

    let index = slot_index.ok_or("No free kernel stack slots")?;
    drop(bitmap); // Release the lock early

    // Calculate addresses
    let slot_base = KERNEL_STACK_BASE + (index as u64 * STACK_SLOT_SIZE);
    let guard_page = VirtAddr::new(slot_base);
    let stack_bottom = VirtAddr::new(slot_base + GUARD_PAGE_SIZE);
    let stack_top = VirtAddr::new(slot_base + STACK_SLOT_SIZE);

    // Map the stack pages (but not the guard page)
    // CRITICAL: Do NOT use GLOBAL flag for stack pages (per Cursor guidance)
    // Stack pages are per-thread and GLOBAL would keep stale TLB entries
    // Also set NO_EXECUTE since stacks should not contain executable code
    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;

    let num_pages = (KERNEL_STACK_SIZE / 4096) as usize;
    log::debug!("Mapping {} pages for kernel stack {}", num_pages, index);
    for i in 0..num_pages {
        let virt_addr = stack_bottom + (i as u64 * 4096);

        // Allocate a physical frame
        let frame = allocate_frame().ok_or("Out of memory for kernel stack")?;

        log::trace!(
            "  Mapping stack page {}: {:#x} -> {:#x}",
            i,
            virt_addr,
            frame.start_address()
        );

        // Map it in the global kernel page tables
        unsafe {
            crate::memory::kernel_page_table::map_kernel_page(
                virt_addr,
                frame.start_address(),
                flags,
            )?;
        }
    }

    log::debug!(
        "Allocated kernel stack {} at {:#x}-{:#x} (guard at {:#x})",
        index,
        stack_bottom,
        stack_top,
        guard_page
    );

    Ok(KernelStack {
        index,
        bottom: stack_bottom,
        top: stack_top,
    })
}

/// Initialize the kernel stack allocator
///
/// This should be called during memory system initialization.
pub fn init() {
    // The bitmap is already statically initialized to all zeros (all free)
    log::info!(
        "Kernel stack allocator initialized: {} slots available",
        MAX_KERNEL_STACKS
    );
    log::info!(
        "  Stack range: {:#x} - {:#x}",
        KERNEL_STACK_BASE,
        KERNEL_STACK_END
    );
    log::info!(
        "  Stack size: {} KiB + {} KiB guard",
        KERNEL_STACK_SIZE / 1024,
        GUARD_PAGE_SIZE / 1024
    );
}
