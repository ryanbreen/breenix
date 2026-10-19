#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{OffsetPageTable, VirtAddr};
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use linked_list_allocator::{Heap, LockedHeap};
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

#[cfg(target_arch = "x86_64")]
pub const HEAP_START: u64 = 0x_4444_4444_0000;
#[cfg(target_arch = "aarch64")]
// ARM64 heap uses the direct-mapped region from boot.S (TTBR1 high-half).
// The heap MUST be in TTBR1 because TTBR0 gets switched to process page tables.
//
// Memory layout (physical):
//   Frame allocator: firmware RAM outside the fixed kernel reservations
//   .dma (NC) block: 0x5000_0000 - 0x501F_FFFF  (2 MB, Non-Cacheable for xHCI DMA)
//   Heap:            0x5020_0000 - 0x581F_FFFF  (128 MB on native VMs, 64 MB on QEMU)
//   Kernel stacks:   0x5820_0000 - 0x5A1F_FFFF  (32 MB)
//
// The heap MUST start AFTER the 2 MB NC DMA block to avoid overlapping
// with xHCI DMA buffers placed in the .dma linker section.
pub const HEAP_START: u64 = crate::arch_impl::aarch64::constants::HHDM_BASE + 0x5020_0000;

/// x86 retains its existing heap reservation.
#[cfg(target_arch = "x86_64")]
pub const HEAP_SIZE: u64 = 64 * 1024 * 1024;
/// ARM64 legacy VirGL startup holds about 52 MiB at 1280x960 before the
/// console and other kernel allocations (#1914). Reserve space for both.
#[cfg(target_arch = "aarch64")]
pub const HEAP_SIZE: u64 = 128 * 1024 * 1024;
/// QEMU's GPU core uses frames for backing; retain its frame-pool capacity.
#[cfg(target_arch = "aarch64")]
pub const QEMU_HEAP_SIZE: u64 = 64 * 1024 * 1024;

/// IRQ-safe wrapper around the kernel's free-list allocator.
///
/// On both architectures, every hold of the inner heap mutex masks local
/// interrupts and restores the exact prior interrupt state after the heap
/// operation.  AArch64 masks IRQ and FIQ in DAIF; x86_64 masks the local IF flag
/// via `arch_without_interrupts`, which delegates to
/// `x86_64::instructions::interrupts::without_interrupts`, around the identical
/// inner allocator call.  The scheduler may grow or drop a queue while holding
/// its mutex, so scheduler -> heap is an existing lock-order edge.  Masking
/// interrupts here prevents a task that already owns the heap mutex from being
/// interrupted and adding the reverse heap -> scheduler edge on exception return.
///
/// Keep the masked region limited to one inner heap operation.  In particular,
/// never acquire another lock, allocate, or format output while the inner heap
/// mutex is held.
///
/// On aarch64 a caller that runs with interrupts enabled waits for a busy heap
/// with them enabled, and masks them only to take the mutex and for its own
/// operation, so this CPU does not wait out another CPU's heap operation with
/// its interrupts masked (`gpu_irq_masked_max_us`, #2003).  A caller that
/// already masked interrupts may hold an interrupt-safe lock, so it keeps
/// waiting masked.
struct IrqSafeLockedHeap {
    inner: LockedHeap,
}

impl IrqSafeLockedHeap {
    const fn empty() -> Self {
        Self {
            inner: LockedHeap::empty(),
        }
    }

    #[inline(always)]
    fn with_inner<R>(&self, operation: impl FnOnce(&mut Heap) -> R) -> R {
        #[cfg(target_arch = "aarch64")]
        if crate::arch_interrupts_enabled() {
            let mut operation = Some(operation);
            loop {
                let done = crate::arch_without_interrupts(|| {
                    self.inner
                        .try_lock()
                        .map(|mut heap| (operation.take().expect("heap operation runs once"))(&mut heap))
                });
                if let Some(result) = done {
                    return result;
                }
                while self.inner.is_locked() {
                    core::hint::spin_loop();
                }
            }
        }
        crate::arch_without_interrupts(|| operation(&mut self.inner.lock()))
    }

    unsafe fn init(&self, heap_bottom: *mut u8, heap_size: usize) {
        self.with_inner(|heap| unsafe {
            heap.init(heap_bottom, heap_size);
        });
    }
}

unsafe impl GlobalAlloc for IrqSafeLockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.with_inner(|heap| {
            heap.allocate_first_fit(layout)
                .map_or(core::ptr::null_mut(), |allocation| allocation.as_ptr())
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.with_inner(|heap| unsafe {
            heap.deallocate(NonNull::new_unchecked(ptr), layout);
        });
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { self.alloc(layout) };
        if !ptr.is_null() {
            // Zeroing does not touch allocator metadata, so do it after interrupt
            // state is restored.
            unsafe {
                ptr.write_bytes(0, layout.size());
            }
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        let new_ptr = unsafe { self.alloc(new_layout) };
        if !new_ptr.is_null() {
            // The copy is private to the caller; it does not require the heap mutex.
            unsafe {
                core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new_ptr
    }
}

/// Global allocator instance using a proper free-list allocator.
///
/// Unlike the previous bump allocator, linked_list_allocator properly
/// reclaims freed memory, preventing heap exhaustion from temporary
/// allocations (Vec clones, BTreeMap nodes, etc.).
#[global_allocator]
static ALLOCATOR: IrqSafeLockedHeap = IrqSafeLockedHeap::empty();

/// Initialize the heap allocator
pub fn init(mapper: &OffsetPageTable<'static>) -> Result<(), &'static str> {
    #[cfg(target_arch = "x86_64")]
    let heap_size = HEAP_SIZE;
    #[cfg(target_arch = "aarch64")]
    let heap_size = if crate::platform_config::is_qemu() {
        QEMU_HEAP_SIZE
    } else {
        HEAP_SIZE
    };
    let heap_start = VirtAddr::new(HEAP_START);
    let heap_end = heap_start + heap_size;

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
                log::error!(
                    "HEAP: Allocated frame {:#x} > 4GB - DMA will fail!",
                    frame_phys
                );
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
        log::info!("ARM64 heap using direct-mapped region at {:#x}", HEAP_START);
    }

    // Initialize the allocator
    unsafe {
        ALLOCATOR.init(HEAP_START as *mut u8, heap_size as usize);
    }

    log::info!(
        "Heap initialized at {:#x} with size {} KiB",
        HEAP_START,
        heap_size / 1024
    );

    Ok(())
}

/// Bytes free in the kernel heap. Physical frames are not heap and are
/// counted by the frame allocator instead.
pub fn free_bytes() -> usize {
    ALLOCATOR.with_inner(|heap| heap.free())
}

/// Handle allocation errors
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("allocation error: {:?}", layout)
}
