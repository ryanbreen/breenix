use linked_list_allocator::LockedHeap;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{OffsetPageTable, VirtAddr};

#[cfg(target_arch = "x86_64")]
pub const HEAP_START: u64 = 0x_4444_4444_0000;
#[cfg(target_arch = "aarch64")]
// ARM64 heap uses the direct-mapped region from boot.S (TTBR1 high-half).
// The heap MUST be in TTBR1 because TTBR0 gets switched to process page tables.
//
// boot.S maps TTBR1 L1[1] = physical 0x4000_0000..0x7FFF_FFFF to virtual 0xFFFF_0000_4000_0000..
// Frame allocator uses: physical 0x4200_0000 to 0x5000_0000
// Heap must be placed AFTER the frame allocator to avoid collision!
pub const HEAP_START: u64 = crate::arch_impl::aarch64::constants::HHDM_BASE + 0x5000_0000;

/// Heap size: 32 MiB.
///
/// This provides sufficient headroom for:
/// - Boot initialization allocations
/// - Running 10+ concurrent processes with full fd tables
/// - ext2 filesystem operations
/// - Network stack buffers
pub const HEAP_SIZE: u64 = 32 * 1024 * 1024;

/// Global allocator instance using a proper free-list allocator.
///
/// Unlike the previous bump allocator, linked_list_allocator properly
/// reclaims freed memory, preventing heap exhaustion from temporary
/// allocations (Vec clones, BTreeMap nodes, etc.).
#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Initialize the heap allocator
pub fn init(mapper: &OffsetPageTable<'static>) -> Result<(), &'static str> {
    let heap_start = VirtAddr::new(HEAP_START);
    let heap_end = heap_start + HEAP_SIZE;

    // On x86_64, we need to map heap pages. On ARM64, boot.S sets up a direct map
    // so HEAP_START is already backed by physical memory.
    #[cfg(target_arch = "x86_64")]
    {
        let heap_start_page = Page::<Size4KiB>::containing_address(heap_start);
        let heap_end_page = Page::<Size4KiB>::containing_address(heap_end - 1u64);

        log::info!(
            "Mapping heap pages from {:?} to {:?}",
            heap_start_page,
            heap_end_page
        );

        for page in Page::range_inclusive(heap_start_page, heap_end_page) {
            let frame = crate::memory::frame_allocator::allocate_frame().ok_or("out of memory")?;

            let frame_phys = frame.start_address().as_u64();
            if frame_phys > 0xFFFF_FFFF {
                log::error!("HEAP: Allocated frame {:#x} > 4GB - DMA will fail!", frame_phys);
            }

            let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

            unsafe {
                let locked_mapper = mapper as *const _ as *mut OffsetPageTable<'static>;
                let mut frame_allocator = crate::memory::frame_allocator::GlobalFrameAllocator;

                (*locked_mapper)
                    .map_to(page, frame, flags, &mut frame_allocator)
                    .map_err(|_| "failed to map heap page")?
                    .flush();
            }
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        // ARM64: Direct map from boot.S covers heap region, no page mapping needed
        let _ = (mapper, heap_end); // suppress unused warnings
        log::info!(
            "ARM64 heap using direct-mapped region at {:#x}",
            HEAP_START
        );
    }

    // Initialize the allocator
    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE as usize);
    }

    log::info!(
        "Heap initialized at {:#x} with size {} KiB",
        HEAP_START,
        HEAP_SIZE / 1024
    );

    Ok(())
}

/// Handle allocation errors
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}
