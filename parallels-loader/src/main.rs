#![no_main]
#![no_std]
#![allow(dead_code)]

mod acpi_discovery;
mod gop_discovery;
pub mod hw_config;
mod kernel_entry;
mod kernel_load;
mod page_tables;

use uefi::prelude::*;
use uefi::mem::memory_map::{MemoryMap, MemoryType};
use uefi::table::cfg::ACPI2_GUID;

use hw_config::HardwareConfig;
use page_tables::PageTableStorage;

/// Page table storage - static so it survives ExitBootServices.
static mut PAGE_TABLES: PageTableStorage = PageTableStorage::new();

/// HardwareConfig - static so it survives ExitBootServices.
static mut HW_CONFIG: HardwareConfig = unsafe { core::mem::zeroed() };

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();

    log::info!("===========================================");
    log::info!("  Breenix Parallels Loader v0.1.0");
    log::info!("  UEFI ARM64 Boot Application");
    log::info!("===========================================");

    // Initialize HW_CONFIG with proper magic/version
    let config = unsafe {
        let ptr = &raw mut HW_CONFIG;
        *ptr = HardwareConfig::new();
        &mut *ptr
    };

    // Find RSDP from UEFI configuration tables
    let rsdp_addr = find_rsdp();
    let rsdp_addr = match rsdp_addr {
        Some(addr) => {
            log::info!("RSDP found at: 0x{:016x}", addr);
            config.rsdp_addr = addr as u64;
            addr
        }
        None => {
            log::error!("RSDP not found in UEFI configuration tables!");
            halt();
        }
    };

    // Discover hardware via ACPI
    log::info!("--- ACPI Discovery ---");
    match acpi_discovery::discover_hardware(rsdp_addr, config) {
        Ok(()) => {
            log::info!("--- Discovery Complete ---");
            log::info!("  UART:    0x{:08x} (IRQ {})", config.uart_base_phys, config.uart_irq);
            log::info!(
                "  GICv{}:   GICD=0x{:08x}",
                config.gic_version,
                config.gicd_base
            );
            if config.gicr_range_count > 0 {
                log::info!(
                    "           GICR=0x{:08x} ({}x ranges)",
                    config.gicr_ranges[0].base,
                    config.gicr_range_count
                );
            }
            if config.pci_ecam_base != 0 {
                log::info!(
                    "  PCI:     ECAM=0x{:08x}, MMIO=0x{:08x}",
                    config.pci_ecam_base,
                    config.pci_mmio_base
                );
            }
            log::info!("  Timer:   {} MHz", config.timer_freq_hz / 1_000_000);
        }
        Err(e) => {
            log::error!("ACPI discovery failed: {}", e);
            halt();
        }
    }

    // Load kernel ELF from ESP
    log::info!("--- Loading Kernel ---");
    let loaded_kernel = match kernel_load::load_kernel() {
        Ok(k) => {
            log::info!("Kernel loaded: entry_phys={:#x}, range={:#x}-{:#x}",
                k.entry_phys, k.load_base, k.load_end);
            k
        }
        Err(e) => {
            log::error!("Failed to load kernel: {}", e);
            halt();
        }
    };

    // Populate RAM regions from UEFI memory map.
    // We need to get the memory map before ExitBootServices.
    populate_ram_regions(config);

    log::info!("RAM regions: {} regions found", config.ram_region_count);
    for i in 0..config.ram_region_count as usize {
        if i >= config.ram_regions.len() {
            break;
        }
        let r = &config.ram_regions[i];
        log::info!("  RAM: {:#x} - {:#x} ({} MB)",
            r.base, r.base + r.size, r.size / (1024 * 1024));
    }

    // Discover UEFI GOP framebuffer (optional — kernel works without display)
    log::info!("--- GOP Framebuffer Discovery ---");
    match gop_discovery::discover_gop(config) {
        Ok(()) => {
            log::info!("GOP framebuffer: {}x{} stride={} fmt={} base={:#x} size={:#x}",
                config.framebuffer.width,
                config.framebuffer.height,
                config.framebuffer.stride,
                if config.framebuffer.pixel_format == 1 { "BGR" } else { "RGB" },
                config.framebuffer.base,
                config.framebuffer.size);
        }
        Err(e) => {
            log::warn!("GOP not available: {} (continuing without display)", e);
        }
    }

    log::info!("--- Exiting Boot Services ---");

    // Exit UEFI boot services. After this, NO UEFI calls are possible.
    // The memory map is required for ExitBootServices.
    unsafe {
        let _ = uefi::boot::exit_boot_services(MemoryType::LOADER_DATA);
    }

    // Jump to kernel with our page tables and HardwareConfig
    let page_tables = unsafe { &mut *(&raw mut PAGE_TABLES) };
    let hw_config = unsafe { &*(&raw const HW_CONFIG) };

    kernel_entry::jump_to_kernel(loaded_kernel.entry_phys, hw_config, page_tables);
}

/// Populate HardwareConfig RAM regions from the UEFI memory map.
///
/// Scans the UEFI memory map for conventional memory (usable RAM) regions
/// and adds them to the HardwareConfig. The kernel uses these to configure
/// its frame allocator.
fn populate_ram_regions(config: &mut HardwareConfig) {
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

/// Find the ACPI RSDP address from UEFI configuration tables.
fn find_rsdp() -> Option<usize> {
    let st = uefi::table::system_table_raw().expect("no system table");

    // Safety: we're in boot services, system table is valid
    let st_ref = unsafe { st.as_ref() };

    // Iterate configuration tables looking for ACPI 2.0 RSDP
    let cfg_entries = st_ref.number_of_configuration_table_entries;
    let cfg_table = st_ref.configuration_table;

    if cfg_table.is_null() || cfg_entries == 0 {
        return None;
    }

    let entries = unsafe { core::slice::from_raw_parts(cfg_table, cfg_entries) };

    for entry in entries {
        if entry.vendor_guid == ACPI2_GUID {
            return Some(entry.vendor_table as usize);
        }
    }

    // Fall back to ACPI 1.0 RSDP
    let acpi1_guid = uefi::table::cfg::ACPI_GUID;
    for entry in entries {
        if entry.vendor_guid == acpi1_guid {
            return Some(entry.vendor_table as usize);
        }
    }

    None
}

/// Halt the CPU in an infinite loop (unrecoverable error).
fn halt() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}
