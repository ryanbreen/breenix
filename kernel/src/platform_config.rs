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

/// Next GICv2m SPI index to allocate (offset from GICV2M_SPI_BASE).
#[cfg(target_arch = "aarch64")]
static GICV2M_NEXT_SPI: AtomicU64 = AtomicU64::new(0);

// =============================================================================
// Hardware address atomics with QEMU virt defaults
// =============================================================================

#[cfg(target_arch = "aarch64")]
static UART_BASE_PHYS: AtomicU64 = AtomicU64::new(0x0900_0000);

/// UART type: 0 = PL011 (default), 1 = 16550
#[cfg(target_arch = "aarch64")]
static UART_TYPE: AtomicU8 = AtomicU8::new(0);

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
static GICV2M_BASE_PHYS: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static GICV2M_SPI_BASE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static GICV2M_SPI_COUNT: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static PCI_BUS_START: AtomicU64 = AtomicU64::new(0);

#[cfg(target_arch = "aarch64")]
static PCI_BUS_END: AtomicU64 = AtomicU64::new(255);

// RAM base offset: difference between actual physical RAM start and the linker-
// expected 0x40000000. Zero on QEMU/Parallels, 0x40000000 on VMware Fusion.
// Used by DMA drivers to convert kernel VAs to correct IPAs for device DMA.
#[cfg(target_arch = "aarch64")]
static RAM_BASE_OFFSET: AtomicU64 = AtomicU64::new(0);

// xHCI loader-level HCRST flag. Non-zero if the parallels-loader already did
// HCRST before ExitBootServices (kernel should skip HCRST).
#[cfg(target_arch = "aarch64")]
static XHCI_HCRST_DONE: AtomicU64 = AtomicU64::new(0);

/// Boot wall clock time (Unix timestamp) provided by UEFI GetTime().
#[cfg(target_arch = "aarch64")]
static BOOT_WALL_TIME_UTC: AtomicU64 = AtomicU64::new(0);

// Memory layout defaults (QEMU virt, 512MB RAM at 0x40000000)
// Kernel image:   0x4000_0000 - 0x4300_0000 (48 MB, image + BSS incl. PCI_3D_FRAMEBUFFER)
// SMP stacks:     0x4300_0000 - 0x4400_0000 (16 MB, 8 CPUs × 2 MB each)
// Frame alloc:    0x4400_0000 - 0x5000_0000 (192 MB)
// DMA (NC):       0x5000_0000 - 0x501F_FFFF (2 MB, Non-Cacheable for xHCI)
// Heap:           0x5020_0000 - 0x541F_FFFF (64 MB)
// Kernel stacks:  0x5420_0000 - 0x561F_FFFF (32 MB)

#[cfg(target_arch = "aarch64")]
static FRAME_ALLOC_START: AtomicU64 = AtomicU64::new(0x4400_0000);

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

/// UART type: 0 = PL011, 1 = 16550
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn uart_type() -> u8 {
    UART_TYPE.load(Ordering::Relaxed)
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

/// Override the GICR base physical address (used by GICR probe when the
/// loader-reported address is wrong and a fallback address is found).
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn set_gicr_base_phys(base: u64) {
    GICR_BASE_PHYS.store(base, Ordering::Relaxed);
}

/// GICv2m MSI frame physical base address. 0 if not probed/available.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn gicv2m_base_phys() -> u64 {
    GICV2M_BASE_PHYS.load(Ordering::Relaxed)
}

/// GICv2m base SPI number (first available MSI SPI).
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn gicv2m_spi_base() -> u32 {
    GICV2M_SPI_BASE.load(Ordering::Relaxed) as u32
}

/// GICv2m number of available MSI SPIs.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn gicv2m_spi_count() -> u32 {
    GICV2M_SPI_COUNT.load(Ordering::Relaxed) as u32
}

/// Probe for GICv2m at the given physical address.
///
/// Reads MSI_TYPER (offset 0x008) to discover SPI range.
/// Returns true if a valid GICv2m frame was found.
#[cfg(target_arch = "aarch64")]
pub fn probe_gicv2m(phys_base: u64) -> bool {
    const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;
    let virt = (HHDM_BASE + phys_base) as *const u32;

    // DSB + ISB to ensure previous MMIO writes complete before reading device registers
    unsafe {
        core::arch::asm!("dsb sy", "isb", options(nomem, nostack));
    }

    // Read MSI_TYPER at offset 0x008
    let msi_typer = unsafe { core::ptr::read_volatile(virt.add(2)) }; // offset 8 / 4

    // MSI_TYPER (ARM IHI0048B §14.1):
    //   bits [25:16] = BASE_SPI: lowest SPI assigned to MSI
    //   bits [9:0]   = NUM_SPI: number of SPIs assigned to MSI
    let spi_base = (msi_typer >> 16) & 0x3FF;
    let spi_count = msi_typer & 0x3FF;

    // Log raw value for debugging
    crate::serial_println!(
        "[gicv2m] MSI_TYPER raw={:#010x} -> base_spi={}, num_spi={}",
        msi_typer, spi_base, spi_count,
    );

    if spi_count == 0 || spi_base == 0 || msi_typer == 0xFFFF_FFFF {
        return false;
    }

    GICV2M_BASE_PHYS.store(phys_base, Ordering::Relaxed);
    GICV2M_SPI_BASE.store(spi_base as u64, Ordering::Relaxed);
    GICV2M_SPI_COUNT.store(spi_count as u64, Ordering::Relaxed);
    true
}

/// Allocate the next available GICv2m MSI SPI.
///
/// Returns the SPI number (GIC INTID) for use with `configure_msi()` and
/// `gic::enable_spi()`. Returns 0 if GICv2m has not been probed or all
/// SPIs have been allocated.
///
/// Thread-safe: uses atomic fetch_add so multiple drivers (xHCI, GPU, etc.)
/// can allocate SPIs without collision.
#[cfg(target_arch = "aarch64")]
pub fn allocate_msi_spi() -> u32 {
    let base = GICV2M_SPI_BASE.load(Ordering::Relaxed);
    let count = GICV2M_SPI_COUNT.load(Ordering::Relaxed);
    if base == 0 || count == 0 {
        return 0;
    }
    let idx = GICV2M_NEXT_SPI.fetch_add(1, Ordering::Relaxed);
    if idx >= count {
        // Roll back — no SPI available
        GICV2M_NEXT_SPI.fetch_sub(1, Ordering::Relaxed);
        return 0;
    }
    (base + idx) as u32
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

/// Returns true if running on VMware Fusion.
/// Detected by non-zero RAM base offset (VMware places RAM at 0x80000000).
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn is_vmware() -> bool {
    ram_base_offset() > 0
}

/// Returns true if the physical timer (CNTP) should be used instead of the
/// virtual timer (CNTV). VMware Fusion doesn't deliver virtual timer (PPI 27)
/// interrupts to EL1 guests; use the physical timer (PPI 30) instead.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn use_physical_timer() -> bool {
    is_vmware()
}

/// RAM base offset for DMA address translation.
///
/// On QEMU/Parallels (offset=0): kernel VA 0x40xxxxxx maps to IPA 0x40xxxxxx.
/// On VMware (offset=0x40000000): kernel VA 0x40xxxxxx maps to IPA 0x80xxxxxx.
///
/// DMA controllers need IPAs (Intermediate Physical Addresses) to access
/// guest memory. The kernel's `virt_to_phys` must add this offset when
/// converting HHDM-mapped kernel addresses to DMA-usable physical addresses.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn ram_base_offset() -> u64 {
    RAM_BASE_OFFSET.load(Ordering::Relaxed)
}

/// Whether the parallels-loader already performed HCRST before ExitBootServices.
/// If true, the kernel should skip HCRST in xhci::init to avoid destroying
/// endpoint state that was created while the xHCI BAR was still active.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn xhci_hcrst_done() -> bool {
    // The loader now disconnects UEFI's xHCI driver instead of doing HCRST.
    // The kernel should always do its own HCRST. Return false unconditionally.
    false
}

/// Raw value of the xHCI HCRST sentinel (for diagnostics).
/// Sentinels: 0x00=untouched, 0xEE=reached, 0xCC=no ECAM, 0xDD=VID mismatch,
///            0xBB=BAR zero, 0x01=HCRST done
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn xhci_hcrst_done_raw() -> u64 {
    XHCI_HCRST_DONE.load(Ordering::Relaxed)
}

/// Boot wall clock time (Unix timestamp) from UEFI GetTime(). 0 if unavailable.
#[cfg(target_arch = "aarch64")]
#[inline]
pub fn boot_wall_time_utc() -> u64 {
    BOOT_WALL_TIME_UTC.load(Ordering::Relaxed)
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
    /// UART type: 0 = PL011, 1 = 16550
    pub uart_type: u8,
    pub _pad0: [u8; 3],
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
    pub xhci_hcrst_done: u32,
    pub _pad6: u32,
    pub xhci_bar_phys: u64,
    pub boot_wall_time_utc: u64,
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
    UART_TYPE.store(config.uart_type, Ordering::Relaxed);
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
        // Use the FIRST RAM region for ram_base_offset — this is the region
        // containing the kernel (loaded by the UEFI loader at its base).
        // On 8GB+ machines, UEFI splits RAM across a PCI hole: ~2GB at
        // 0x40000000 and ~6GB above 4GB. The largest region is above 4GB,
        // but the kernel lives in the first region. The loader uses
        // ram_regions[0].base for its offset, so we must match.
        let first_base = config.ram_regions[0].base;
        let first_size = config.ram_regions[0].size;

        // Compute RAM base offset for DMA address translation.
        // The kernel linker script assumes physical RAM starts at 0x40000000.
        // On VMware Fusion ARM64, RAM starts at 0x80000000, giving offset 0x40000000.
        // On QEMU/Parallels, RAM starts at 0x40000000, giving offset 0.
        let expected_ram_base: u64 = 0x4000_0000;
        let actual_ram_base = first_base & !0x3FFF_FFFF; // Round down to 1GB boundary
        if actual_ram_base > expected_ram_base {
            let offset = actual_ram_base - expected_ram_base;
            RAM_BASE_OFFSET.store(offset, Ordering::Relaxed);
        }

        if first_size > 0 {
            // Frame allocator starts after kernel image + BSS + SMP stacks (64 MB from RAM base).
            // BSS (0x0300_0000) includes PCI_3D_FRAMEBUFFER (~7.5 MB) and other large statics.
            // SMP stacks occupy 0x0300_0000 - 0x0400_0000 (8 CPUs × 2 MB = 16 MB).
            // 0x0400_0000 (64 MB) keeps the frame allocator safely clear of all static regions.
            let fa_start = first_base + 0x0400_0000; // +64 MB
            // Frame allocator must end BEFORE the DMA NC region.
            // The .dma section starts at physical 0x5000_0000, so cap fa_end there.
            // On VMware (RAM base 0x80000000), apply the offset to the DMA boundary.
            let dma_boundary = 0x5000_0000u64 + RAM_BASE_OFFSET.load(Ordering::Relaxed);
            let fa_end = (first_base + first_size).min(dma_boundary);
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

    // Store xHCI loader-level HCRST flag
    if config.xhci_hcrst_done != 0 {
        XHCI_HCRST_DONE.store(config.xhci_hcrst_done as u64, Ordering::Relaxed);
    }

    // Store boot wall clock time from UEFI GetTime()
    if config.boot_wall_time_utc != 0 {
        BOOT_WALL_TIME_UTC.store(config.boot_wall_time_utc, Ordering::Relaxed);
    }

    true
}
