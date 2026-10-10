#![no_std]

/// High-half direct-map base used by the ARM64 loader and kernel.
pub const HHDM_BASE: u64 = 0xffff_0000_0000_0000;
/// Start of the linked kernel RAM alias in the loader's L1[1] mapping.
pub const LINKED_RAM_START: u64 = 0x4000_0000;
/// The loader remaps this 512 MiB interval; the rest of L1[1] is device space.
pub const LINKED_RAM_SIZE: u64 = 0x2000_0000;
/// HardwareConfig version carrying the loader's RAM relocation offset and the
/// firmware's enabled-CPU count.
pub const HARDWARE_CONFIG_VERSION: u32 = 4;

/// Firmware ranges that must remain identity mapped for physical-mode runtime
/// services after ExitBootServices. They are excluded from usable RAM.
pub const MAX_RUNTIME_REGIONS: usize = 64;
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeRegion {
    pub base: u64,
    pub pages: u64,
    pub kind: u32,
    pub _pad: u32,
}

/// Convert a linked kernel alias or direct RAM/MMIO VA to a device-visible IPA.
/// Low linked aliases follow the same mapping as their high-half counterparts.
#[inline]
pub const fn kernel_va_to_ipa(virt: u64, ram_offset: u64) -> u64 {
    let flat = if virt >= HHDM_BASE {
        virt - HHDM_BASE
    } else {
        virt
    };
    if flat >= LINKED_RAM_START && flat < LINKED_RAM_START + LINKED_RAM_SIZE {
        flat + ram_offset
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relocated_alias_boundaries_and_direct_addresses() {
        for base in [0, HHDM_BASE] {
            for (va, ipa) in [
                (0x3fff_ffff, 0x3fff_ffff),
                (0x4000_0000, 0x8000_0000),
                (0x4200_0000, 0x8200_0000),
                (0x5000_0000, 0x9000_0000),
                (0x5fff_ffff, 0x9fff_ffff),
                (0x6000_0000, 0x6000_0000),
                (0x7000_0000, 0x7000_0000),
                (0x8000_0000, 0x8000_0000),
                (0x9000_0000, 0x9000_0000),
                (0x1_0000_0000, 0x1_0000_0000),
            ] {
                assert_eq!(kernel_va_to_ipa(base + va, 0x4000_0000), ipa);
                assert_eq!(kernel_va_to_ipa(base + va, 0), va);
            }
        }
    }
}
