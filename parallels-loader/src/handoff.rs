use arm64_boot_contract::{
    hw_config::HardwareConfig,
    uefi_map::{self, Descriptor, ATTR_RUNTIME, BOOT_MAP_MEMORY_TYPE},
    RuntimeRegion,
};
use uefi::mem::memory_map::{MemoryMap, MemoryType};

/// Look up the relocation base while boot services are still available.
pub fn populate_ram_regions() -> Option<(u64, usize)> {
    let map = uefi::boot::memory_map(MemoryType::LOADER_DATA).ok()?;
    let meta = map.meta();
    let buffer = map.buffer();
    uefi_map::check_meta(
        meta.desc_version,
        meta.desc_size,
        meta.map_size as u64,
        buffer.len() as u64,
    )
    .ok()?;
    let mut count = 0;
    let mut lowest = u64::MAX;
    for record in buffer[..meta.map_size].chunks_exact(meta.desc_size) {
        let descriptor = Descriptor::read(record);
        if matches!(descriptor.kind, 3 | 4 | 7) && descriptor.extent().is_some() {
            lowest = lowest.min(descriptor.start);
            count += 1;
        }
    }
    (count != 0).then_some((lowest, count))
}

/// Consume boot services and preserve the exact map associated with the
/// successful exit key. HardwareConfig and the pool survive the jump.
pub unsafe fn exit_and_encode(config: &mut HardwareConfig) {
    let st = uefi::table::system_table_raw().expect("no system table");
    config.rtc_get_time = unsafe { (*st.as_ref().runtime_services).get_time as usize as u64 };
    let final_map =
        unsafe { uefi::boot::exit_boot_services(MemoryType::custom(BOOT_MAP_MEMORY_TYPE)) };
    let meta = final_map.meta();
    let buffer = final_map.buffer();
    encode_final_map(
        config,
        buffer,
        meta.map_size,
        meta.desc_size,
        meta.desc_version,
    );
    core::mem::forget(final_map);
}

pub(crate) fn encode_final_map(
    config: &mut HardwareConfig,
    buffer: &[u8],
    size: usize,
    stride: usize,
    version: u32,
) {
    config.uefi_map = uefi_map::encode_uefi_map(buffer, size, stride, version);
    config.ram_region_count = 0;
    config.runtime_region_count = 0;
    if config.uefi_map.loader_check == 0 {
        for record in buffer[..size].chunks_exact(stride) {
            let descriptor = Descriptor::read(record);
            if descriptor.attributes & ATTR_RUNTIME == 0 {
                continue;
            }
            let index = config.runtime_region_count as usize;
            if index == config.runtime_regions.len() {
                config.rtc_get_time = 0;
                break;
            }
            config.runtime_regions[index] = RuntimeRegion {
                base: descriptor.start,
                pages: descriptor.pages,
                kind: descriptor.kind,
                _pad: 0,
            };
            config.runtime_region_count += 1;
        }
    } else {
        config.rtc_get_time = 0;
    }
}
