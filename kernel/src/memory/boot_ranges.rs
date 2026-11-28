use arm64_boot_contract::{
    hw_config::HardwareConfig,
    phys_memory::{self, PhysRange, RangeSet},
};

pub fn from_hardware_config(
    config: &HardwareConfig,
    window: PhysRange,
    fixed: impl IntoIterator<Item = PhysRange>,
) -> RangeSet {
    let region_count = (config.ram_region_count as usize).min(config.ram_regions.len());
    let runtime_count = (config.runtime_region_count as usize).min(config.runtime_regions.len());
    let runtime = config.runtime_regions[..runtime_count]
        .iter()
        .map(|r| PhysRange::from_base_size(r.base, r.pages.saturating_mul(4096)));
    let mut ranges = phys_memory::frame_pool(
        config.ram_regions[..region_count]
            .iter()
            .map(|r| PhysRange::from_base_size(r.base, r.size)),
        runtime,
        window,
    );
    // The heap and kernel stacks are written directly, outside the frame
    // allocator, so they come out of conventional RAM, after the fixed
    // ranges are excluded.
    for fixed in fixed {
        ranges.subtract(fixed);
    }
    ranges
}
