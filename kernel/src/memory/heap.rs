use spin::Mutex;
use x86_64::structures::paging::{Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

pub const HEAP_START: u64 = 0x_4444_4444_0000;
pub const HEAP_SIZE: u64 = 1024 * 1024; // 1 MiB

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
#[global_allocator]
static ALLOCATOR: GlobalAllocator = GlobalAllocator(Mutex::new(BumpAllocator::new()));

/// Initialize the heap allocator
pub fn init(mapper: &OffsetPageTable<'static>) -> Result<(), &'static str> {
    let heap_start = VirtAddr::new(HEAP_START);
    let heap_end = heap_start + HEAP_SIZE;

    // Map heap pages
    let heap_start_page = Page::<Size4KiB>::containing_address(heap_start);
    let heap_end_page = Page::<Size4KiB>::containing_address(heap_end - 1u64);

    log::info!(
        "Mapping heap pages from {:?} to {:?}",
        heap_start_page,
        heap_end_page
    );

    for page in Page::range_inclusive(heap_start_page, heap_end_page) {
        let frame = crate::memory::frame_allocator::allocate_frame().ok_or("out of memory")?;

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
