use crate::exports::{__brt_tf_add, __brt_tf_div, __brt_tf_to_f32};
use core::arch::asm;

#[test]
fn exports_follow_fpcr_and_fpsr() {
    // Arm DDI 0487 M.b §§C5.2.8, C5.2.10: RMode, QC, IXC and DZC.
    const RMODE: u64 = 3 << 22;
    const QC: u64 = 1 << 27;
    const IXC: u64 = 1 << 4;
    const DZC: u64 = 1 << 1;
    const ONE: u128 = 16383 << 112;
    const HALF_ULP: u128 = (16383 - 113) << 112;
    const F32_HALF_ULP: u128 = 1 << (112 - 24);
    const INFINITY: u128 = 0x7fff << 112;

    let saved_fpcr: u64;
    let saved_fpsr: u64;
    // SAFETY: Reading FPCR reports the current thread's controls.
    unsafe {
        asm!("mrs {value}, fpcr", value = out(reg) saved_fpcr, options(nostack, preserves_flags))
    };
    // SAFETY: Reading FPSR reports the current thread's cumulative status.
    unsafe {
        asm!("mrs {value}, fpsr", value = out(reg) saved_fpsr, options(nostack, preserves_flags))
    };
    for (mode, expected, expected_narrow) in [
        (0, ONE, 0x3f80_0000),
        (1, ONE + 1, 0x3f80_0001),
        (2, ONE, 0x3f80_0000),
        (3, ONE, 0x3f80_0000),
    ] {
        // SAFETY: Replace only RMode on this thread and restore it before asserting.
        unsafe {
            asm!("msr fpcr, {value}", value = in(reg) ((saved_fpcr & !RMODE) | (mode << 22)), options(nostack, preserves_flags))
        };
        // SAFETY: Seed writable cumulative status on this thread for the test.
        unsafe { asm!("msr fpsr, {value}", value = in(reg) QC, options(nostack, preserves_flags)) };
        let quotient = __brt_tf_div(ONE, 0);
        let sum = __brt_tf_add(ONE, HALF_ULP);
        let status: u64;
        // SAFETY: Reading FPSR observes the effects of the two calls above.
        unsafe {
            asm!("mrs {value}, fpsr", value = out(reg) status, options(nostack, preserves_flags))
        };
        // SAFETY: Clear the test thread's status to observe narrowing on its own.
        unsafe { asm!("msr fpsr, xzr", options(nostack, preserves_flags)) };
        let narrow = __brt_tf_to_f32(ONE + F32_HALF_ULP);
        let narrow_status: u64;
        // SAFETY: Reading FPSR observes the effect of the narrowing call.
        unsafe {
            asm!("mrs {value}, fpsr", value = out(reg) narrow_status, options(nostack, preserves_flags))
        };
        // SAFETY: Restore the FPCR value read from this thread.
        unsafe {
            asm!("msr fpcr, {value}", value = in(reg) saved_fpcr, options(nostack, preserves_flags))
        };
        // SAFETY: Restore the FPSR value read from this thread.
        unsafe {
            asm!("msr fpsr, {value}", value = in(reg) saved_fpsr, options(nostack, preserves_flags))
        };
        assert_eq!(
            (sum, quotient, status),
            (expected, INFINITY, QC | IXC | DZC),
            "RMode {mode}"
        );
        assert_eq!(
            (narrow, narrow_status),
            (expected_narrow, IXC),
            "RMode {mode}"
        );
    }
}
