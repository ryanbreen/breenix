//! The executing CPU's EL0 FP/SIMD registers, for signal frames.
//!
//! The kernel is built soft-float and executes no FP/SIMD instruction of its
//! own, so v0-v31, FPSR and FPCR hold the user state of the thread that last
//! ran at EL0 on this CPU. CPACR_EL1.FPEN permits EL1 access (boot.S), which
//! is how these routines read and write them; the assembler is told the FP
//! and SIMD extensions are present for these instructions only.

use crate::signal::types::FpsimdContext;

/// FPSR bits: the cumulative exception flags and the NZCV/QC comparison flags.
const FPSR_MASK: u32 = 0xf800_009f;
/// FPCR bits: AHP, DN, FZ, RMode, FZ16, the trap enables, NEP, AH and FIZ.
const FPCR_MASK: u32 = 0x07c8_9f07;

/// Store this CPU's v0-v31, FPSR and FPCR in `ctx`.
pub fn save(ctx: &mut FpsimdContext) {
    let fpsr: u64;
    let fpcr: u64;
    // SAFETY: stores 512 bytes to `ctx.vregs`, which is 16-byte aligned and
    // that size, and reads two system registers.
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "stp q0, q1, [{v}, #0]",
            "stp q2, q3, [{v}, #32]",
            "stp q4, q5, [{v}, #64]",
            "stp q6, q7, [{v}, #96]",
            "stp q8, q9, [{v}, #128]",
            "stp q10, q11, [{v}, #160]",
            "stp q12, q13, [{v}, #192]",
            "stp q14, q15, [{v}, #224]",
            "stp q16, q17, [{v}, #256]",
            "stp q18, q19, [{v}, #288]",
            "stp q20, q21, [{v}, #320]",
            "stp q22, q23, [{v}, #352]",
            "stp q24, q25, [{v}, #384]",
            "stp q26, q27, [{v}, #416]",
            "stp q28, q29, [{v}, #448]",
            "stp q30, q31, [{v}, #480]",
            "mrs {fpsr}, fpsr",
            "mrs {fpcr}, fpcr",
            v = in(reg) ctx.vregs.as_mut_ptr(),
            fpsr = out(reg) fpsr,
            fpcr = out(reg) fpcr,
            options(nostack, preserves_flags),
        );
    }
    ctx.fpsr = fpsr as u32;
    ctx.fpcr = fpcr as u32;
}

/// Load this CPU's v0-v31, FPSR and FPCR from `ctx`. FPSR and FPCR bits
/// with no defined meaning are cleared.
pub fn restore(ctx: &FpsimdContext) {
    let fpsr = (ctx.fpsr & FPSR_MASK) as u64;
    let fpcr = (ctx.fpcr & FPCR_MASK) as u64;
    // SAFETY: loads 512 bytes from `ctx.vregs`, which is 16-byte aligned and
    // that size, and writes the FP/SIMD registers, which kernel code never
    // uses.
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "ldp q0, q1, [{v}, #0]",
            "ldp q2, q3, [{v}, #32]",
            "ldp q4, q5, [{v}, #64]",
            "ldp q6, q7, [{v}, #96]",
            "ldp q8, q9, [{v}, #128]",
            "ldp q10, q11, [{v}, #160]",
            "ldp q12, q13, [{v}, #192]",
            "ldp q14, q15, [{v}, #224]",
            "ldp q16, q17, [{v}, #256]",
            "ldp q18, q19, [{v}, #288]",
            "ldp q20, q21, [{v}, #320]",
            "ldp q22, q23, [{v}, #352]",
            "ldp q24, q25, [{v}, #384]",
            "ldp q26, q27, [{v}, #416]",
            "ldp q28, q29, [{v}, #448]",
            "ldp q30, q31, [{v}, #480]",
            "msr fpsr, {fpsr}",
            "msr fpcr, {fpcr}",
            v = in(reg) ctx.vregs.as_ptr(),
            fpsr = in(reg) fpsr,
            fpcr = in(reg) fpcr,
            options(nostack, preserves_flags, readonly),
        );
    }
}
