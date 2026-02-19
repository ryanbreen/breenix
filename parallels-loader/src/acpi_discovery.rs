/// ACPI table discovery for Parallels hardware.
///
/// Parses MADT (GIC addresses), MCFG (PCI ECAM), and SPCR (UART) tables
/// to populate HardwareConfig with actual hardware addresses.
use core::ptr::NonNull;

use acpi::madt::{Madt, MadtEntry};
use acpi::mcfg::Mcfg;
use acpi::spcr::Spcr;
use acpi::{AcpiHandler, AcpiTables, PhysicalMapping};

use crate::hw_config::{GicrRange, HardwareConfig, MAX_GICR_RANGES, MAX_RAM_REGIONS};

/// UEFI ACPI handler.
///
/// During UEFI boot services, physical memory is identity-mapped,
/// so physical addresses can be used directly as virtual addresses.
#[derive(Clone)]
pub struct UefiAcpiHandler;

unsafe impl Send for UefiAcpiHandler {}

impl AcpiHandler for UefiAcpiHandler {
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> PhysicalMapping<Self, T> {
        // UEFI identity maps all physical memory during boot services
        let ptr = NonNull::new(physical_address as *mut T).expect("null ACPI address");
        unsafe { PhysicalMapping::new(physical_address, ptr, size, size, self.clone()) }
    }

    fn unmap_physical_region<T>(_region: &PhysicalMapping<Self, T>) {
        // Nothing to unmap - identity mapped
    }
}

/// Discover hardware configuration from ACPI tables.
///
/// `rsdp_addr` is the physical address of the RSDP, obtained from
/// UEFI's `EFI_ACPI_TABLE_GUID` configuration table.
pub fn discover_hardware(rsdp_addr: usize, config: &mut HardwareConfig) -> Result<(), &'static str> {
    config.rsdp_addr = rsdp_addr as u64;

    let handler = UefiAcpiHandler;
    let tables = unsafe { AcpiTables::from_rsdp(handler, rsdp_addr) }
        .map_err(|_| "Failed to parse ACPI tables from RSDP")?;

    // Parse MADT for GIC configuration
    parse_madt(&tables, config)?;

    // Parse MCFG for PCI ECAM
    parse_mcfg(&tables, config);

    // Parse SPCR for UART (fallback if not found)
    parse_spcr(&tables, config);

    // Read timer frequency from CNTFRQ_EL0
    config.timer_freq_hz = read_timer_freq();

    Ok(())
}

/// Parse MADT to extract GIC distributor, redistributor, and CPU interface info.
fn parse_madt(
    tables: &AcpiTables<UefiAcpiHandler>,
    config: &mut HardwareConfig,
) -> Result<(), &'static str> {
    let madt_mapping = tables
        .find_table::<Madt>()
        .map_err(|_| "MADT table not found")?;

    let madt_pin = madt_mapping.get();
    let mut gicr_idx = 0usize;

    for entry in madt_pin.entries() {
        match entry {
            MadtEntry::Gicd(gicd) => {
                config.gicd_base = gicd.physical_base_address;
                config.gic_version = gicd.gic_version;
                // Version 0 means "detect from hardware"
                if config.gic_version == 0 {
                    // Default to GICv3 on Parallels (known from hw dump)
                    config.gic_version = 3;
                }
                log::info!(
                    "  GICD: base=0x{:08x}, version={}",
                    config.gicd_base,
                    config.gic_version
                );
            }
            MadtEntry::Gicc(gicc) => {
                let gicc_base = gicc.gic_registers_address;
                if gicc_base != 0 && config.gicc_base == 0 {
                    config.gicc_base = gicc_base;
                    log::info!("  GICC: base=0x{:08x}", config.gicc_base);
                }
            }
            MadtEntry::GicRedistributor(gicr) => {
                if gicr_idx < MAX_GICR_RANGES {
                    let base = gicr.discovery_range_base_address;
                    let length = gicr.discovery_range_length;
                    config.gicr_ranges[gicr_idx] = GicrRange {
                        base,
                        length,
                        _pad: 0,
                    };
                    log::info!(
                        "  GICR[{}]: base=0x{:08x}, length=0x{:x}",
                        gicr_idx,
                        base,
                        length,
                    );
                    gicr_idx += 1;
                }
            }
            MadtEntry::GicMsiFrame(msi) => {
                let msi_base = msi.physical_base_address;
                let spi_base = msi.spi_base;
                let spi_count = msi.spi_count;
                log::info!(
                    "  GICv2m MSI: base=0x{:08x}, SPI base={}, count={}",
                    msi_base,
                    spi_base,
                    spi_count,
                );
            }
            _ => {}
        }
    }

    config.gicr_range_count = gicr_idx as u32;

    if config.gicd_base == 0 {
        return Err("No GICD found in MADT");
    }

    Ok(())
}

/// Parse MCFG for PCI Enhanced Configuration Access Mechanism (ECAM).
fn parse_mcfg(tables: &AcpiTables<UefiAcpiHandler>, config: &mut HardwareConfig) {
    let mcfg_mapping = match tables.find_table::<Mcfg>() {
        Ok(m) => m,
        Err(_) => {
            log::warn!("  MCFG table not found - PCI ECAM unavailable");
            return;
        }
    };

    for entry in mcfg_mapping.entries() {
        // Use the first segment group (typically segment 0)
        if config.pci_ecam_base == 0 {
            config.pci_ecam_base = entry.base_address;
            config.pci_bus_start = entry.bus_number_start;
            config.pci_bus_end = entry.bus_number_end;

            // ECAM size: 4KB per function, 8 functions per device, 32 devices per bus
            let bus_count = (config.pci_bus_end as u64 - config.pci_bus_start as u64 + 1) as u64;
            config.pci_ecam_size = bus_count * 32 * 8 * 4096;

            log::info!(
                "  PCI ECAM: base=0x{:08x}, buses {}..{}, size=0x{:x}",
                config.pci_ecam_base,
                config.pci_bus_start,
                config.pci_bus_end,
                config.pci_ecam_size,
            );
        }
    }

    // PCI MMIO window - typically at 0x10000000 on Parallels
    // This is from the host bridge _CRS in DSDT, but we hardcode the known value
    // since DSDT AML parsing is complex. The kernel can override from device tree.
    if config.pci_ecam_base != 0 && config.pci_mmio_base == 0 {
        config.pci_mmio_base = 0x1000_0000;
        config.pci_mmio_size = 0x1000_0000; // 256 MB
        log::info!(
            "  PCI MMIO: base=0x{:08x}, size=0x{:x}",
            config.pci_mmio_base,
            config.pci_mmio_size,
        );
    }
}

/// Parse SPCR for serial port configuration.
fn parse_spcr(tables: &AcpiTables<UefiAcpiHandler>, config: &mut HardwareConfig) {
    let spcr_mapping = match tables.find_table::<Spcr>() {
        Ok(s) => s,
        Err(_) => {
            log::info!("  SPCR table not found - using fallback UART detection");
            // Fallback: check DBG2 table or use known Parallels address
            if config.uart_base_phys == 0 {
                config.uart_base_phys = 0x0211_0000; // Known Parallels PL011 address
                config.uart_irq = 32; // SPI 0
                log::info!(
                    "  UART (fallback): base=0x{:08x}, irq={}",
                    config.uart_base_phys,
                    config.uart_irq
                );
            }
            return;
        }
    };

    let iface_type = spcr_mapping.interface_type();
    log::info!("  SPCR interface type: {:?}", iface_type);

    if let Some(Ok(addr)) = spcr_mapping.base_address() {
        config.uart_base_phys = addr.address;
        log::info!("  UART: base=0x{:08x}", config.uart_base_phys);
    }

    if let Some(gsi) = spcr_mapping.global_system_interrupt() {
        config.uart_irq = gsi;
        log::info!("  UART: irq={}", config.uart_irq);
    }
}

/// Read the ARM generic timer frequency from CNTFRQ_EL0.
fn read_timer_freq() -> u64 {
    let freq: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq);
    }
    log::info!("  Timer frequency: {} Hz ({} MHz)", freq, freq / 1_000_000);
    freq
}

/// Populate RAM regions from UEFI memory map.
///
/// Called after ExitBootServices with the UEFI memory map.
/// Only includes EfiConventionalMemory regions.
#[allow(dead_code)] // Used in Phase 2 when kernel entry is implemented
pub fn populate_ram_regions(
    config: &mut HardwareConfig,
    regions: &[(u64, u64)], // (base, size) pairs of conventional memory
) {
    let count = regions.len().min(MAX_RAM_REGIONS);
    for (i, &(base, size)) in regions.iter().take(count).enumerate() {
        config.ram_regions[i].base = base;
        config.ram_regions[i].size = size;
    }
    config.ram_region_count = count as u32;

    let total_mb: u64 = regions.iter().map(|(_, s)| s).sum::<u64>() / (1024 * 1024);
    log::info!("  RAM: {} regions, {} MiB total", count, total_mb);
}
