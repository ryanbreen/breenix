#[cfg(not(target_arch = "x86_64"))]
use crate::memory::arch_stub::{OffsetPageTable, VirtAddr};
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;

use super::kmem;

use super::heap_index::Heap;

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
    /// The pool's address range, set once at `init`.
    start: AtomicUsize,
    end: AtomicUsize,
}

impl IrqSafeLockedHeap {
    const fn empty() -> Self {
        Self {
            inner: Mutex::new(Inner {
                heap: Heap::empty(),
                counts: case::Counts::new(),
            }),
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(0),
        }
    }

    /// One heap operation if the mutex is free now, with interrupts masked
    /// for the hold, as `with_inner` holds it.
    #[inline(always)]
    fn try_inner<R>(&self, operation: impl FnOnce(&mut Heap) -> (R, u32)) -> Option<R> {
        crate::arch_without_interrupts(|| {
            self.inner
                .try_lock()
                .map(|mut inner| inner.run(case::LockWait::new(), operation))
        })
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
        self.start.store(heap_bottom as usize, Ordering::Relaxed);
        self.end.store(heap_bottom as usize + heap_size, Ordering::Relaxed);
    }
}

/// The most bytes in use at once since boot in the main heap (the reserve is
/// not counted), as the allocator counts them, including block headers and
/// alignment padding.
static HIGH_WATER: AtomicUsize = AtomicUsize::new(0);

impl kmem::Pool for IrqSafeLockedHeap {
    fn take(&self, layout: Layout, wait: bool) -> Result<NonNull<u8>, kmem::Miss> {
        let operation = |heap: &mut Heap| {
            let (ptr, walk) = heap.allocate(layout);
            let used = heap.used();
            if ptr.is_some() && used > HIGH_WATER.load(Ordering::Relaxed) {
                HIGH_WATER.store(used, Ordering::Relaxed);
            }
            (ptr, walk)
        };
        let ptr = if wait {
            self.with_inner(operation)
        } else {
            self.try_inner(operation).ok_or(kmem::Miss::Busy)?
        };
        ptr.ok_or(kmem::Miss::Exhausted)
    }

    fn contains(&self, ptr: NonNull<u8>) -> bool {
        let address = ptr.as_ptr() as usize;
        address >= self.start.load(Ordering::Relaxed) && address < self.end.load(Ordering::Relaxed)
    }

    unsafe fn give(&self, ptr: NonNull<u8>) {
        self.with_inner(|heap| unsafe {
            heap.deallocate(ptr);
            ((), 0)
        });
    }

    unsafe fn resize(&self, ptr: NonNull<u8>, new_size: usize) -> bool {
        self.with_inner(|heap| unsafe {
            let resized = heap.resize_in_place(ptr, new_size);
            let used = heap.used();
            if resized && used > HIGH_WATER.load(Ordering::Relaxed) {
                HIGH_WATER.store(used, Ordering::Relaxed);
            }
            (resized, 0)
        })
    }
}

/// The kernel's allocator: requests up to a page go to the slab caches and
/// their per-CPU magazines (E1-6), larger ones to the fixed kernel heap
/// until E1-9. At the heap's top is the emergency reserve that only `Atomic`
/// requests use (5.3). The heap and the reserve each have their own mutex,
/// so an interrupt handler that finds the heap's mutex held still has the
/// reserve.
struct KernelHeap {
    main: IrqSafeLockedHeap,
    reserve: IrqSafeLockedHeap,
}

impl KernelHeap {
    fn front(&self) -> kmem::Front<'_, IrqSafeLockedHeap, IrqSafeLockedHeap, kmem::KernelSmall> {
        kmem::Front {
            main: &self.main,
            reserve: &self.reserve,
            small: &kmem::KernelSmall,
            shrinkers: &kmem::SHRINKERS,
            reclaim: &kmem::RECLAIM,
            stats: &kmem::STATS,
        }
    }
}

/// Allocate from the kernel heap through the front, in `ctx`.
pub(super) fn allocate(layout: Layout, ctx: kmem::Ctx) -> Result<NonNull<u8>, kmem::AllocError> {
    ALLOCATOR.front().allocate(layout, ctx, &kmem::KernelCpu)
}

/// The slow path for `frames` frames the frame allocator could not find
/// (`kmem::Front::reclaim_frames`). Returns the frames given back.
pub(super) fn reclaim_frames(frames: usize) -> usize {
    ALLOCATOR.front().reclaim_frames(frames, &kmem::KernelCpu)
}

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.front()
            .allocate(layout, kmem::Ctx::Normal, &kmem::KernelCpu)
            .map_or(core::ptr::null_mut(), NonNull::as_ptr)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.front().deallocate(NonNull::new_unchecked(ptr), layout) }
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
        // A block that keeps its place (6.6) is freed later with the new
        // layout by the layer that holds it.
        unsafe {
            self.front()
                .reallocate(NonNull::new_unchecked(ptr), layout, new_size, &kmem::KernelCpu)
                .map_or(core::ptr::null_mut(), NonNull::as_ptr)
        }
    }
}

/// Global allocator over the boot-reserved kernel heap.
#[global_allocator]
static ALLOCATOR: KernelHeap = KernelHeap {
    main: IrqSafeLockedHeap::empty(),
    reserve: IrqSafeLockedHeap::empty(),
};

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

    // The reserve is the top of the region, so the main heap starts where it
    // did and the region as a whole is unchanged.
    let reserve_bytes = (kmem::reserve_frames(heap_size as usize / 4096) * 4096) as u64;
    let main_bytes = heap_size - reserve_bytes;
    unsafe {
        ALLOCATOR.main.init(heap_base as *mut u8, main_bytes as usize);
        ALLOCATOR
            .reserve
            .init((heap_base + main_bytes) as *mut u8, reserve_bytes as usize);
    }

    kmem::init_slab_layer();

    log::info!(
        "Heap initialized at {:#x} with size {} KiB ({} KiB emergency reserve)",
        heap_base,
        heap_size / 1024,
        reserve_bytes / 1024
    );

    Ok(())
}

/// Bytes free in the kernel heap, its emergency reserve included. Physical
/// frames are not heap and are counted by the frame allocator instead.
pub fn free_bytes() -> usize {
    ALLOCATOR.main.with_inner(|heap| (heap.free(), 0))
        + ALLOCATOR.reserve.with_inner(|heap| (heap.free(), 0))
}

/// The most bytes the kernel heap has had in use at once since boot.
pub fn high_water_bytes() -> usize {
    HIGH_WATER.load(Ordering::Relaxed)
}

/// The pool's free bytes, free extents and largest extent, if its mutex is
/// free now.
fn pool_shape(pool: &IrqSafeLockedHeap) -> Option<(usize, u32, u32)> {
    let guard = pool.inner.try_lock()?;
    let free = guard.heap.free();
    let (extents, largest) = guard.heap.free_shape();
    Some((free, extents, largest))
}

/// Print the failing allocation, the slow path's state and, on x86-64, its
/// callers, before the general panic handler. The heap and serial paths may
/// already be under contention.
#[cold]
fn report_oom(layout: Layout) {
    use crate::capture::record::Writer;
    use crate::per_cpu;
    use core::sync::atomic::Ordering::Relaxed;

    let mut writer = Writer::new();
    if !writer.open("OOM") {
        return;
    }
    writer.kv_dec("size", layout.size() as u64);
    writer.kv_dec("align", layout.align() as u64);
    if let Some((cause, ctx)) = kmem::STATS.last_failure() {
        writer.kv_text("cause", cause.name());
        writer.kv_text(
            "ctx",
            match ctx {
                kmem::Ctx::Normal => "normal",
                kmem::Ctx::NoReclaim => "noreclaim",
                kmem::Ctx::Atomic => "atomic",
            },
        );
    }
    writer.kv_dec("high_water", high_water_bytes() as u64);
    if let Some((free, extents, largest)) = pool_shape(&ALLOCATOR.main) {
        writer.kv_dec("free", free as u64);
        writer.kv_dec("extents", u64::from(extents));
        writer.kv_dec("largest", u64::from(largest));
    }
    if let Some((free, _, largest)) = pool_shape(&ALLOCATOR.reserve) {
        writer.kv_dec("reserve_free", free as u64);
        writer.kv_dec("reserve_largest", u64::from(largest));
    }
    writer.kv_dec("reclaim_runs", kmem::STATS.reclaim_runs.load(Relaxed));
    writer.kv_dec("reclaim_frames", kmem::STATS.reclaim_frames.load(Relaxed));
    writer.kv_dec("frames_frontier", super::frame_allocator::frontier_frames() as u64);
    if !per_cpu::is_initialized() {
        let _ = writer.close();
        return;
    }
    writer.kv_dec("cpu", kmem::cpu_number() as u64);
    #[cfg(target_arch = "x86_64")]
    let tid = per_cpu::current_thread_id_lock_free();
    #[cfg(target_arch = "aarch64")]
    let tid = per_cpu::current_thread().map(|thread| thread.id);
    if let Some(tid) = tid {
        writer.kv_dec("tid", tid);
    }
    // The matching ELF can use this symbol address to account for relocation.
    writer.kv_hex("anchor", alloc_error_handler as *const () as u64);
    if !writer.close() {
        return;
    }
    // The ARM64 kernel is not built with forced frame pointers, so only x86-64
    // can follow its callers' frame links.
    #[cfg(target_arch = "x86_64")]
    report_oom_frames(writer);
}

#[cfg(target_arch = "x86_64")]
#[cold]
fn report_oom_frames(mut writer: crate::capture::record::Writer) {
    use crate::per_cpu;

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
    use core::sync::atomic::AtomicBool;

    // A secondary OOM must leave the first CPU's allocation record and
    // general panic capture readable.
    static REPORTING: AtomicBool = AtomicBool::new(false);
    if REPORTING.swap(true, Ordering::AcqRel) {
        loop {
            // SAFETY: this CPU stops here for good.
            unsafe { crate::arch_disable_interrupts() };
            #[cfg(target_arch = "x86_64")]
            x86_64::instructions::hlt();
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("wfe", options(nomem, nostack));
            }
        }
    }
    crate::arch_without_interrupts(|| report_oom(layout));
    panic!("allocation error: {:?}", layout)
}
