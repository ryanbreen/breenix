use arm64_boot_contract::hw_config::{self, HardwareConfig};
use uefi::mem::memory_map::{MemoryMap, MemoryType};

/// Populate HardwareConfig RAM regions from the UEFI memory map.
///
/// Scans the UEFI memory map for conventional memory (usable RAM) regions
/// and adds them to the HardwareConfig. The kernel uses these to configure
/// its frame allocator.
pub fn populate_ram_regions(config: &mut HardwareConfig) {
    // Get the UEFI memory map
    let buf = [0u8; 8192];
    let memory_map = match uefi::boot::memory_map(MemoryType::LOADER_DATA) {
        Ok(map) => map,
        Err(_) => {
            log::warn!("Failed to get UEFI memory map for RAM regions");
            return;
        }
    };

    let mut count = 0usize;

    for desc in memory_map.entries() {
        // Only count conventional memory (usable RAM)
        let mem_type = desc.ty;
        let is_usable = matches!(
            mem_type,
            MemoryType::CONVENTIONAL
                | MemoryType::BOOT_SERVICES_CODE
                | MemoryType::BOOT_SERVICES_DATA
        );

        if !is_usable {
            continue;
        }

        let base = desc.phys_start;
        let size = desc.page_count * 4096;

        // Merge with previous region if contiguous
        if count > 0 {
            let prev = &mut config.ram_regions[count - 1];
            if prev.base + prev.size == base {
                prev.size += size;
                continue;
            }
        }

        if count < config.ram_regions.len() {
            config.ram_regions[count] = hw_config::RamRegion { base, size };
            count += 1;
        }
    }

    config.ram_region_count = count as u32;

    let _ = buf; // Suppress unused warning
}

pub unsafe fn exit_and_encode(config: &mut HardwareConfig) {
    // Runtime GetTime remains available after ExitBootServices in physical
    // mode. Pass the final runtime memory map, never reclaimable boot pages.
    let st = uefi::table::system_table_raw().expect("no system table");
    config.rtc_get_time = unsafe { (*st.as_ref().runtime_services).get_time as usize as u64 };
    let final_map = unsafe { uefi::boot::exit_boot_services(MemoryType::LOADER_DATA) };
    // UEFI 2.10 section 7.2 (Tables 7.5/7.6): EfiConventionalMemory is
    // unallocated. Keep boot-services and loader memory unavailable even after
    // EBS, since firmware USB DMA survives until the kernel resets the device.
    // Use the final map so later loader allocations are excluded too.
    let free = arm64_boot_contract::phys_memory::final_uefi_conventional_ranges(
        final_map
            .entries()
            .map(|d| (d.ty.0, d.att.bits(), d.phys_start, d.page_count)),
    );
    config.ram_region_count = free.as_slice().len() as u32;
    for (slot, range) in config.ram_regions.iter_mut().zip(free.as_slice()) {
        *slot = hw_config::RamRegion {
            base: range.start,
            size: range.len(),
        };
    }
    for desc in final_map.entries().filter(|d| {
        d.att
            .contains(uefi::mem::memory_map::MemoryAttribute::RUNTIME)
    }) {
        let i = config.runtime_region_count as usize;
        if i == config.runtime_regions.len() {
            config.rtc_get_time = 0; // Never call with a truncated runtime map.
            break;
        }
        config.runtime_regions[i] = arm64_boot_contract::RuntimeRegion {
            base: desc.phys_start,
            pages: desc.page_count,
            kind: desc.ty.0,
            _pad: 0,
        };
        config.runtime_region_count += 1;
    }
}
