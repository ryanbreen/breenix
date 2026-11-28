//! The kernel side of allocation contexts and reclaim (E1 plan, 6.6 and 7):
//! the shrinker registry, the explicit fallible allocation API, and the
//! counts `/proc/breenix/kmem` and the OOM record read. The algorithms are in
//! `kmem_core.rs`, which the host tests include.
use core::alloc::Layout;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};

#[path = "kmem_core.rs"]
mod core_impl;
pub use core_impl::{AllocError, Ctx, ReclaimCtx, Shrinker};
pub(super) use core_impl::{reserve_frames, Cpu, Front, Miss, Pool, Reclaim, Registry, Stats};

pub(super) static SHRINKERS: Registry = Registry::new();
pub(super) static RECLAIM: Reclaim = Reclaim::new();
pub(super) static STATS: Stats = Stats::new();

/// Page faults that found no frame to back the page (`FaultOutcome::OutOfMemory`).
static FAULT_OUT_OF_MEMORY: AtomicU64 = AtomicU64::new(0);

/// Add a reclaim hook. Shrinkers run in cost order from the allocator's slow
/// path, and must follow `Shrinker`'s rules.
pub fn register_shrinker(shrinker: &'static Shrinker) {
    SHRINKERS.register(shrinker);
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

/// Count a page fault that could not get a frame. Its process still gets
/// SIGKILL until E3 chooses a victim.
pub fn note_fault_out_of_memory() {
    FAULT_OUT_OF_MEMORY.fetch_add(1, Ordering::Relaxed);
}

/// The slow path's counts as `name value` lines.
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
    super::kva::write_stats(out)
}
