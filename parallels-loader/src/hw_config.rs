/// Hardware configuration discovered by the UEFI loader.
///
/// This struct is passed to the kernel at entry. It contains physical addresses
/// for all platform-specific hardware, discovered via ACPI tables.
/// The kernel uses these instead of hardcoded QEMU virt addresses.

/// Maximum number of RAM regions from the UEFI memory map.
pub const MAX_RAM_REGIONS: usize = 32;

/// Maximum number of GIC redistributor ranges.
pub const MAX_GICR_RANGES: usize = 8;

/// A contiguous region of physical memory.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct RamRegion {
    pub base: u64,
    pub size: u64,
}

/// GIC redistributor discovery range.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct GicrRange {
    pub base: u64,
    pub length: u32,
    pub _pad: u32,
}

/// Framebuffer information from UEFI GOP.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct FramebufferInfo {
    pub base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// 0 = RGB, 1 = BGR
    pub pixel_format: u32,
}

/// Complete hardware configuration for the platform.
///
/// Populated by the UEFI loader via ACPI discovery and UEFI boot services.
/// Passed to the kernel entry point in x0.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct HardwareConfig {
    /// Magic number for validation: 0x4252_4E58 ("BRNX")
    pub magic: u32,
    /// Version of this struct (currently 1)
    pub version: u32,

    // --- UART ---
    /// PL011 UART base physical address
    pub uart_base_phys: u64,
    /// UART interrupt (GIC SPI number)
    pub uart_irq: u32,
    pub _pad0: u32,

    // --- GIC ---
    /// GIC version (2, 3, or 4)
    pub gic_version: u8,
    pub _pad1: [u8; 7],
    /// GIC Distributor base physical address
    pub gicd_base: u64,
    /// GIC CPU Interface base (GICv2 only, 0 for GICv3+)
    pub gicc_base: u64,
    /// Number of GICR ranges
    pub gicr_range_count: u32,
    pub _pad2: u32,
    /// GIC Redistributor ranges (GICv3+)
    pub gicr_ranges: [GicrRange; MAX_GICR_RANGES],

    // --- PCI ---
    /// PCI ECAM base physical address
    pub pci_ecam_base: u64,
    /// PCI ECAM region size
    pub pci_ecam_size: u64,
    /// PCI bus range start
    pub pci_bus_start: u8,
    /// PCI bus range end
    pub pci_bus_end: u8,
    pub _pad3: [u8; 6],
    /// PCI MMIO window base
    pub pci_mmio_base: u64,
    /// PCI MMIO window size
    pub pci_mmio_size: u64,

    // --- Memory ---
    /// Number of valid RAM regions
    pub ram_region_count: u32,
    pub _pad4: u32,
    /// RAM regions from UEFI memory map
    pub ram_regions: [RamRegion; MAX_RAM_REGIONS],

    // --- Framebuffer ---
    /// Whether framebuffer info is valid
    pub has_framebuffer: u32,
    pub _pad5: u32,
    /// Framebuffer from UEFI GOP
    pub framebuffer: FramebufferInfo,

    // --- ACPI ---
    /// RSDP physical address (for kernel to parse additional ACPI tables)
    pub rsdp_addr: u64,

    // --- Timer ---
    /// Generic timer frequency in Hz (from CNTFRQ_EL0)
    pub timer_freq_hz: u64,
}

pub const HARDWARE_CONFIG_MAGIC: u32 = 0x4252_4E58; // "BRNX"
pub const HARDWARE_CONFIG_VERSION: u32 = 1;

impl HardwareConfig {
    /// Create a zeroed config with magic and version set.
    pub fn new() -> Self {
        Self {
            magic: HARDWARE_CONFIG_MAGIC,
            version: HARDWARE_CONFIG_VERSION,
            uart_base_phys: 0,
            uart_irq: 0,
            _pad0: 0,
            gic_version: 0,
            _pad1: [0; 7],
            gicd_base: 0,
            gicc_base: 0,
            gicr_range_count: 0,
            _pad2: 0,
            gicr_ranges: [GicrRange { base: 0, length: 0, _pad: 0 }; MAX_GICR_RANGES],
            pci_ecam_base: 0,
            pci_ecam_size: 0,
            pci_bus_start: 0,
            pci_bus_end: 0,
            _pad3: [0; 6],
            pci_mmio_base: 0,
            pci_mmio_size: 0,
            ram_region_count: 0,
            _pad4: 0,
            ram_regions: [RamRegion { base: 0, size: 0 }; MAX_RAM_REGIONS],
            has_framebuffer: 0,
            _pad5: 0,
            framebuffer: FramebufferInfo {
                base: 0,
                size: 0,
                width: 0,
                height: 0,
                stride: 0,
                pixel_format: 0,
            },
            rsdp_addr: 0,
            timer_freq_hz: 0,
        }
    }

    #[allow(dead_code)] // Used by kernel to validate config received from loader
    pub fn validate(&self) -> bool {
        self.magic == HARDWARE_CONFIG_MAGIC && self.version == HARDWARE_CONFIG_VERSION
    }
}
