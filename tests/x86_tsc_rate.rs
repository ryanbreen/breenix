//! The TSC rate rule (`kernel/src/arch_impl/x86_64/tsc_rate.rs`), executed:
//! which CPUID leaves state a rate, and when a stated rate replaces the PIT's
//! measurement.

#[path = "../kernel/src/arch_impl/x86_64/tsc_rate.rs"]
mod tsc_rate;

use tsc_rate::{choose, ppm_apart, stated, Leaves, Stated, AGREEMENT_PPM};

/// A Coffee Lake part as measured on hardware: leaf 15H gives the ratio 300/2
/// with no crystal, leaf 16H a 3600 MHz base.
const COFFEE_LAKE: Leaves = Leaves {
    max_leaf: 0x16,
    ratio_denominator: 2,
    ratio_numerator: 300,
    crystal_hz: 0,
    base_mhz: 3600,
};

#[test]
fn leaf_15h_states_the_rate_when_it_names_the_crystal() {
    let leaves = Leaves { crystal_hz: 24_000_000, ..COFFEE_LAKE };
    assert_eq!(stated(leaves), Some(Stated { hz: 3_600_000_000, source: "cpuid-15h" }));
    let leaves = Leaves { crystal_hz: 38_400_000, ratio_numerator: 125, ratio_denominator: 2, ..COFFEE_LAKE };
    assert_eq!(stated(leaves).unwrap().hz, 2_400_000_000);
}

#[test]
fn without_a_crystal_leaf_16h_base_frequency_states_it() {
    assert_eq!(stated(COFFEE_LAKE), Some(Stated { hz: 3_600_000_000, source: "cpuid-16h" }));
    // Only bits 15:0 of leaf 16H EAX are the base frequency.
    let leaves = Leaves { base_mhz: 0xabcd_0000 | 2400, ..COFFEE_LAKE };
    assert_eq!(stated(leaves).unwrap().hz, 2_400_000_000);
}

#[test]
fn leaves_the_processor_does_not_implement_state_nothing() {
    assert_eq!(stated(Leaves { max_leaf: 0x14, ..COFFEE_LAKE }), None, "a leaf-14H processor");
    assert_eq!(
        stated(Leaves { max_leaf: 0x15, crystal_hz: 0, ..COFFEE_LAKE }),
        None,
        "15H without a crystal, and no 16H"
    );
    assert_eq!(stated(Leaves { base_mhz: 0, ..COFFEE_LAKE }), None, "16H reporting 0");
    assert_eq!(stated(Leaves::default()), None);
}

#[test]
fn a_stated_rate_is_used_only_when_the_pit_agrees() {
    let cpuid = stated(COFFEE_LAKE);
    // The PIT measured 3,600,081,343 Hz on hardware: 22 ppm apart.
    assert_eq!(ppm_apart(3_600_000_000, 3_600_081_343), 22);
    assert_eq!(choose(3_600_081_343, cpuid), cpuid.unwrap());
    // At the bound it is still taken; past it the measurement is.
    let at_bound = 3_600_000_000 * 1_000_000 / (1_000_000 - AGREEMENT_PPM) - 1;
    assert_eq!(choose(at_bound, cpuid).source, "cpuid-16h");
    let measured = choose(3_610_000_000, cpuid);
    assert_eq!(measured, Stated { hz: 3_610_000_000, source: "pit" });
    assert_eq!(choose(2_399_000_000, None), Stated { hz: 2_399_000_000, source: "pit" });
}

#[test]
fn ppm_apart_is_relative_to_the_reference_and_survives_zero() {
    assert_eq!(ppm_apart(1_001_000, 1_000_000), 1000);
    assert_eq!(ppm_apart(999_000, 1_000_000), 1000);
    assert_eq!(ppm_apart(5, 0), u64::MAX);
}
