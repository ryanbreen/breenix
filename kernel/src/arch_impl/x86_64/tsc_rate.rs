//! The TSC rate the processor states, and the rule that accepts it against a
//! measurement. Pure arithmetic over CPUID values, kept apart from `timer` so
//! the host executes it (`tests/x86_tsc_rate.rs`).
//!
//! Sources, from the Intel SDM vol. 2A (CPUID leaves 15H and 16H) and vol. 3B
//! ("Determining the Processor Base Frequency"):
//!
//! - Leaf 15H gives the TSC-to-core-crystal ratio EBX/EAX and, in ECX, the
//!   crystal frequency in Hz. With ECX nonzero the TSC rate is
//!   `ECX * EBX / EAX`.
//! - With ECX 0 the crystal is not enumerated. Leaf 16H EAX then gives the
//!   processor base frequency in MHz. It is only a candidate TSC rate:
//!   invariant TSC alone does not establish equality to this frequency.
//!
//! Neither is trusted alone: `choose` takes a stated rate only when it agrees
//! with the rate the PIT measured, so a hypervisor or a part whose leaves do
//! not describe its TSC falls back to the measurement.

/// The most a stated rate and the PIT measurement may differ by, in parts per
/// million, for the stated rate to be taken. The PIT must agree within
/// 200 ppm; hardware observations belong in private evidence.
pub const AGREEMENT_PPM: u64 = 200;

/// CPUID values the stated rate is derived from. A leaf the processor does not
/// implement (above `max_leaf`) is read as 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Leaves {
    pub max_leaf: u32,
    /// Leaf 15H EAX, EBX, ECX.
    pub ratio_denominator: u32,
    pub ratio_numerator: u32,
    pub crystal_hz: u32,
    /// Leaf 16H EAX bits 15:0.
    pub base_mhz: u32,
}

/// A TSC rate the processor states, and which leaf stated it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stated {
    pub hz: u64,
    pub source: &'static str,
}

/// The TSC rate leaves 15H and 16H state, if they state one.
pub fn stated(leaves: Leaves) -> Option<Stated> {
    if leaves.max_leaf >= 0x15
        && leaves.ratio_denominator != 0
        && leaves.ratio_numerator != 0
        && leaves.crystal_hz != 0
    {
        let hz = u64::from(leaves.crystal_hz) * u64::from(leaves.ratio_numerator)
            / u64::from(leaves.ratio_denominator);
        return Some(Stated { hz, source: "cpuid-15h" });
    }
    let base = leaves.base_mhz & 0xffff;
    if leaves.max_leaf >= 0x16 && base != 0 {
        return Some(Stated {
            hz: u64::from(base) * 1_000_000,
            source: "cpuid-16h",
        });
    }
    None
}

/// How far apart two rates are, in parts per million of `reference`.
pub fn ppm_apart(rate: u64, reference: u64) -> u64 {
    if reference == 0 {
        return u64::MAX;
    }
    let difference = rate.abs_diff(reference);
    (u128::from(difference) * 1_000_000 / u128::from(reference)) as u64
}

/// The rate to run the TSC clock at: the stated rate when it agrees with the
/// PIT's measurement within `AGREEMENT_PPM`, else the measurement.
pub fn choose(measured_hz: u64, stated: Option<Stated>) -> Stated {
    match stated {
        Some(stated) if ppm_apart(stated.hz, measured_hz) <= AGREEMENT_PPM => stated,
        _ => Stated {
            hz: measured_hz,
            source: "pit",
        },
    }
}
