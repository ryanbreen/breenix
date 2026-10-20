#![no_std]

pub mod phys_memory;
pub mod uefi_time;

/// High-half direct-map base used by the ARM64 loader and kernel.
pub const HHDM_BASE: u64 = 0xffff_0000_0000_0000;
/// Start of the linked kernel RAM alias in the loader's L1[1] mapping.
pub const LINKED_RAM_START: u64 = 0x4000_0000;
/// The loader remaps this 512 MiB interval; the rest of L1[1] is device space.
pub const LINKED_RAM_SIZE: u64 = 0x2000_0000;
/// HardwareConfig v5 carries only final-map conventional memory as free RAM.
/// Earlier versions included boot-services memory before loader allocations.
/// v6 adds `boot_flags`. v7 adds the boot seed (`boot_seed_source`, `boot_seed`).
pub const HARDWARE_CONFIG_VERSION: u32 = 7;

/// Bytes of boot seed the loader hands the kernel's random pool.
pub const BOOT_SEED_BYTES: usize = 64;
/// `boot_seed_source`: the seed came from the firmware's EFI_RNG_PROTOCOL.
pub const BOOT_SEED_EFI_RNG: u32 = 1;
/// `boot_seed_source`: the seed came from the TPM's TPM2_GetRandom.
pub const BOOT_SEED_TPM: u32 = 2;

/// `boot_flags` bit: the ESP carries `\EFI\BREENIX\GPUCORE`, so the GPU core
/// (`kernel/src/gpu/`) owns the display device instead of the legacy display driver.
pub const BOOT_FLAG_GPU_CORE: u32 = 1 << 0;

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
