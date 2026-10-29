//! Address and contiguity admission for the shared GPU DMA boundary.
extern crate alloc;
#[allow(dead_code)] // The host exercises address admission, not DMAR parsing or PTE encoding.
#[path = "../kernel/src/gpu/gk208/dma_policy.rs"]
mod dma_policy;
#[path = "../kernel/src/gpu/arch_rules.rs"]
mod arch_rules;
use arch_rules::{device_span, Architecture::{Arm64, X86_64}};

#[test]
fn arm_uses_translated_device_pages_without_the_kepler_aperture() {
    // The addresses are IPAs after the boot contract's relocation, not kernel VAs.
    assert_eq!(device_span(Arm64, &[0x8000_0000, 0x8000_1000], 64, 4096, &[]), Ok(0x8000_0040));
    assert_eq!(device_span(Arm64, &[1 << 44], 0, 4096, &[]), Ok(1 << 44));
}

#[test]
fn x86_checks_the_entire_span_and_platform_translation() {
    let limit = dma_policy::LIMIT;
    assert_eq!(device_span(X86_64, &[limit - 4096], 64, 4032, &[0]), Ok(limit - 4032));
    assert!(device_span(X86_64, &[limit - 4096, limit], 64, 4096, &[]).is_err());
    assert!(device_span(X86_64, &[4096], 0, 1, &[1 << 31]).is_err());
    assert!(device_span(X86_64, &[0], 0, 1, &[]).is_err());
}

#[test]
fn both_architectures_refuse_gaps_misalignment_overflow_and_short_backing() {
    for arch in [Arm64, X86_64] {
        assert_eq!(device_span(arch, &[4096, 8192], 0, 8192, &[]), Ok(4096));
        for pages in [&[4096, 12288][..], &[4096, 4096], &[4096, 8193], &[4096]] {
            assert!(device_span(arch, pages, 0, 8192, &[]).is_err());
        }
        assert!(device_span(arch, &[], 0, 1, &[]).is_err());
        assert!(device_span(arch, &[4096], 4096, 1, &[]).is_err());
        assert!(device_span(arch, &[4096], 0, 0, &[]).is_err());
        assert!(device_span(arch, &[4096], 1, usize::MAX, &[]).is_err());
        assert!(device_span(arch, &[u64::MAX - 4095, 0], 0, 8192, &[]).is_err());
    }
}
