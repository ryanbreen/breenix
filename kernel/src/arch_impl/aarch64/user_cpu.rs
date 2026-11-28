//! EL0 CPU policy for primary and secondary entry.
use crate::auxv::arm::*;
use core::sync::atomic::{AtomicU64, Ordering};

pub fn capabilities(pfr0: u64, isar0: u64, fp_enabled: bool) -> u64 {
    let field = |value: u64, shift: u32| (value >> shift) & 0xf;
    let mut caps = 0;
    if fp_enabled && field(pfr0, 16) <= 1 {
        caps |= HWCAP_FP;
    }
    if fp_enabled && field(pfr0, 20) <= 1 {
        caps |= HWCAP_ASIMD;
        if (1..=2).contains(&field(isar0, 4)) {
            caps |= HWCAP_AES;
            if field(isar0, 4) == 2 {
                caps |= HWCAP_PMULL;
            }
        }
        if field(isar0, 8) == 1 {
            caps |= HWCAP_SHA1;
        }
        if (1..=2).contains(&field(isar0, 12)) {
            caps |= HWCAP_SHA2;
        }
    }
    if field(isar0, 16) == 1 {
        caps |= HWCAP_CRC32;
    }
    caps
}

pub struct CommonCaps(AtomicU64);

impl CommonCaps {
    const PUBLISHED: u64 = 1 << 63;
    const ASSIGNED: u64 =
        HWCAP_FP | HWCAP_ASIMD | HWCAP_AES | HWCAP_PMULL | HWCAP_SHA1 | HWCAP_SHA2 | HWCAP_CRC32;

    pub const fn new() -> Self {
        Self(AtomicU64::new(Self::ASSIGNED))
    }

    pub fn admit(&self, local: u64) -> bool {
        self.0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                if current & Self::PUBLISHED == 0 {
                    Some(current & local)
                } else if (current & !Self::PUBLISHED) & !local == 0 {
                    Some(current)
                } else {
                    None
                }
            })
            .is_ok()
    }

    pub fn publish(&self) -> u64 {
        self.0.fetch_or(Self::PUBLISHED, Ordering::AcqRel) & !Self::PUBLISHED
    }
}

#[cfg(not(test))]
static COMMON: CommonCaps = CommonCaps::new();

#[cfg(not(test))]
pub fn init() -> bool {
    let (pfr0, isar0, cpacr, sctlr): (u64, u64, u64, u64);
    // Arm ARM, SCTLR_EL1.A, SA and SA0. Ordinary EL0 RAM mappings use Normal
    // memory. Normal scalar loads/stores may be unaligned; retain SP checks
    // and the architectural restrictions on atomics and Device memory.
    unsafe {
        core::arch::asm!(
            "mrs {pfr0}, id_aa64pfr0_el1",
            "mrs {isar0}, id_aa64isar0_el1",
            "mrs {cpacr}, cpacr_el1",
            "mrs {sctlr}, sctlr_el1",
            pfr0 = out(reg) pfr0,
            isar0 = out(reg) isar0,
            cpacr = out(reg) cpacr,
            sctlr = out(reg) sctlr,
            options(nomem, nostack, preserves_flags),
        );
        core::arch::asm!(
            "msr sctlr_el1, {}",
            "isb",
            in(reg) (sctlr & !(1 << 1)) | (3 << 3),
            options(nostack, preserves_flags),
        );
    }
    let local = capabilities(pfr0, isar0, cpacr & (3 << 20) == 3 << 20);
    local & (HWCAP_FP | HWCAP_ASIMD) == HWCAP_FP | HWCAP_ASIMD && COMMON.admit(local)
}

#[cfg(not(test))]
pub fn hwcap() -> [u64; 2] {
    [COMMON.publish(), 0]
}
