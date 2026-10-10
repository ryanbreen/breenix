//! Cache maintenance for pages executed through user aliases.

/// Make a page just written through HHDM safe to execute from a user VA:
/// clean its data cache lines to the point of unification, then invalidate
/// every CPU's instruction cache. Like the ELF loader, this invalidates the
/// whole I-cache, because per-line `ic ivau` on the HHDM alias need not hit
/// the user VA's sets on a VIPT I-cache.
///
/// # Safety
/// `page_va` must be the page-aligned HHDM address of a mapped frame.
pub(crate) unsafe fn sync_user_page(page_va: u64) {
    let ctr: u64;
    core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack));
    let line = 4u64 << ((ctr >> 16) & 0xF);
    let mut addr = page_va;
    while addr < page_va + 4096 {
        core::arch::asm!("dc cvau, {}", in(reg) addr, options(nostack, preserves_flags));
        addr += line;
    }
    core::arch::asm!(
        "dsb ish",
        "ic ialluis",
        "dsb ish",
        "isb",
        options(nostack, preserves_flags)
    );
}

/// SCTLR_EL1 bits that let EL0 read CTR_EL0 (UCT) and run DC CVAU, DC CVAC,
/// DC CIVAC and IC IVAU (UCI) instead of trapping to EL1.
const SCTLR_UCT: u64 = 1 << 15;
const SCTLR_UCI: u64 = 1 << 26;

/// Let EL0 make code it wrote executable on the calling CPU, as Linux does: a
/// JIT or a C library's `__clear_cache` reads the cache line sizes from
/// CTR_EL0, then cleans each data line with DC CVAU and invalidates each
/// instruction line with IC IVAU. SCTLR_EL1 is per CPU and the bits reset to
/// trapping, so every CPU runs this as it comes up.
pub fn grant_el0_cache_maintenance() {
    unsafe {
        let mut sctlr: u64;
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack));
        sctlr |= SCTLR_UCT | SCTLR_UCI;
        core::arch::asm!("msr sctlr_el1, {}", "isb", in(reg) sctlr, options(nomem, nostack));
    }
}
