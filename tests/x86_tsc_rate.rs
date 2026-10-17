//! The TSC rate rule (`kernel/src/arch_impl/x86_64/tsc_rate.rs`), executed:
//! which CPUID leaves state a rate, and when a stated rate replaces the PIT's
//! measurement.

#[path = "../kernel/src/arch_impl/x86_64/tsc_rate.rs"]
mod tsc_rate;

use tsc_rate::{choose, ppm_apart, stated, Leaves, Stated, AGREEMENT_PPM};

/// Synthetic CPUID values, unrelated to any measured machine.
const SYNTHETIC: Leaves = Leaves {
    max_leaf: 0x16,
    ratio_denominator: 3,
    ratio_numerator: 375,
    crystal_hz: 0,
    base_mhz: 3000,
};

#[test]
fn leaf_15h_states_the_rate_when_it_names_the_crystal() {
    let leaves = Leaves { crystal_hz: 24_000_000, ..SYNTHETIC };
    assert_eq!(stated(leaves), Some(Stated { hz: 3_000_000_000, source: "cpuid-15h" }));
    let leaves = Leaves { crystal_hz: 38_400_000, ratio_numerator: 125, ratio_denominator: 2, ..SYNTHETIC };
    assert_eq!(stated(leaves).unwrap().hz, 2_400_000_000);
}

#[test]
fn without_a_crystal_leaf_16h_base_frequency_states_it() {
    assert_eq!(stated(SYNTHETIC), Some(Stated { hz: 3_000_000_000, source: "cpuid-16h" }));
    // Only bits 15:0 of leaf 16H EAX are the base frequency.
    let leaves = Leaves { base_mhz: 0xabcd_0000 | 2400, ..SYNTHETIC };
    assert_eq!(stated(leaves).unwrap().hz, 2_400_000_000);
}

#[test]
fn leaves_the_processor_does_not_implement_state_nothing() {
    assert_eq!(stated(Leaves { max_leaf: 0x14, ..SYNTHETIC }), None, "a leaf-14H processor");
    assert_eq!(
        stated(Leaves { max_leaf: 0x15, crystal_hz: 0, ..SYNTHETIC }),
        None,
        "15H without a crystal, and no 16H"
    );
    assert_eq!(stated(Leaves { base_mhz: 0, ..SYNTHETIC }), None, "16H reporting 0");
    assert_eq!(stated(Leaves::default()), None);
}

#[test]
fn a_stated_rate_is_used_only_when_the_pit_agrees() {
    let cpuid = stated(SYNTHETIC);
    // A synthetic measurement ten ppm above the candidate.
    assert_eq!(ppm_apart(3_000_000_000, 3_000_030_000), 9);
    assert_eq!(choose(3_000_030_000, cpuid), cpuid.unwrap());
    // At the bound it is still taken; past it the measurement is.
    let at_bound = 3_000_000_000 * 1_000_000 / (1_000_000 - AGREEMENT_PPM) - 1;
    assert_eq!(choose(at_bound, cpuid).source, "cpuid-16h");
    assert_eq!(choose(3_001_500_000, cpuid).source, "pit", "a 500 ppm discrepancy is rejected");
    let measured = choose(3_010_000_000, cpuid);
    assert_eq!(measured, Stated { hz: 3_010_000_000, source: "pit" });
    assert_eq!(choose(2_399_000_000, None), Stated { hz: 2_399_000_000, source: "pit" });
}

#[test]
fn ppm_apart_is_relative_to_the_reference_and_survives_zero() {
    assert_eq!(ppm_apart(1_001_000, 1_000_000), 1000);
    assert_eq!(ppm_apart(999_000, 1_000_000), 1000);
    assert_eq!(ppm_apart(5, 0), u64::MAX);
}
