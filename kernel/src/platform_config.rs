/// Platform hardware configuration for ARM64.
///
/// Provides dynamic hardware addresses that differ between platforms
/// (QEMU virt vs Parallels Desktop). Defaults to QEMU virt addresses
/// so the existing boot path works unchanged.
///
/// The Parallels boot path calls `init_from_parallels()` early in boot
/// to override these with ACPI-discovered addresses.

#[cfg(target_arch = "aarch64")]
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

// =============================================================================
// Hardware address atomics with QEMU virt defaults
// =============================================================================

#[cfg(target_arch = "aarch64")]
static UART_BASE_PHYS: AtomicU64 = AtomicU64::new(0x0900_0000);

#[cfg(target_arch = "aarch64")]
static GIC_VERSION: AtomicU8 = AtomicU8::new(2);

#[cfg(target_arch = "aarch64")]
static GICD_BASE_PHYS: AtomicU64 = AtomicU64::new(0x0800_0000);

#[cfg(target_arch = "aarch64")]
static GICC_BASE_PHYS: AtomicU64 = AtomicU64::new(0x0801_0000);

#[cfg(target_arch = "aarch64")]
static GICR_BASE_PHYS: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static GICR_SIZE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static PCI_ECAM_BASE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static PCI_ECAM_SIZE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static PCI_MMIO_BASE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static PCI_MMIO_SIZE: AtomicU64 = AtomicU64::new(0);

// PCI bus range (from MCFG ACPI table). Default 0-255 for full scan on QEMU.
// Parallels provides actual bus range; scanning beyond it faults.
#[cfg(target_arch = "aarch64")]
static PCI_BUS_START: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static PCI_BUS_END: AtomicU64 = AtomicU64::new(255);

// Memory layout defaults (QEMU virt, 512MB RAM at 0x40000000)
// Kernel image:   0x4000_0000 - 0x4100_0000 (16 MB)
// Per-CPU stacks: 0x4100_0000 - 0x4200_0000 (16 MB)
// Frame alloc:    0x4200_0000 - 0x5000_0000 (224 MB)
// Heap:           0x5000_0000 - 0x5200_0000 (32 MB)

#[cfg(target_arch = "aarch64")]
static FRAME_ALLOC_START: AtomicU64 = AtomicU64::new(0x4200_0000);

#[cfg(target_arch = "aarch64")]
static FRAME_ALLOC_END: AtomicU64 = AtomicU64::new(0x5000_0000);

// =============================================================================
// Framebuffer info (UEFI GOP, populated by init_from_parallels)
// =============================================================================

#[cfg(target_arch = "aarch64")]
static FB_BASE_PHYS: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static FB_SIZE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static FB_WIDTH: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static FB_HEIGHT: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static FB_STRIDE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static FB_IS_BGR: AtomicU8 = AtomicU8::new(0);

// =============================================================================
// Accessor functions
// =============================================================================

/// UART physical base address.
/// QEMU virt: 0x0900_0000, Parallels: 0x0211_0000
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn uart_base_phys() -> u64 {
    UART_BASE_PHYS.load(Ordering::Relaxed)
}

/// UART virtual address via HHDM. Used by raw serial functions in hot paths.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub fn uart_virt() -> u64 {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    HHDM_BASE + UART_BASE_PHYS.load(Ordering::Relaxed)
}

/// GIC version (2, 3, or 4).
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn gic_version() -> u8 {
    GIC_VERSION.load(Ordering::Relaxed)
}

/// GIC Distributor physical base address.
/// QEMU virt: 0x0800_0000, Parallels: 0x0201_0000
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn gicd_base_phys() -> u64 {
    GICD_BASE_PHYS.load(Ordering::Relaxed)
}

/// GIC CPU Interface physical base address (GICv2 only).
/// QEMU virt: 0x0801_0000, Parallels: 0 (uses GICv3 system registers)
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn gicc_base_phys() -> u64 {
    GICC_BASE_PHYS.load(Ordering::Relaxed)
}

/// GIC Redistributor physical base address (GICv3+ only).
/// QEMU virt: 0, Parallels: 0x0250_0000
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn gicr_base_phys() -> u64 {
    GICR_BASE_PHYS.load(Ordering::Relaxed)
}

/// GIC Redistributor region size in bytes.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn gicr_size() -> u64 {
    GICR_SIZE.load(Ordering::Relaxed)
}

/// PCI ECAM physical base address. 0 if no PCI.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn pci_ecam_base() -> u64 {
    PCI_ECAM_BASE.load(Ordering::Relaxed)
}

/// PCI ECAM region size in bytes.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn pci_ecam_size() -> u64 {
    PCI_ECAM_SIZE.load(Ordering::Relaxed)
}

/// PCI MMIO window physical base address.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn pci_mmio_base() -> u64 {
    PCI_MMIO_BASE.load(Ordering::Relaxed)
}

/// PCI MMIO window size in bytes.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn pci_mmio_size() -> u64 {
    PCI_MMIO_SIZE.load(Ordering::Relaxed)
}

/// PCI bus range start (from MCFG ACPI table).
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn pci_bus_start() -> u8 {
    PCI_BUS_START.load(Ordering::Relaxed) as u8
}

/// PCI bus range end (from MCFG ACPI table).
/// Scanning beyond this faults on Parallels.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn pci_bus_end() -> u8 {
    PCI_BUS_END.load(Ordering::Relaxed) as u8
}

/// Frame allocator start physical address.
/// QEMU: 0x4200_0000 (after kernel + stacks)
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn frame_alloc_start() -> u64 {
    FRAME_ALLOC_START.load(Ordering::Relaxed)
}

/// Frame allocator end physical address (exclusive).
/// QEMU: 0x5000_0000
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn frame_alloc_end() -> u64 {
    FRAME_ALLOC_END.load(Ordering::Relaxed)
}

/// Returns true if running on QEMU (default platform).
/// Detected by checking if the UART address is the QEMU virt default.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn is_qemu() -> bool {
    uart_base_phys() == 0x0900_0000
}

/// Whether a UEFI GOP framebuffer was discovered by the loader.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn has_framebuffer() -> bool {
    FB_BASE_PHYS.load(Ordering::Relaxed) != 0
}

/// GOP framebuffer physical base address.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn fb_base_phys() -> u64 {
    FB_BASE_PHYS.load(Ordering::Relaxed)
}

/// GOP framebuffer size in bytes.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn fb_size() -> u64 {
    FB_SIZE.load(Ordering::Relaxed)
}

/// GOP framebuffer width in pixels.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn fb_width() -> u32 {
    FB_WIDTH.load(Ordering::Relaxed) as u32
}

/// GOP framebuffer height in pixels.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn fb_height() -> u32 {
    FB_HEIGHT.load(Ordering::Relaxed) as u32
}

/// GOP framebuffer stride in pixels.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn fb_stride() -> u32 {
    FB_STRIDE.load(Ordering::Relaxed) as u32
}

/// Whether the GOP framebuffer uses BGR pixel format (vs RGB).
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn fb_is_bgr() -> bool {
    FB_IS_BGR.load(Ordering::Relaxed) != 0
}

// =============================================================================
// Initialization from HardwareConfig (Parallels boot path)
// =============================================================================

/// HardwareConfig as received from the UEFI loader.
/// This must match the layout in parallels-loader/src/hw_config.rs.
#[cfg(target_arch = "aarch64")]
#[repr(C)]
pub struct HardwareConfig {
    pub magic: u32,
    pub version: u32,
    pub uart_base_phys: u64,
    pub uart_irq: u32,
    pub _pad0: u32,
    pub gic_version: u8,
    pub _pad1: [u8; 7],
    pub gicd_base: u64,
    pub gicc_base: u64,
    pub gicr_range_count: u32,
    pub _pad2: u32,
    pub gicr_ranges: [GicrRange; 8],
    pub pci_ecam_base: u64,
    pub pci_ecam_size: u64,
    pub pci_bus_start: u8,
    pub pci_bus_end: u8,
    pub _pad3: [u8; 6],
    pub pci_mmio_base: u64,
    pub pci_mmio_size: u64,
    pub ram_region_count: u32,
    pub _pad4: u32,
    pub ram_regions: [RamRegion; 32],
    pub has_framebuffer: u32,
    pub _pad5: u32,
    pub framebuffer: FramebufferInfo,
    pub rsdp_addr: u64,
    pub timer_freq_hz: u64,
}

#[cfg(target_arch = "aarch64")]
#[repr(C)]
pub struct GicrRange {
    pub base: u64,
    pub length: u32,
    pub _pad: u32,
}

#[cfg(target_arch = "aarch64")]
#[repr(C)]
pub struct RamRegion {
    pub base: u64,
    pub size: u64,
}

#[cfg(target_arch = "aarch64")]
#[repr(C)]
pub struct FramebufferInfo {
    pub base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: u32,
}

#[cfg(target_arch = "aarch64")]
const HARDWARE_CONFIG_MAGIC: u32 = 0x4252_4E58;

/// Initialize platform config from the HardwareConfig struct provided by
/// the UEFI loader. Called very early in boot, before serial init.
#[cfg(target_arch = "aarch64")]
pub fn init_from_parallels(config: &HardwareConfig) -> bool {
    if config.magic != HARDWARE_CONFIG_MAGIC {
        return false;
    }

    if config.uart_base_phys != 0 {
        UART_BASE_PHYS.store(config.uart_base_phys, Ordering::Relaxed);
    }
    if config.gic_version != 0 {
        GIC_VERSION.store(config.gic_version, Ordering::Relaxed);
    }
    if config.gicd_base != 0 {
        GICD_BASE_PHYS.store(config.gicd_base, Ordering::Relaxed);
    }
    if config.gicc_base != 0 {
        GICC_BASE_PHYS.store(config.gicc_base, Ordering::Relaxed);
    }
    if config.gicr_range_count > 0 {
        GICR_BASE_PHYS.store(config.gicr_ranges[0].base, Ordering::Relaxed);
        GICR_SIZE.store(config.gicr_ranges[0].length as u64, Ordering::Relaxed);
    }
    if config.pci_ecam_base != 0 {
        PCI_ECAM_BASE.store(config.pci_ecam_base, Ordering::Relaxed);
        PCI_ECAM_SIZE.store(config.pci_ecam_size, Ordering::Relaxed);
        PCI_BUS_START.store(config.pci_bus_start as u64, Ordering::Relaxed);
        PCI_BUS_END.store(config.pci_bus_end as u64, Ordering::Relaxed);
    }
    if config.pci_mmio_base != 0 {
        PCI_MMIO_BASE.store(config.pci_mmio_base, Ordering::Relaxed);
        PCI_MMIO_SIZE.store(config.pci_mmio_size, Ordering::Relaxed);
    }

    // Compute frame allocator range from RAM regions.
    // Find the largest RAM region starting at 0x4000_0000 (standard ARM64 RAM base).
    // Reserve: kernel (16 MB) + per-CPU stacks (16 MB) at the start,
    // and heap (32 MB) at the end.
    if config.ram_region_count > 0 {
        let mut best_base = 0u64;
        let mut best_size = 0u64;
        for i in 0..config.ram_region_count as usize {
            if i >= config.ram_regions.len() {
                break;
            }
            let region = &config.ram_regions[i];
            if region.size > best_size {
                best_base = region.base;
                best_size = region.size;
            }
        }

        if best_size > 0 {
            // Frame allocator starts after kernel + stacks (32 MB from RAM base)
            let fa_start = best_base + 0x0200_0000; // +32 MB
            // Frame allocator must end BEFORE the heap region.
            // The heap is at fixed physical 0x5000_0000 (32 MB), so cap fa_end there.
            let fa_end = (best_base + best_size).min(0x5000_0000);
            if fa_end > fa_start {
                FRAME_ALLOC_START.store(fa_start, Ordering::Relaxed);
                FRAME_ALLOC_END.store(fa_end, Ordering::Relaxed);
            }
        }
    }

    // Store framebuffer info if the loader discovered a GOP framebuffer
    if config.has_framebuffer != 0 && config.framebuffer.base != 0 {
        FB_BASE_PHYS.store(config.framebuffer.base, Ordering::Relaxed);
        FB_SIZE.store(config.framebuffer.size, Ordering::Relaxed);
        FB_WIDTH.store(config.framebuffer.width as u64, Ordering::Relaxed);
        FB_HEIGHT.store(config.framebuffer.height as u64, Ordering::Relaxed);
        FB_STRIDE.store(config.framebuffer.stride as u64, Ordering::Relaxed);
        FB_IS_BGR.store(if config.framebuffer.pixel_format == 1 { 1 } else { 0 }, Ordering::Relaxed);
    }

    true
}
