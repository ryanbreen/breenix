use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;
#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{OffsetPageTable, VirtAddr};

#[cfg(target_arch = "x86_64")]
pub const HEAP_START: u64 = 0x_4444_4444_0000;
#[cfg(target_arch = "aarch64")]
// ARM64 heap uses the direct-mapped region from boot.S.
// boot.S maps TTBR1 L1[1] = physical 0x4000_0000..0x7FFF_FFFF to virtual 0xFFFF_0000_4000_0000..
// We place heap at physical 0x4800_0000 (virtual 0xFFFF_0000_4800_0000) to avoid
// collision with frame allocator starting at 0x4200_0000.
pub const HEAP_START: u64 = crate::arch_impl::aarch64::constants::HHDM_BASE + 0x4800_0000;

/// Heap size of 4 MiB.
///
/// This size was chosen to support concurrent process tests which require:
/// - Multiple child processes (4+) running simultaneously after fork()
/// - Each process needs: fd table (~6KB), pipe buffers (4KB each), ProcessInfo struct,
///   Thread structs, page tables, and kernel stack allocations
/// - Total per-process overhead is approximately 50-100KB depending on fd usage
///
/// IMPORTANT: We use a bump allocator which only reclaims memory when ALL allocations
/// are freed. This means memory fragmentation is effectively permanent during a test run.
/// The 4 MiB size provides sufficient headroom for:
/// - Boot initialization allocations (~500KB)
/// - Running 10+ concurrent processes with full fd tables
/// - Pipe buffers for IPC testing
/// - Safety margin for test variations
///
/// Reduced sizes (1-2 MiB) caused OOM during concurrent fork/pipe tests.
/// Increased from 1 MiB based on empirical testing of pipe_concurrent_test scenarios.
/// Increased from 4 MiB to 32 MiB to accommodate ext2 filesystem operations which
/// allocate Vec buffers that aren't freed by the bump allocator.
/// The test suite runs 43+ processes, each needing kernel stacks (64KB), page tables,
/// file descriptor tables, etc. The bump allocator never reclaims until ALL allocations
/// are freed, so memory accumulates across the entire test run.
pub const HEAP_SIZE: u64 = 32 * 1024 * 1024;

/// A simple bump allocator
struct BumpAllocator {
    heap_start: u64,
    heap_end: u64,
    next: u64,
    allocations: usize,
}

impl BumpAllocator {
    /// Creates a new bump allocator
    pub const fn new() -> Self {
        Self {
            heap_start: 0,
            heap_end: 0,
            next: 0,
            allocations: 0,
        }
    }

    /// Initializes the bump allocator with the given heap bounds
    pub unsafe fn init(&mut self, heap_start: u64, heap_size: u64) {
        self.heap_start = heap_start;
        self.heap_end = heap_start + heap_size;
        self.next = heap_start;
    }
}

/// Wrapper for the global allocator
pub struct GlobalAllocator(Mutex<BumpAllocator>);

unsafe impl core::alloc::GlobalAlloc for GlobalAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let mut allocator = self.0.lock();

        // Align the start address
        let alloc_start = align_up(allocator.next, layout.align() as u64);
        let alloc_end = match alloc_start.checked_add(layout.size() as u64) {
            Some(end) => end,
            None => return core::ptr::null_mut(),
        };

        if alloc_end > allocator.heap_end {
            core::ptr::null_mut() // out of memory
        } else {
            allocator.next = alloc_end;
            allocator.allocations += 1;
            alloc_start as *mut u8
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {
        let mut allocator = self.0.lock();

        allocator.allocations -= 1;
        if allocator.allocations == 0 {
            allocator.next = allocator.heap_start;
        }
    }
}

/// Global allocator instance
/// Defined for all architectures
#[global_allocator]
static ALLOCATOR: GlobalAllocator = GlobalAllocator(Mutex::new(BumpAllocator::new()));

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
        ALLOCATOR.0.lock().init(HEAP_START, HEAP_SIZE);
    }

    log::info!(
        "Heap initialized at {:#x} with size {} KiB",
        HEAP_START,
        HEAP_SIZE / 1024
    );

    Ok(())
}

/// Align the given address upwards to the given alignment
fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

/// Handle allocation errors
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}
