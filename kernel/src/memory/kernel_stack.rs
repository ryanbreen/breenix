//! Kernel stack allocator with bitmap management
//!
//! Reserves VA range 0xffffc900_0000_0000 – 0xffffc900_07ff_ffff (128 MiB) for kernel stacks.
//! Each stack gets 512 KiB usable space + 4 KiB guard page (total 516 KiB per slot).

#[cfg(target_arch = "x86_64")]
use crate::memory::frame_allocator::allocate_frame;
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::{structures::paging::PageTableFlags, VirtAddr};
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::VirtAddr;

/// Base address for kernel stack allocation
const KERNEL_STACK_BASE: u64 = 0xffffc900_0000_0000;

/// End address for kernel stack allocation (128 MiB total space)
/// Increased to 128 MiB to support 512KB stacks (kernel stacks are leaked,
/// not freed, so we need enough slots for all processes created during tests)
const KERNEL_STACK_END: u64 = 0xffffc900_0800_0000;

/// Size of each kernel stack (512 KiB)
/// Increased to 512KB to handle interactive mode's deep call stacks.
/// The keyboard interrupt handler path triggers framebuffer echo rendering:
/// keyboard_interrupt → push_char_nonblock → input_char_nonblock → output_char_nonblock
/// → write_char_to_framebuffer → terminal_manager → terminal_pane → font rendering
/// This path can use 300KB+ of stack when combined with interrupt frame overhead
/// and nested help command processing with terminal output formatting.
const KERNEL_STACK_SIZE: u64 = 512 * 1024;

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
    #[allow(dead_code)]
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
#[cfg(target_arch = "x86_64")]
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
            log::trace!("Mapping kernel stack page {:#x} -> {:#x}", virt_addr, frame.start_address());
            crate::memory::kernel_page_table::map_kernel_page(
                virt_addr,
                frame.start_address(),
                flags,
            )?;
            log::trace!("Kernel stack page {:#x} mapped successfully", virt_addr);
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
#[cfg(target_arch = "x86_64")]
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

// =============================================================================
// ARM64-specific kernel stack allocator (identity-mapped)
// =============================================================================

#[cfg(target_arch = "aarch64")]
mod aarch64 {
    use core::sync::atomic::{AtomicU64, Ordering};
    use super::VirtAddr;

    /// ARM64 kernel stack base (in high-half direct map)
    /// Physical range: 0x5100_0000 .. 0x5200_0000 (16MB for kernel stacks)
    const ARM64_KERNEL_STACK_PHYS_BASE: u64 = 0x5100_0000;
    const ARM64_KERNEL_STACK_PHYS_END: u64 = 0x5200_0000;
    const ARM64_KERNEL_STACK_BASE: u64 =
        crate::arch_impl::aarch64::constants::HHDM_BASE + ARM64_KERNEL_STACK_PHYS_BASE;
    const ARM64_KERNEL_STACK_END: u64 =
        crate::arch_impl::aarch64::constants::HHDM_BASE + ARM64_KERNEL_STACK_PHYS_END;

    /// Stack size for ARM64 (64KB per stack)
    const ARM64_KERNEL_STACK_SIZE: u64 = 64 * 1024;

    /// Guard page size (4KB)
    const ARM64_GUARD_PAGE_SIZE: u64 = 4 * 1024;

    /// Total slot size (stack + guard)
    const ARM64_STACK_SLOT_SIZE: u64 = ARM64_KERNEL_STACK_SIZE + ARM64_GUARD_PAGE_SIZE;

    /// Next available stack slot (atomic bump allocator)
    static NEXT_STACK_SLOT: AtomicU64 = AtomicU64::new(ARM64_KERNEL_STACK_BASE);

    /// A kernel stack allocation for ARM64
    #[derive(Debug)]
    pub struct Aarch64KernelStack {
        /// Bottom of the stack (lowest address, above guard page)
        pub bottom: VirtAddr,
        /// Top of the stack (highest address)
        pub top: VirtAddr,
    }

    impl Aarch64KernelStack {
        /// Get the top of the stack (for SP initialization)
        pub fn top(&self) -> VirtAddr {
            self.top
        }
    }

    /// Allocate a kernel stack for ARM64
    ///
    /// Uses a simple bump allocator in the high-half direct map region.
    /// Stacks are not freed (leaked) - this is acceptable for the
    /// current single-process test workload.
    pub fn allocate_kernel_stack() -> Result<Aarch64KernelStack, &'static str> {
        let slot_base = NEXT_STACK_SLOT.fetch_add(ARM64_STACK_SLOT_SIZE, Ordering::SeqCst);

        if slot_base + ARM64_STACK_SLOT_SIZE > ARM64_KERNEL_STACK_END {
            return Err("ARM64 kernel stack pool exhausted");
        }

        let stack_bottom = VirtAddr::new(slot_base + ARM64_GUARD_PAGE_SIZE);
        let stack_top = VirtAddr::new(slot_base + ARM64_STACK_SLOT_SIZE);

        log::debug!(
            "ARM64 kernel stack allocated: {:#x}-{:#x}",
            stack_bottom.as_u64(),
            stack_top.as_u64()
        );

        Ok(Aarch64KernelStack {
            bottom: stack_bottom,
            top: stack_top,
        })
    }

    /// Initialize the ARM64 kernel stack allocator
    pub fn init() {
        let total_slots = (ARM64_KERNEL_STACK_END - ARM64_KERNEL_STACK_BASE) / ARM64_STACK_SLOT_SIZE;
        log::info!(
            "ARM64 kernel stack allocator initialized: {} slots available",
            total_slots
        );
        log::info!(
            "  Stack range (virt): {:#x} - {:#x}",
            ARM64_KERNEL_STACK_BASE,
            ARM64_KERNEL_STACK_END
        );
        log::info!(
            "  Stack range (phys): {:#x} - {:#x}",
            ARM64_KERNEL_STACK_PHYS_BASE,
            ARM64_KERNEL_STACK_PHYS_END
        );
        log::info!(
            "  Stack size: {} KiB + {} KiB guard",
            ARM64_KERNEL_STACK_SIZE / 1024,
            ARM64_GUARD_PAGE_SIZE / 1024
        );
    }
}

#[cfg(target_arch = "aarch64")]
pub use aarch64::{allocate_kernel_stack as allocate_kernel_stack_aarch64, init as init_aarch64, Aarch64KernelStack};

/// ARM64: Use the aarch64-specific allocator
#[cfg(target_arch = "aarch64")]
pub fn allocate_kernel_stack() -> Result<KernelStack, &'static str> {
    let aarch64_stack = allocate_kernel_stack_aarch64()?;
    // Convert to KernelStack format for API compatibility
    Ok(KernelStack {
        index: 0, // Not used for ARM64
        bottom: aarch64_stack.bottom,
        top: aarch64_stack.top,
    })
}

/// ARM64: Initialize the kernel stack allocator
#[cfg(target_arch = "aarch64")]
pub fn init() {
    init_aarch64();
}
