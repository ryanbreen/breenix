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

    // --- Pre-ExitBootServices xHCI driver disconnect ---
    //
    // CRITICAL Parallels workaround: UEFI's ExitBootServices cleanup resets the
    // xHCI controller (XHC controller reset) and then disables the PCI BAR
    // (phymemrange_disable 0x18011000). Once the BAR is disabled, the Parallels
    // hypervisor permanently disassociates virtual USB devices from the controller.
    //
    // Fix: Disconnect UEFI's xHCI driver BEFORE ExitBootServices. Without a driver
    // bound to the device, ExitBootServices won't reset the controller or disable
    // the BAR. The kernel then does its own HCRST on a BAR that was never disabled.
    //
    // xHCI device: PCI 00:03.0 (vendor 0x1033, device 0x0194)
    config.xhci_hcrst_done = disconnect_xhci_driver();

    log::info!("--- Exiting Boot Services ---");

    // Exit UEFI boot services. After this, NO UEFI calls are possible.
    unsafe {
        let _ = uefi::boot::exit_boot_services(MemoryType::LOADER_DATA);
    }

    // Safety: re-enable xHCI PCI BAR after ExitBootServices as a fallback.
    // If the disconnect worked, the BAR was never disabled and this is a no-op.
    // If it failed, this ensures the kernel can at least see the device.
    if config.pci_ecam_base != 0 {
        unsafe {
            let ecam_xhci = config.pci_ecam_base + 0x18000;
            let cmd_addr = (ecam_xhci + 4) as *mut u32;
            let dword = core::ptr::read_volatile(cmd_addr);
            let new_cmd = ((dword & 0xFFFF) | 0x0006) & 0xFFFF;
            core::ptr::write_volatile(cmd_addr, new_cmd);

            // UART breadcrumb 'U' = post-EBS BAR re-enable
            let uart = config.uart_base_phys as *mut u32;
            core::ptr::write_volatile(uart, b'U' as u32);
        }
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

// ---------------------------------------------------------------------------
// Minimal EFI_PCI_IO_PROTOCOL binding for disconnect_xhci_driver()
// ---------------------------------------------------------------------------

/// GetLocation function pointer type.
type PciIoGetLocationFn = unsafe extern "efiapi" fn(
    this: *const PciIoProtocol,
    segment: *mut usize,
    bus: *mut usize,
    device: *mut usize,
    function: *mut usize,
) -> Status;

/// Minimal EFI_PCI_IO_PROTOCOL — only need GetLocation for identifying the device.
/// Layout must match UEFI Spec 2.10 Section 14.4 up through GetLocation.
#[repr(C)]
#[uefi::proto::unsafe_protocol("4cf5b200-68b8-4ca5-9eec-b23e3f50029a")]
struct PciIoProtocol {
    poll_mem: usize,
    poll_io: usize,
    mem_read: usize,
    mem_write: usize,
    io_read: usize,
    io_write: usize,
    pci_read: usize,
    pci_write: usize,
    copy_mem: usize,
    map: usize,
    unmap: usize,
    allocate_buffer: usize,
    free_buffer: usize,
    flush: usize,
    get_location: PciIoGetLocationFn,
}

/// Disconnect UEFI's xHCI driver from the PCI device at 00:03.0.
///
/// Returns a sentinel value for diagnostics:
///   0x01 = success (driver disconnected)
///   0xA1 = no PCI handles found
///   0xA2 = xHCI device not found among handles
///   0xA3 = disconnect_controller failed
fn disconnect_xhci_driver() -> u32 {
    use uefi::boot;

    // Find all handles with EFI_PCI_IO_PROTOCOL
    let handles = match boot::locate_handle_buffer(
        boot::SearchType::ByProtocol(&<PciIoProtocol as uefi::Identify>::GUID),
    ) {
        Ok(h) => h,
        Err(_) => {
            log::warn!("xHCI disconnect: no PCI handles found");
            return 0xA1;
        }
    };

    log::info!("xHCI disconnect: found {} PCI device handles", handles.len());

    let image = boot::image_handle();

    for &handle in handles.iter() {
        // Open protocol just to peek at GetLocation
        let pci_io = match unsafe {
            boot::open_protocol::<PciIoProtocol>(
                boot::OpenProtocolParams {
                    handle,
                    agent: image,
                    controller: None,
                },
                boot::OpenProtocolAttributes::GetProtocol,
            )
        } {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Check if this is 00:03.0 (the xHCI device)
        let (mut seg, mut bus, mut dev, mut fun) = (0usize, 0, 0, 0);
        let status = unsafe {
            (pci_io.get_location)(
                &*pci_io as *const PciIoProtocol,
                &mut seg,
                &mut bus,
                &mut dev,
                &mut fun,
            )
        };

        if status != Status::SUCCESS {
            continue;
        }

        if bus != 0 || dev != 3 || fun != 0 {
            continue;
        }

        log::info!("xHCI disconnect: found device at {:04x}:{:02x}:{:02x}.{:x}", seg, bus, dev, fun);

        // Drop the protocol reference before disconnecting
        drop(pci_io);

        // Disconnect ALL drivers from this controller
        match boot::disconnect_controller(handle, None, None) {
            Ok(_) => {
                log::info!("xHCI disconnect: SUCCESS — UEFI driver detached");
                return 0x01;
            }
            Err(e) => {
                log::warn!("xHCI disconnect: disconnect_controller failed: {:?}", e);
                return 0xA3;
            }
        }
    }

    log::warn!("xHCI disconnect: device 00:03.0 not found");
    0xA2
}
