//! Fixtures for the usable CPU capability words.
#[path = "../libs/libbreenix/src/auxv.rs"]
mod auxv;
#[path = "../kernel/src/arch_impl/aarch64/user_cpu.rs"]
mod arm;
#[path = "../kernel/src/arch_impl/x86_64/xstate_layout.rs"]
mod x86;

#[test]
fn x86_flags_need_compatible_xcr0_and_independent_instruction_bits() {
    use x86::{AVX, AVX512, BASELINE};
    let b = (1 << 5) | (1 << 9) | (1 << 16);
    let c = (1 << 1) | (1 << 25) | (1 << 28);
    assert_eq!(x86::capabilities(1 << 26, c, b, 1 << 4, false, BASELINE), [7, 3]);
    assert_eq!(x86::capabilities(1 << 26, c, b, 0, true, BASELINE), [7, 1]);
    assert_eq!(x86::capabilities(1 << 26, c, b, 0, true, BASELINE | AVX), [31, 1]);
    assert_eq!(x86::capabilities(1 << 26, c, b, 0, true, BASELINE | AVX | AVX512), [63, 1]);
    assert_eq!(x86::capabilities(0, 1 << 25, 0, 1 << 4, false, BASELINE), [2, 2]);
    assert_eq!(x86::capabilities(0, 1 << 1, 0, 0, false, BASELINE), [4, 0]);
}

#[test]
fn arm_aes_and_pmull_are_distinct_and_admission_freezes_the_mask() {
    assert_eq!(arm::capabilities(0, 1 << 4, true), 0x0b);
    assert_eq!(arm::capabilities(0, 2 << 4, true), 0x1b);
    assert_eq!(arm::capabilities(0xf << 20, (1 << 16) | (2 << 4), true), 0x81);
    assert_eq!(arm::capabilities(0, (1 << 16) | (2 << 4), false), 0x80);
    let mask = arm::CommonCaps::new();
    assert!(mask.admit(0x1b));
    assert!(mask.admit(0x0b));
    assert_eq!(mask.publish(), 0x0b);
    assert!(!mask.admit(0x03));
    assert!(mask.admit(0x1b));
    assert_eq!(mask.publish(), 0x0b);
}
