#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{OffsetPageTable, VirtAddr};
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

#[path = "heap_index.rs"]
mod index;
use index::Heap;

#[path = "heap_case.rs"]
mod case;
pub(crate) use case::{sys_heap_case, PR_HEAP_CASE};

struct Inner {
    heap: Heap,
    counts: case::Counts,
}

impl Inner {
    #[inline(always)]
    fn run<R>(
        &mut self,
        mut wait: case::LockWait,
        operation: impl FnOnce(&mut Heap) -> (R, u32),
    ) -> R {
        wait.acquired();
        let (result, walk) = operation(&mut self.heap);
        self.counts.record(&self.heap, wait, walk);
        result
    }
}
#[cfg(target_arch = "x86_64")]
use x86_64::structures::paging::{Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB};
#[cfg(target_arch = "x86_64")]
use x86_64::VirtAddr;

#[cfg(target_arch = "x86_64")]
pub const HEAP_START: u64 = 0x_4444_4444_0000;

/// x86 retains its existing heap reservation.
#[cfg(target_arch = "x86_64")]
pub const HEAP_SIZE: u64 = 64 * 1024 * 1024;

/// The lowest address the kernel heap can start at: on ARM64 it is somewhere
/// in the high-half direct map, which no user address reaches.
#[cfg(target_arch = "x86_64")]
pub const HEAP_REGION_START: u64 = HEAP_START;
#[cfg(target_arch = "aarch64")]
pub const HEAP_REGION_START: u64 = crate::arch_impl::aarch64::constants::HHDM_BASE;

// ARM64: the heap lives in the high-half direct map (TTBR1), because TTBR0 is
// switched to process page tables. Where, and how large, comes from the
// memory map: the frame pool carves it out of the RAM the firmware describes
// (`platform_config::kernel_pools`, sized by `phys_memory::kernel_heap_size`),
// so its pages are not in the pool the frame allocator hands out.
#[cfg(target_arch = "aarch64")]
static HEAP_BASE: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
#[cfg(target_arch = "aarch64")]
static HEAP_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The kernel heap's first address (0 on ARM64 before `init`).
pub fn heap_start() -> u64 {
    #[cfg(target_arch = "x86_64")]
    return HEAP_START;
    #[cfg(target_arch = "aarch64")]
    return HEAP_BASE.load(Ordering::Relaxed);
}

/// The kernel heap's size in bytes (0 on ARM64 before `init`).
pub fn heap_size() -> u64 {
    #[cfg(target_arch = "x86_64")]
    return HEAP_SIZE;
    #[cfg(target_arch = "aarch64")]
    return HEAP_BYTES.load(Ordering::Relaxed);
}

/// IRQ-safe wrapper around the kernel's size-indexed allocator.
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
    inner: Mutex<Inner>,
}

impl IrqSafeLockedHeap {
    const fn empty() -> Self {
        Self {
            inner: Mutex::new(Inner {
                heap: Heap::empty(),
                counts: case::Counts::new(),
            }),
        }
    }

    #[inline(always)]
    fn with_inner<R>(&self, operation: impl FnOnce(&mut Heap) -> (R, u32)) -> R {
        let mut wait = case::LockWait::new();
        #[cfg(target_arch = "aarch64")]
        if crate::arch_interrupts_enabled() {
            let mut operation = Some(operation);
            loop {
                let mut acquired = Some(wait);
                let done = crate::arch_without_interrupts(|| {
                    self.inner.try_lock().map(|mut inner| {
                        inner.run(acquired.take().unwrap(), operation.take().unwrap())
                    })
                });
                if let Some(result) = done {
                    return result;
                }
                wait = acquired.unwrap();
                wait.contended();
                while self.inner.is_locked() {
                    core::hint::spin_loop();
                }
            }
        }
        crate::arch_without_interrupts(|| {
            if wait.active() {
                if let Some(mut inner) = self.inner.try_lock() {
                    return inner.run(wait, operation);
                }
                wait.contended();
            }
            self.inner.lock().run(wait, operation)
        })
    }

    unsafe fn init(&self, heap_bottom: *mut u8, heap_size: usize) {
        self.with_inner(|heap| unsafe {
            heap.init(heap_bottom, heap_size);
            ((), 0)
        });
    }
}

/// The most heap bytes in use at once since boot, counted as the allocator
/// counts them, including block headers and alignment padding.
static HIGH_WATER: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for IrqSafeLockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.with_inner(|heap| {
            let (ptr, walk) = heap.allocate(layout);
            let ptr = ptr.map_or(core::ptr::null_mut(), NonNull::as_ptr);
            let used = heap.used();
            if used > HIGH_WATER.load(Ordering::Relaxed) {
                HIGH_WATER.store(used, Ordering::Relaxed);
            }
            (ptr, walk)
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        self.with_inner(|heap| unsafe {
            heap.deallocate(NonNull::new_unchecked(ptr));
            ((), 0)
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

/// Global allocator over the boot-reserved kernel heap.
#[global_allocator]
static ALLOCATOR: IrqSafeLockedHeap = IrqSafeLockedHeap::empty();

/// Initialize the heap allocator
pub fn init(mapper: &OffsetPageTable<'static>) -> Result<(), &'static str> {
    #[cfg(target_arch = "x86_64")]
    let (heap_base, heap_size) = (HEAP_START, HEAP_SIZE);
    #[cfg(target_arch = "aarch64")]
    let (heap_base, heap_size) = {
        let pools = crate::platform_config::kernel_pools().ok_or("no heap placed in RAM")?;
        let base = crate::arch_impl::aarch64::constants::HHDM_BASE + pools.heap.start;
        HEAP_BASE.store(base, Ordering::Relaxed);
        HEAP_BYTES.store(pools.heap.len(), Ordering::Relaxed);
        (base, pools.heap.len())
    };
    let heap_start = VirtAddr::new(heap_base);
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
        // ARM64: the direct map covers the heap's RAM; no page mapping needed.
        let _ = (mapper, heap_end);
        log::info!("ARM64 heap using direct-mapped region at {:#x}", heap_base);
    }

    // Initialize the allocator
    unsafe {
        ALLOCATOR.init(heap_base as *mut u8, heap_size as usize);
    }

    log::info!(
        "Heap initialized at {:#x} with size {} KiB",
        heap_base,
        heap_size / 1024
    );

    Ok(())
}

/// Bytes free in the kernel heap. Physical frames are not heap and are
/// counted by the frame allocator instead.
pub fn free_bytes() -> usize {
    ALLOCATOR.with_inner(|heap| (heap.free(), 0))
}

/// The most bytes the kernel heap has had in use at once since boot.
pub fn high_water_bytes() -> usize {
    HIGH_WATER.load(Ordering::Relaxed)
}

/// Print the failing allocation and its callers before the general panic
/// handler. The heap and serial paths may already be under contention.
#[cfg(target_arch = "x86_64")]
#[cold]
fn report_oom(layout: Layout) {
    use crate::capture::record::Writer;
    use crate::per_cpu;

    let mut writer = Writer::new();
    if !writer.open("OOM") {
        return;
    }
    writer.kv_dec("size", layout.size() as u64);
    writer.kv_dec("align", layout.align() as u64);
    writer.kv_dec("high_water", high_water_bytes() as u64);
    if let Some(guard) = ALLOCATOR.inner.try_lock() {
        let free = guard.heap.free();
        let (extents, largest) = guard.heap.free_shape();
        drop(guard);
        writer.kv_dec("free", free as u64);
        writer.kv_dec("extents", u64::from(extents));
        writer.kv_dec("largest", u64::from(largest));
    }
    if !per_cpu::is_initialized() {
        let _ = writer.close();
        return;
    }
    writer.kv_dec("cpu", per_cpu::cpu_id() as u64);
    if let Some(tid) = per_cpu::current_thread_id_lock_free() {
        writer.kv_dec("tid", tid);
    }
    // The matching ELF can use this symbol address to account for relocation.
    writer.kv_hex("anchor", alloc_error_handler as *const () as u64);
    if !writer.close() {
        return;
    }

    let (mut frame, rsp): (u64, u64);
    unsafe {
        core::arch::asm!(
            "mov {frame}, rbp", "mov {stack}, rsp",
            frame = out(reg) frame, stack = out(reg) rsp,
            options(nomem, nostack, preserves_flags),
        );
    }
    let top = per_cpu::kernel_stack_top();
    if !super::kernel_stack::kernel_stack_contains(top, rsp) {
        return;
    }
    for index in 0..16 {
        // Follow only upward, aligned links within the current mapped stack.
        // The user frame and any unrelated RBP value will end the walk.
        if frame < rsp || frame & 7 != 0 || frame > top.saturating_sub(16) {
            break;
        }
        let (parent, pc) = unsafe {
            let words = frame as *const u64;
            (words.read(), words.add(1).read())
        };
        if !writer.open("OOM_FRAME") {
            break;
        }
        writer.kv_dec("i", index);
        writer.kv_hex("pc", pc);
        if !writer.close() || parent <= frame {
            break;
        }
        frame = parent;
    }
}

/// Handle allocation errors
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    #[cfg(target_arch = "x86_64")]
    {
        use core::sync::atomic::AtomicBool;

        // A secondary OOM must leave the first CPU's allocation record and
        // general panic capture readable.
        static REPORTING: AtomicBool = AtomicBool::new(false);
        if REPORTING.swap(true, Ordering::AcqRel) {
            x86_64::instructions::interrupts::disable();
            loop {
                x86_64::instructions::hlt();
            }
        }
        x86_64::instructions::interrupts::without_interrupts(|| report_oom(layout));
    }
    panic!("allocation error: {:?}", layout)
}
