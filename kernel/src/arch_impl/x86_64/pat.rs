//! The page attribute table (Intel SDM Vol. 3A, "Page Attribute Table (PAT)").
//!
//! Each CPU programs the same layout from its per-CPU init, before it is
//! marked online and so before it can run anything that touches a mapping
//! that selects write-combining. Entries 0, 2 and 3 keep their power-up types
//! (write-back, UC-, uncacheable), the ones the kernel's other mappings select
//! (no cache bits, PCD alone, or PCD and PWT as `memory::map_mmio` sets);
//! entry 1, power-up write-through, which PWT alone selects and the kernel does
//! not use, becomes write-combining. Entries 4-7 keep their power-up types.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::Msr;

const IA32_PAT: u32 = 0x277;

const UC: u64 = 0x00;
const WC: u64 = 0x01;
const WT: u64 = 0x04;
const WB: u64 = 0x06;
const UC_MINUS: u64 = 0x07;

/// Entries 0 to 7, one byte each.
pub const LAYOUT: u64 =
    WB | WC << 8 | UC_MINUS << 16 | UC << 24 | WB << 32 | WT << 40 | UC_MINUS << 48 | UC << 56;

/// The entry write-combining mappings select: PWT set, PCD and PAT clear.
pub const WRITE_COMBINING_ENTRY: usize = 1;

/// Bit `n` set once CPU `n` read `LAYOUT` back after programming it.
static PROGRAMMED: AtomicU64 = AtomicU64::new(0);

/// CPUID.01H:EDX bit 16.
pub fn supported() -> bool {
    // SAFETY: CPUID leaf 1 exists on x86_64 processors.
    unsafe { core::arch::x86_64::__cpuid(1).edx & (1 << 16) != 0 }
}

/// Program `LAYOUT` on the executing CPU, logical CPU `cpu`, with interrupts
/// masked. Follows the SDM's sequence for changing a memory type: caches in
/// no-fill mode and flushed, TLBs flushed (global entries included) before and
/// after the write.
pub fn program(cpu: usize) {
    if !supported() {
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        // SAFETY: CR0.CD with NW clear is no-fill cache mode; WBINVD writes the
        // caches back; toggling CR4.PGE flushes the TLBs, global entries
        // included. The PAT MSR exists (CPUID). Interrupts are masked between
        // the steps.
        unsafe {
            let cr0: u64;
            let cr4: u64;
            core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
            let no_fill = (cr0 | 1 << 30) & !(1 << 29);
            core::arch::asm!("mov cr0, {}", in(reg) no_fill, options(nostack, preserves_flags));
            core::arch::asm!("wbinvd", options(nostack, preserves_flags));
            flush_tlb(cr4);
            Msr::new(IA32_PAT).write(LAYOUT);
            core::arch::asm!("wbinvd", options(nostack, preserves_flags));
            flush_tlb(cr4);
            core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
            if Msr::new(IA32_PAT).read() == LAYOUT && cpu < 64 {
                PROGRAMMED.fetch_or(1 << cpu, Ordering::AcqRel);
            }
        }
    });
}

/// Flush the TLBs, global entries included: clear and restore CR4.PGE when
/// it is set, otherwise reload CR3.
///
/// # Safety
/// `cr4` is the executing CPU's CR4.
unsafe fn flush_tlb(cr4: u64) {
    const PGE: u64 = 1 << 7;
    if cr4 & PGE != 0 {
        core::arch::asm!("mov cr4, {}", in(reg) cr4 & !PGE, options(nostack, preserves_flags));
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    } else {
        core::arch::asm!("mov {0}, cr3", "mov cr3, {0}", out(reg) _, options(nostack, preserves_flags));
    }
}

/// After page-table entries changed memory type on the executing CPU: write
/// back and invalidate the caches, then flush the TLBs, global entries
/// included.
pub fn flush_after_type_change() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        // SAFETY: WBINVD and a TLB flush of this CPU, with its own CR4.
        unsafe {
            let cr4: u64;
            core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
            core::arch::asm!("wbinvd", options(nostack, preserves_flags));
            flush_tlb(cr4);
        }
    });
}

/// The CPUs, as a bit mask, that hold `LAYOUT`.
pub fn programmed_mask() -> u64 {
    PROGRAMMED.load(Ordering::Acquire)
}
