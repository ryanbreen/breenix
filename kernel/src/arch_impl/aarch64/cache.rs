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
