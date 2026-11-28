//! The kernel side of allocation contexts and reclaim (E1 plan, 6.6 and 7):
//! the shrinker registry, the explicit fallible allocation API, and the
//! counts `/proc/breenix/kmem` and the OOM record read. The algorithms are in
//! `kmem_core.rs`, which the host tests include.
//!
//! It is also the glue of the slab layer (6.2, 6.3, 5.4a): slab pages are
//! frames reached through the direct map, each frame's `FrameMeta` names its
//! slab, and each CPU registers its magazines in a slot table sized from the
//! CPU count the firmware describes. The algorithms are `slab_cache.rs` and
//! `magazine.rs`.
use core::alloc::Layout;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use super::magazine::{CpuCaches, Kmem};
use super::slab_cache::{class_of, Backing, Short, Slab, SlabCache, CLASSES, CLASS_COUNT};

#[path = "kmem_core.rs"]
mod core_impl;
pub use core_impl::{moved_layer, route, AllocError, Ctx, Drain, Layer, ReclaimCtx, Shrinker};
pub(super) use core_impl::{reserve_frames, Cpu, Front, Miss, Pool, Reclaim, Registry, Small, Stats};

pub(super) static SHRINKERS: Registry = Registry::new();
pub(super) static RECLAIM: Reclaim = Reclaim::new();
pub(super) static STATS: Stats = Stats::new();

/// Page faults that found no frame to back the page (`FaultOutcome::OutOfMemory`).
static FAULT_OUT_OF_MEMORY: AtomicU64 = AtomicU64::new(0);
/// CPUs whose magazines are published, and CPUs that stayed on the shared
/// path because their slot or their block could not be had.
static CPUS_REGISTERED: AtomicU64 = AtomicU64::new(0);
static CPUS_UNREGISTERED: AtomicU64 = AtomicU64::new(0);

/// Add a reclaim hook. Shrinkers run in cost order from the allocator's slow
/// path, and must follow `Shrinker`'s rules.
pub fn register_shrinker(shrinker: &'static Shrinker) {
    SHRINKERS.register(shrinker);
}

/// Add a step-2 drain (7.2): a layer's per-CPU frames, given back from each
/// CPU with `try_lock` before the shrinkers run.
pub fn register_drain(drain: &'static Drain) {
    SHRINKERS.register_drain(drain);
}

/// Allocate `layout` in `ctx`, which the allocator lowers in interrupt context
/// and inside reclaim. A zero-size layout is refused.
pub fn try_alloc(layout: Layout, ctx: Ctx) -> Result<NonNull<u8>, AllocError> {
    if layout.size() == 0 {
        return Err(AllocError::Refused);
    }
    super::heap::allocate(layout, ctx)
}

/// The executing CPU's number. ARM64 reads MPIDR_EL1 before TPIDR_EL1 is
/// set; x86-64 is the boot CPU, 0, before its per-CPU data exists.
pub(super) fn cpu_number() -> usize {
    #[cfg(target_arch = "aarch64")]
    return crate::arch_impl::aarch64::percpu::Aarch64PerCpu::cpu_id() as usize;
    #[cfg(target_arch = "x86_64")]
    return if crate::per_cpu::is_initialized() { crate::per_cpu::cpu_id() } else { 0 };
}

/// The executing CPU's logical number from its own per-CPU data. On ARM64 a
/// CPU names itself once `per_cpu_aarch64::init_cpu` points its TPIDR_EL1 at
/// its entry of the per-CPU array, and on x86-64 once the boot CPU's per-CPU
/// data exists; before that it takes the slab layer's shared path (5.4a).
/// TPIDR_EL1's reset value is UNKNOWN, so on ARM64 the value is compared with
/// the array's entries and not read through (`cpu_of_base`).
pub(super) fn named_cpu() -> Option<usize> {
    #[cfg(target_arch = "aarch64")]
    return {
        let (first, stride, count) = crate::per_cpu_aarch64::cpu_data_array();
        core_impl::cpu_of_base(crate::arch_impl::aarch64::percpu::percpu_base(), first, stride, count)
    };
    #[cfg(target_arch = "x86_64")]
    return crate::per_cpu::is_initialized().then(crate::per_cpu::cpu_id);
}

/// The executing CPU, as the allocator's front sees it.
pub(super) struct KernelCpu;

impl Cpu for KernelCpu {
    fn in_interrupt(&self) -> bool {
        crate::per_cpu::is_initialized() && crate::per_cpu::in_interrupt()
    }

    fn id(&self) -> usize {
        cpu_number()
    }

    fn masked<R>(&self, f: impl FnOnce() -> R) -> R {
        crate::arch_without_interrupts(f)
    }
}

#[cfg(not(target_arch = "x86_64"))]
use super::arch_stub::{PhysAddr, PhysFrame};
#[cfg(target_arch = "x86_64")]
use x86_64::{structures::paging::PhysFrame, PhysAddr};

/// The slab layer's pages: frames through the direct map.
pub(super) struct KernelPages;

/// The frame behind a direct-map page address.
fn frame_of(page: NonNull<u8>) -> Option<PhysFrame> {
    let physical = (page.as_ptr() as u64).checked_sub(super::direct_map_offset()?)?;
    Some(PhysFrame::containing_address(PhysAddr::new(physical)))
}

impl Backing for KernelPages {
    fn take_page(&self, wait: bool) -> Result<NonNull<u8>, Short> {
        use super::frame_allocator::SlabFrameMiss;
        let offset = super::direct_map_offset().ok_or(Short::NoPage)?;
        let frame = super::frame_allocator::allocate_slab_frame(wait).map_err(|miss| match miss {
            SlabFrameMiss::Busy => Short::Busy,
            SlabFrameMiss::Exhausted => Short::NoPage,
        })?;
        NonNull::new((offset + frame.start_address().as_u64()) as *mut u8).ok_or(Short::NoPage)
    }

    fn give_page(&self, page: NonNull<u8>) {
        if let Some(frame) = frame_of(page) {
            super::frame_allocator::deallocate_slab_frame(frame);
        }
    }

    fn set_slab(&self, page: NonNull<u8>, slab: *mut Slab) {
        if let Some(entry) = frame_of(page).and_then(super::frame_allocator::frame_entry) {
            entry.set_slab(slab as u64);
        }
    }

    fn slab(&self, page: NonNull<u8>) -> *mut Slab {
        frame_of(page)
            .and_then(super::frame_allocator::frame_entry)
            .map_or(core::ptr::null_mut(), |entry| entry.slab() as *mut Slab)
    }

    fn now_ms(&self) -> u64 {
        crate::time::get_monotonic_time()
    }
}

/// The size classes, their depots and the per-CPU magazines.
static KMEM: Kmem<KernelPages> = Kmem::new(KernelPages);

fn miss(short: Short) -> Miss {
    match short {
        Short::Busy => Miss::Busy,
        Short::NoPage => Miss::Exhausted,
    }
}

/// The slab layer as the allocator's front reaches it. Each operation masks
/// this CPU's interrupts and reads the CPU's identity inside the masked
/// region, so it neither migrates nor reenters its own lock (6.3).
pub(super) struct KernelSmall;

impl Small for KernelSmall {
    #[inline]
    fn serves(&self, layout: Layout) -> bool {
        class_of(layout.size(), layout.align()).is_some()
    }

    #[inline]
    fn take(&self, layout: Layout, wait: bool) -> Result<NonNull<u8>, Miss> {
        let class = class_of(layout.size(), layout.align()).ok_or(Miss::Exhausted)?;
        crate::arch_without_interrupts(|| KMEM.alloc(class, named_cpu(), wait)).map_err(miss)
    }

    #[inline]
    unsafe fn give(&self, ptr: NonNull<u8>, layout: Layout) {
        let Some(class) = class_of(layout.size(), layout.align()) else {
            panic!(
                "slab: free of {:#x} with a {}-byte layout aligned to {}, which no size class serves",
                ptr.as_ptr() as usize,
                layout.size(),
                layout.align()
            );
        };
        crate::arch_without_interrupts(|| KMEM.free(ptr, class, named_cpu()));
    }

    fn same_object(&self, old: Layout, new: Layout) -> bool {
        let old = class_of(old.size(), old.align());
        old.is_some() && old == class_of(new.size(), new.align())
    }

    fn drain_own(&self, wait: bool) {
        crate::arch_without_interrupts(|| {
            if let Some(cpu) = named_cpu() {
                KMEM.drain(cpu, wait);
            }
        });
    }

    fn drain(&self, cpu: usize) -> bool {
        crate::arch_without_interrupts(|| KMEM.drain(cpu, false))
    }

    fn cpus(&self) -> usize {
        KMEM.slot_count()
    }

    fn release_freed(&self, target: usize) -> usize {
        crate::arch_without_interrupts(|| {
            if let Some(cpu) = named_cpu() {
                KMEM.drain(cpu, false);
            }
            KMEM.shrink(target) + super::slab::release_empty()
        })
    }
}

/// Install the per-CPU slot table for the `cpus` CPUs the firmware
/// describes (5.4a): one null slot each, from the shared path. Called once,
/// on the boot CPU, before another CPU starts. If the table cannot be had,
/// the CPUs stay on the shared path, which is correct and slower.
pub fn init_cpu_slots(cpus: usize) {
    let count = cpus.max(1);
    let Ok(layout) = Layout::array::<AtomicPtr<CpuCaches>>(count) else {
        return;
    };
    let Ok(table) = try_alloc(layout, Ctx::Normal) else {
        CPUS_UNREGISTERED.fetch_add(count as u64, Ordering::Relaxed);
        return;
    };
    let table = table.cast::<AtomicPtr<CpuCaches>>().as_ptr();
    // SAFETY: the block holds `count` slots and no code frees it.
    let slots = unsafe {
        for index in 0..count {
            table.add(index).write(AtomicPtr::new(core::ptr::null_mut()));
        }
        core::slice::from_raw_parts(table, count)
    };
    KMEM.install_slots(slots);
    super::large::init_cpu_arenas(count);
}

/// Build this CPU's magazines from the shared path and publish them in its
/// slot (5.4a). The CPU calls this once its per-CPU id is readable and
/// before it enables interrupts. A CPU with no slot, or whose block cannot
/// be had, stays on the shared path.
pub fn register_cpu() {
    let registered = crate::arch_without_interrupts(|| {
        let cpu = named_cpu()?;
        if cpu >= KMEM.slot_count() {
            return None;
        }
        let block = try_alloc(Layout::new::<CpuCaches>(), Ctx::Normal).ok()?.cast::<CpuCaches>();
        // SAFETY: the block is sized and aligned for `CpuCaches`, and a
        // published block lives for the kernel's lifetime.
        let caches: &'static CpuCaches = unsafe {
            block.as_ptr().write(CpuCaches::new());
            &*block.as_ptr()
        };
        KMEM.register(cpu, caches).then_some(())
    });
    let counter = if registered.is_some() { &CPUS_REGISTERED } else { &CPUS_UNREGISTERED };
    counter.fetch_add(1, Ordering::Relaxed);
}

/// One object of a named cache (memory/slab.rs), from its slabs.
pub(crate) fn named_take(cache: &SlabCache) -> Option<NonNull<u8>> {
    let mut one = [core::ptr::null_mut(); 1];
    crate::arch_without_interrupts(|| cache.take(&mut one, true, KMEM.backing(), KMEM.descriptors())).ok()?;
    NonNull::new(one[0])
}

/// Free an object of a named cache, checked against its slab first.
pub(crate) fn named_put(cache: &SlabCache, object: NonNull<u8>) {
    cache.check(object, KMEM.backing());
    crate::arch_without_interrupts(|| cache.put(&[object.as_ptr()], KMEM.backing(), KMEM.descriptors()));
}

/// Give back a named cache's empty slabs, for the slab shrinker.
pub(crate) fn named_release_empty(cache: &SlabCache) -> usize {
    cache.release_empty(KMEM.backing(), KMEM.descriptors())
}

fn slab_count() -> usize {
    KMEM.reclaimable() + super::slab::reclaimable()
}

fn slab_scan(target: usize, _ctx: ReclaimCtx) -> usize {
    let pages = KMEM.shrink(target);
    pages + super::slab::release_empty()
}

/// The slab shrinker, the cheapest (7.3): depot magazines back to their
/// slabs, then empty slabs back to the page layer.
static SLAB_SHRINKER: Shrinker = Shrinker::new("slab", 10, slab_count, slab_scan);

/// Register the slab shrinker; called once, when the heap is set up.
pub(super) fn init_slab_layer() {
    register_shrinker(&SLAB_SHRINKER);
}

/// One class's counts. Reading the CPUs' magazines takes their locks with
/// `try_lock`, so this CPU's interrupts are masked for the read: an
/// interrupt that allocates must not find this CPU's own lock held.
fn class_stats(class: usize) -> super::magazine::ClassStats {
    crate::arch_without_interrupts(|| KMEM.class_stats(class))
}

/// Pages the slab layer holds: its slabs, descriptors and magazines.
pub fn slab_pages() -> usize {
    (0..CLASS_COUNT).map(|class| class_stats(class).slabs.slabs).sum::<usize>() + KMEM.overhead_pages()
}

/// Count a page fault that could not get a frame. Its process still gets
/// SIGKILL until E3 chooses a victim.
pub fn note_fault_out_of_memory() {
    FAULT_OUT_OF_MEMORY.fetch_add(1, Ordering::Relaxed);
}

/// Lines `write_stats` writes, for a reader that reserves its buffer first:
/// 18 allocator and slab lines, a row per class, the kernel windows' 14 and
/// the span and vmap layers' 4.
pub const STATS_LINES: usize = 18 + CLASS_COUNT + 14 + 4;

/// The slow path's counts, and a row per size class, as `name value` lines.
pub fn write_stats(out: &mut impl core::fmt::Write) -> core::fmt::Result {
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    for (index, ctx) in ["normal", "noreclaim", "atomic"].iter().enumerate() {
        writeln!(out, "alloc_slow_{ctx} {}", load(&STATS.slow[index]))?;
        writeln!(out, "alloc_refused_{ctx} {}", load(&STATS.refused[index]))?;
        writeln!(out, "alloc_failed_{ctx} {}", load(&STATS.failed[index]))?;
    }
    writeln!(out, "alloc_reserve_hits {}", load(&STATS.reserve_hits))?;
    writeln!(out, "reclaim_runs {}", load(&STATS.reclaim_runs))?;
    writeln!(out, "reclaim_frames {}", load(&STATS.reclaim_frames))?;
    writeln!(out, "reclaim_busy {}", load(&STATS.reclaim_busy))?;
    writeln!(out, "fault_out_of_memory {}", load(&FAULT_OUT_OF_MEMORY))?;
    writeln!(out, "slab_cpu_slots {}", KMEM.slot_count())?;
    writeln!(out, "slab_cpus_registered {}", load(&CPUS_REGISTERED))?;
    writeln!(out, "slab_cpus_shared {}", load(&CPUS_UNREGISTERED))?;
    writeln!(out, "slab_overhead_pages {}", KMEM.overhead_pages())?;
    for (class, size) in CLASSES.iter().enumerate() {
        let stats = class_stats(class);
        writeln!(
            out,
            "slab_{size} slabs={} objects={} empty={} cpu_rounds={} depot_rounds={} magazine={}",
            stats.slabs.slabs,
            stats.slabs.objects,
            stats.slabs.empty,
            stats.cpu_rounds,
            stats.depot_rounds,
            stats.magazine_rounds
        )?;
    }
    super::kva::write_stats(out)?;
    super::large::write_stats(out)
}
