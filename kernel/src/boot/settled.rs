//! Whether boot's own work is over.
//!
//! On ARM64 the boot thread enters the first program itself, but USB enumeration
//! and GPU console preparation continue on their workers. On x86-64 the boot
//! thread goes on after the first program is published: it starts the console executor and, on bare
//! metal, the tick census, whose per-CPU windows run for a few seconds. A
//! suite's memory record samples once this answers true (`/proc/breenix/frames`,
//! row `boot`; `docs/suites/README.md`, "Memory record"), so the kernel stacks
//! of workers boot starts once are not counted as the suite's.

#[cfg(target_arch = "x86_64")]
use core::sync::atomic::{AtomicBool, Ordering};

/// Set by the x86-64 boot thread once it has started its last worker.
#[cfg(target_arch = "x86_64")]
static TAIL_DONE: AtomicBool = AtomicBool::new(false);

/// The x86-64 boot thread has started its last worker.
#[cfg(target_arch = "x86_64")]
pub fn tail_done() {
    TAIL_DONE.store(true, Ordering::Release);
}

/// Whether boot's work is over: ARM64's device workers have finished their initial
/// allocations; x86-64's tail and tick census, where one runs, have finished.
pub fn settled() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        TAIL_DONE.load(Ordering::Acquire)
            && (crate::drivers::fw_cfg::present() || crate::arch_impl::x86_64::tick_census::reported())
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::drivers::xhci::state() != "enumerating" && crate::gpu::boot_settled()
    }
}
