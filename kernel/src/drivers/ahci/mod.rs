//! AHCI (Advanced Host Controller Interface) Storage Driver
//!
//! Implements the AHCI specification for SATA storage access.
//! Used on Parallels Desktop (ARM64) where storage is AHCI-based
//! rather than VirtIO block.
//!
//! # Architecture
//!
//! AHCI exposes a Host Bus Adapter (HBA) via PCI BAR5 (ABAR).
//! The HBA manages up to 32 ports, each connected to a SATA device.
//! Communication uses DMA with command lists and FIS (Frame Information
//! Structures) in host memory.
//!
//! # Memory Layout (per port)
//!
//! - Command List: 1 KB (32 × 32-byte command headers)
//! - Received FIS: 256 bytes
//! - Command Tables: 256 bytes each (CFIS + PRDT)

#![allow(dead_code)]

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::block::{BlockDevice, BlockError};
use crate::drivers::pci;

/// HHDM base for memory-mapped access.
const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Convert a kernel virtual address to a physical address.
///
/// On ARM64, the kernel runs in the higher half (HHDM at 0xFFFF_0000_0000_0000).
/// BSS statics are at VMA = HHDM + physical_address.
///
/// On VMware Fusion, the kernel runs from the L1[2] identity-mapped region
/// (VA 0x80xxxxxx → IPA 0x80xxxxxx), so HHDM addresses have the correct
/// IPA after subtracting HHDM_BASE — no offset needed.
///
/// On Parallels/QEMU, the kernel runs from the L1[1] identity-mapped region
/// (VA 0x40xxxxxx → IPA 0x40xxxxxx), same formula applies.
#[inline]
fn virt_to_phys(virt: u64) -> u64 {
    if virt >= HHDM_BASE {
        virt - HHDM_BASE
    } else {
        virt // Already a physical address (identity-mapped kernel)
    }
}

/// Clean (flush) a range of memory from CPU caches to the point of coherency.
///
/// Must be called after writing DMA descriptors/data and before issuing
/// DMA commands, so the device sees the updated data in physical memory.
#[cfg(target_arch = "aarch64")]
fn dma_cache_clean(ptr: *const u8, len: usize) {
    const CACHE_LINE: usize = 64;
    let start = ptr as usize & !(CACHE_LINE - 1);
    let end = (ptr as usize + len + CACHE_LINE - 1) & !(CACHE_LINE - 1);
    for addr in (start..end).step_by(CACHE_LINE) {
        unsafe {
            // Clean+Invalidate by VA to Point of Coherency — flush CPU cache
            // to RAM so the device sees the latest data, and invalidate so
            // subsequent reads re-fetch from RAM.
            core::arch::asm!("dc civac, {}", in(reg) addr, options(nostack));
        }
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// Invalidate a range of memory in CPU caches after a device DMA write.
///
/// Must be called after a DMA read completes and before the CPU reads
/// the DMA buffer, to ensure the CPU sees the device-written data.
#[cfg(target_arch = "aarch64")]
fn dma_cache_invalidate(ptr: *const u8, len: usize) {
    const CACHE_LINE: usize = 64;
    let start = ptr as usize & !(CACHE_LINE - 1);
    let end = (ptr as usize + len + CACHE_LINE - 1) & !(CACHE_LINE - 1);
    for addr in (start..end).step_by(CACHE_LINE) {
        unsafe {
            // Clean+Invalidate by VA to Point of Coherency.
            // On NC memory this is a no-op; the dsb sy after the loop provides
            // the actual ordering guarantee. On cacheable memory (fallback),
            // civac ensures stale cache lines are discarded.
            core::arch::asm!("dc civac, {}", in(reg) addr, options(nostack));
        }
    }
    unsafe {
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// No-op cache maintenance on x86_64 (DMA coherent by default).
#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn dma_cache_clean(_ptr: *const u8, _len: usize) {}

/// No-op cache maintenance on x86_64 (DMA coherent by default).
#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn dma_cache_invalidate(_ptr: *const u8, _len: usize) {}

/// Sector size in bytes (standard for SATA).
pub const SECTOR_SIZE: usize = 512;

/// Command timeout in seconds (matches Linux ata_internal_cmd_timeout).
const AHCI_TIMEOUT_SECS: u64 = 5;

/// Read the ARM64 generic timer counter (CNTPCT_EL0).
/// This is a free-running counter available at all times, independent of
/// whether the timer interrupt is configured.
#[cfg(target_arch = "aarch64")]
#[inline]
fn read_cntpct() -> u64 {
    let val: u64;
    unsafe { core::arch::asm!("mrs {}, cntpct_el0", out(reg) val, options(nomem, nostack)); }
    val
}

/// Read both CNTPCT_EL0 and CNTFRQ_EL0 (counter value and frequency).
#[cfg(target_arch = "aarch64")]
#[inline]
fn read_cntpct_and_freq() -> (u64, u64) {
    let cnt: u64;
    let freq: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntpct_el0", out(reg) cnt, options(nomem, nostack));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq, options(nomem, nostack));
    }
    (cnt, freq)
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn read_cntpct() -> u64 { 0 }

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn read_cntpct_and_freq() -> (u64, u64) { (0, 1) }

/// Maximum number of AHCI ports.
const MAX_PORTS: usize = 32;

/// Maximum number of command slots per port.
const MAX_CMD_SLOTS: usize = 32;

/// AHCI port register block size.
const PORT_REG_SIZE: usize = 0x80;

/// Whether AHCI has been initialized.
static AHCI_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Global AHCI controller state.
static AHCI_CONTROLLER: Mutex<Option<AhciController>> = Mutex::new(None);

/// Virtual base address of the AHCI ABAR (for use in interrupt handler).
/// 0 = driver not yet initialised.
static AHCI_ABAR: AtomicU64 = AtomicU64::new(0);

/// GIC SPI number allocated for AHCI MSI/MSI-X/wired. 0 = polling mode.
static AHCI_IRQ: AtomicU32 = AtomicU32::new(0);

/// Whether the AHCI IRQ is edge-triggered (MSI) or level-triggered (wired).
/// For edge-triggered, the ISR must clear the SPI pending bit.
/// For level-triggered, clearing PORT_IS de-asserts the line; no SPI clear needed.
static AHCI_IRQ_EDGE: AtomicBool = AtomicBool::new(true);

/// Per-port command-completion flag set by the MSI interrupt handler.
/// Initialised false, cleared before issuing each command, set on completion.
static AHCI_PORT_COMPLETE: [AtomicBool; MAX_AHCI_PORTS] = [
    const { AtomicBool::new(false) }; MAX_AHCI_PORTS
];

/// Count ISR invocations (for diagnostic/timeout reporting).
static AHCI_ISR_COUNT: AtomicU32 = AtomicU32::new(0);
/// Count commands issued via issue_cmd_slot0 (for diagnostic/timeout reporting).
static AHCI_CMD_COUNT: AtomicU32 = AtomicU32::new(0);

// =============================================================================
// HBA Generic Host Control Registers (offset from ABAR)
// =============================================================================

/// Host Capabilities
const HBA_CAP: usize = 0x00;
/// Global Host Control
const HBA_GHC: usize = 0x04;
/// Interrupt Status
const HBA_IS: usize = 0x08;
/// Ports Implemented
const HBA_PI: usize = 0x0C;
/// Version
const HBA_VS: usize = 0x10;

/// GHC bits
const GHC_HR: u32 = 1 << 0;    // HBA Reset
const GHC_IE: u32 = 1 << 1;    // Interrupt Enable
const GHC_AE: u32 = 1 << 31;   // AHCI Enable

// =============================================================================
// Port Registers (offset from ABAR + 0x100 + port * 0x80)
// =============================================================================

/// Command List Base Address (low)
const PORT_CLB: usize = 0x00;
/// Command List Base Address (high)
const PORT_CLBU: usize = 0x04;
/// FIS Base Address (low)
const PORT_FB: usize = 0x08;
/// FIS Base Address (high)
const PORT_FBU: usize = 0x0C;
/// Interrupt Status
const PORT_IS: usize = 0x10;
/// Interrupt Enable
const PORT_IE: usize = 0x14;
/// Command and Status
const PORT_CMD: usize = 0x18;
/// Task File Data
const PORT_TFD: usize = 0x20;
/// Signature
const PORT_SIG: usize = 0x24;
/// SATA Status (SCR0: SStatus)
const PORT_SSTS: usize = 0x28;
/// SATA Control (SCR2: SControl)
const PORT_SCTL: usize = 0x2C;
/// SATA Error (SCR1: SError)
const PORT_SERR: usize = 0x30;
/// SATA Active
const PORT_SACT: usize = 0x34;
/// Command Issue
const PORT_CI: usize = 0x38;

/// PORT_CMD bits
const PORT_CMD_ST: u32 = 1 << 0;   // Start
const PORT_CMD_FRE: u32 = 1 << 4;  // FIS Receive Enable
const PORT_CMD_FR: u32 = 1 << 14;  // FIS Receive Running
const PORT_CMD_CR: u32 = 1 << 15;  // Command List Running

/// PORT_TFD bits
const PORT_TFD_BSY: u32 = 1 << 7;  // Busy
const PORT_TFD_DRQ: u32 = 1 << 3;  // Data Request

/// PORT_IS interrupt status bits (from Linux libahci.h)
const PORT_IRQ_D2H_REG_FIS: u32 = 1 << 0;    // D2H Register FIS received (command complete)
const PORT_IRQ_PIO_FIS: u32 = 1 << 1;         // PIO Setup FIS received
const PORT_IRQ_DMA_FIS: u32 = 1 << 2;         // DMA Setup FIS received
const PORT_IRQ_SDB_FIS: u32 = 1 << 3;         // Set Device Bits FIS received
const PORT_IRQ_UNK_FIS: u32 = 1 << 4;         // Unknown FIS received
const PORT_IRQ_SG_DONE: u32 = 1 << 5;         // Descriptor processed
const PORT_IRQ_CONNECT: u32 = 1 << 6;         // Port connect change
const PORT_IRQ_DMPS: u32 = 1 << 7;            // Device mechanical presence
const PORT_IRQ_PHYRDY: u32 = 1 << 22;         // PhyRdy changed
const PORT_IRQ_BAD_PMP: u32 = 1 << 23;        // Bad PMP status
const PORT_IRQ_OVERFLOW: u32 = 1 << 24;       // Overflow status
const PORT_IRQ_IF_NONFATAL: u32 = 1 << 26;    // Interface non-fatal error
const PORT_IRQ_IF_ERR: u32 = 1 << 27;         // Interface fatal error
const PORT_IRQ_HBUS_DATA_ERR: u32 = 1 << 28;  // Host bus data error
const PORT_IRQ_HBUS_ERR: u32 = 1 << 29;       // Host bus fatal error
const PORT_IRQ_TF_ERR: u32 = 1 << 30;         // Task file error (TFES)
const PORT_IRQ_FREEZE: u32 = PORT_IRQ_HBUS_ERR | PORT_IRQ_IF_ERR | PORT_IRQ_CONNECT
    | PORT_IRQ_PHYRDY | PORT_IRQ_UNK_FIS | PORT_IRQ_BAD_PMP;
const PORT_IRQ_ERROR: u32 = PORT_IRQ_FREEZE | PORT_IRQ_TF_ERR
    | PORT_IRQ_HBUS_DATA_ERR | PORT_IRQ_IF_NONFATAL | PORT_IRQ_OVERFLOW;

/// Completion mask: any of these PORT_IS bits signals a command finished.
/// D2H Register FIS = normal completion; PIO FIS = PIO command completion.
const PORT_IRQ_COMPLETE: u32 = PORT_IRQ_D2H_REG_FIS | PORT_IRQ_PIO_FIS | PORT_IRQ_SDB_FIS;

/// SATA Status (SSTS) - device detection
const SSTS_DET_MASK: u32 = 0x0F;
const SSTS_DET_PRESENT: u32 = 0x03;  // Device detected, Phy communication established

/// Device signatures
const SIG_SATA: u32 = 0x00000101;    // SATA drive
const SIG_ATAPI: u32 = 0xEB140101;   // SATAPI device

// =============================================================================
// FIS Types
// =============================================================================

/// Host to Device FIS type
const FIS_TYPE_REG_H2D: u8 = 0x27;

// =============================================================================
// ATA Commands
// =============================================================================

/// IDENTIFY DEVICE
const ATA_CMD_IDENTIFY: u8 = 0xEC;
/// READ DMA EXT (48-bit LBA)
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
/// WRITE DMA EXT (48-bit LBA)
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
/// FLUSH CACHE EXT
const ATA_CMD_FLUSH_EXT: u8 = 0xEA;

// =============================================================================
// DMA Memory Structures
// =============================================================================

/// Command List entry (Command Header) - 32 bytes each, 32 per port.
#[repr(C, packed)]
struct CmdHeader {
    /// DW0: Command FIS length (bits 4:0), ATAPI (bit 5), Write (bit 6), Prefetchable (bit 7)
    ///      Reset (bit 8), BIST (bit 9), Clear BSY on R_OK (bit 10), Port Multiplier (15:12)
    ///      PRDTL (31:16) = Physical Region Descriptor Table Length
    dw0: u32,
    /// DW1: Physical Region Descriptor Byte Count (bytes transferred)
    prdbc: u32,
    /// DW2: Command Table Descriptor Base Address (low)
    ctba: u32,
    /// DW3: Command Table Descriptor Base Address (high)
    ctbau: u32,
    /// DW4-7: Reserved
    _reserved: [u32; 4],
}

/// Physical Region Descriptor Table entry - 16 bytes.
#[repr(C, packed)]
struct PrdtEntry {
    /// Data Base Address (low)
    dba: u32,
    /// Data Base Address (high)
    dbau: u32,
    /// Reserved
    _reserved: u32,
    /// Data Byte Count (bits 21:0) and Interrupt on Completion (bit 31)
    dbc: u32,
}

/// Host to Device FIS (Register) - 20 bytes.
#[repr(C, packed)]
struct FisRegH2d {
    /// FIS type (0x27)
    fis_type: u8,
    /// Port multiplier (bits 3:0), reserved (bits 6:4), C bit (bit 7) = command/control
    pmport_c: u8,
    /// Command register
    command: u8,
    /// Feature register (low)
    featurel: u8,
    /// LBA low
    lba0: u8,
    /// LBA mid
    lba1: u8,
    /// LBA high
    lba2: u8,
    /// Device register
    device: u8,
    /// LBA low (exp)
    lba3: u8,
    /// LBA mid (exp)
    lba4: u8,
    /// LBA high (exp)
    lba5: u8,
    /// Feature register (high)
    featureh: u8,
    /// Count (low)
    countl: u8,
    /// Count (high)
    counth: u8,
    /// Isochronous Command Completion
    icc: u8,
    /// Control register
    control: u8,
    /// Reserved
    _reserved: [u8; 4],
}

/// Command Table - contains the Command FIS and PRDT entries.
/// We use a fixed single-PRDT layout for simplicity.
#[repr(C, align(128))]
struct CmdTable {
    /// Command FIS (up to 64 bytes)
    cfis: [u8; 64],
    /// ATAPI Command (16 bytes)
    acmd: [u8; 16],
    /// Reserved (48 bytes)
    _reserved: [u8; 48],
    /// PRDT entries (we use 1 entry for single-sector operations)
    prdt: [PrdtEntry; 1],
}

/// Received FIS structure - 256 bytes per port.
#[repr(C, align(256))]
struct ReceivedFis {
    data: [u8; 256],
}

/// Per-port DMA memory allocation.
///
/// All memory must be physically contiguous and accessible via DMA.
/// We use static allocations with known physical addresses.
#[repr(C, align(4096))]
struct PortDmaMem {
    /// Command list (32 headers × 32 bytes = 1024 bytes)
    cmd_list: [CmdHeader; MAX_CMD_SLOTS],
    /// Received FIS area
    received_fis: ReceivedFis,
    /// Command table for slot 0 (we only use slot 0 for simplicity)
    cmd_table: CmdTable,
    /// DMA buffer for sector I/O (one sector)
    dma_buf: [u8; SECTOR_SIZE],
}

/// Static DMA memory for up to 2 ports.
/// These are page-aligned for DMA safety.
const MAX_AHCI_PORTS: usize = 4;
static PORT_DMA: Mutex<[Option<&'static mut PortDmaMem>; MAX_AHCI_PORTS]> =
    Mutex::new([const { None }; MAX_AHCI_PORTS]);

// We use a static array for DMA memory so we know the physical addresses.
#[repr(C, align(4096))]
struct PortDmaStorage {
    ports: [PortDmaMem; MAX_AHCI_PORTS],
}

#[link_section = ".dma"]
static mut DMA_STORAGE: PortDmaStorage = unsafe { core::mem::zeroed() };

// =============================================================================
// AHCI Controller
// =============================================================================

/// AHCI controller state.
struct AhciController {
    /// Virtual base address of the HBA registers (ABAR via HHDM)
    abar_virt: u64,
    /// Number of command slots supported
    num_cmd_slots: u32,
    /// Bitmask of implemented ports
    ports_implemented: u32,
    /// Port states
    ports: [Option<AhciPort>; MAX_PORTS],
}

/// Per-port state.
struct AhciPort {
    /// Port number (0-31)
    port_num: usize,
    /// Device type
    device_type: DeviceType,
    /// Sector count (from IDENTIFY DEVICE)
    sector_count: u64,
    /// DMA memory index in DMA_STORAGE
    dma_index: usize,
}

/// AHCI device type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceType {
    Sata,
    Atapi,
    Unknown,
}

// =============================================================================
// Register Access Helpers
// =============================================================================

#[inline]
fn hba_read(abar: u64, offset: usize) -> u32 {
    unsafe { core::ptr::read_volatile((abar + offset as u64) as *const u32) }
}

#[inline]
fn hba_write(abar: u64, offset: usize, value: u32) {
    unsafe { core::ptr::write_volatile((abar + offset as u64) as *mut u32, value) }
}

/// MMIO read with post-load barrier (ARM64 readl semantics).
/// Use at completion checkpoints, not in hot polling loops.
#[inline]
#[cfg(target_arch = "aarch64")]
fn hba_read_barrier(abar: u64, offset: usize) -> u32 {
    let val = unsafe { core::ptr::read_volatile((abar + offset as u64) as *const u32) };
    unsafe { core::arch::asm!("dsb ld", options(nostack, preserves_flags)); }
    val
}

/// MMIO write with pre-store barrier (ARM64 writel semantics).
/// Use when Normal memory stores must be visible before the MMIO write.
#[inline]
#[cfg(target_arch = "aarch64")]
fn hba_write_barrier(abar: u64, offset: usize, value: u32) {
    unsafe { core::arch::asm!("dsb st", options(nostack, preserves_flags)); }
    unsafe { core::ptr::write_volatile((abar + offset as u64) as *mut u32, value) }
}

#[inline]
fn port_base(abar: u64, port: usize) -> u64 {
    abar + 0x100 + (port as u64) * PORT_REG_SIZE as u64
}

#[inline]
fn port_read(abar: u64, port: usize, offset: usize) -> u32 {
    hba_read(port_base(abar, port), offset)
}

#[inline]
fn port_write(abar: u64, port: usize, offset: usize, value: u32) {
    hba_write(port_base(abar, port), offset, value)
}

// =============================================================================
// Controller Implementation
// =============================================================================

impl AhciController {
    /// Create and initialize an AHCI controller from a PCI device.
    fn init(pci_dev: &pci::Device) -> Result<Self, &'static str> {
        // AHCI uses BAR5 (ABAR)
        let bar5 = &pci_dev.bars[5];
        if !bar5.is_valid() || bar5.is_io {
            return Err("AHCI: BAR5 not valid or not MMIO");
        }

        let abar_virt = HHDM_BASE + bar5.address;

        // Enable PCI bus mastering and memory space
        pci_dev.enable_bus_master();
        pci_dev.enable_memory_space();

        let controller = Self::init_common(abar_virt)?;

        // Set up MSI/MSI-X interrupt after the controller is running.
        #[cfg(target_arch = "aarch64")]
        setup_ahci_msi(pci_dev);

        Ok(controller)
    }

    /// Create and initialize an AHCI controller from a known MMIO base address.
    ///
    /// Used for platform devices (e.g., Parallels Desktop) where the AHCI
    /// controller is not on the PCI bus but at a fixed MMIO address.
    /// After port initialization, probes for the wired GIC SPI so subsequent
    /// I/O uses interrupt-driven completion instead of MMIO polling.
    fn init_from_mmio(abar_phys: u64) -> Result<Self, &'static str> {
        let abar_virt = HHDM_BASE + abar_phys;

        crate::serial_println!("[ahci] Platform AHCI at phys {:#x}, virt {:#x}", abar_phys, abar_virt);

        let controller = Self::init_common(abar_virt)?;

        // Probe for the wired SPI. Issues a fresh IDENTIFY command and
        // checks GICD_ISPENDR while PORT_IS is still set (interrupt asserted).
        #[cfg(target_arch = "aarch64")]
        probe_platform_irq(&controller);

        Ok(controller)
    }

    /// Common AHCI controller initialization.
    ///
    /// Enables AHCI mode, reads capabilities, discovers ports, and
    /// issues IDENTIFY DEVICE to each connected SATA drive.
    fn init_common(abar_virt: u64) -> Result<Self, &'static str> {
        // Store ABAR for the interrupt handler (must happen before any port init
        // that might issue commands).
        AHCI_ABAR.store(abar_virt, Ordering::Release);

        // Enable AHCI mode and global interrupt enable.
        // GHC_IE (bit 1) allows the HBA to generate MSI/INTx completions.
        let ghc = hba_read(abar_virt, HBA_GHC);
        hba_write(abar_virt, HBA_GHC, ghc | GHC_AE | GHC_IE);

        // Read capabilities
        let cap = hba_read(abar_virt, HBA_CAP);
        let num_cmd_slots = ((cap >> 8) & 0x1F) + 1;
        let num_ports = (cap & 0x1F) + 1;
        let ports_implemented = hba_read(abar_virt, HBA_PI);
        let version = hba_read(abar_virt, HBA_VS);

        crate::serial_println!(
            "[ahci] HBA version {}.{}, {} ports, {} cmd slots, PI={:#010x}",
            version >> 16,
            version & 0xFFFF,
            num_ports,
            num_cmd_slots,
            ports_implemented,
        );

        // Initialize DMA memory references
        let dma_storage_ptr = &raw mut DMA_STORAGE;
        let mut dma_lock = PORT_DMA.lock();
        for i in 0..MAX_AHCI_PORTS {
            dma_lock[i] = Some(unsafe { &mut (*dma_storage_ptr).ports[i] });
        }
        drop(dma_lock);

        let mut controller = AhciController {
            abar_virt,
            num_cmd_slots,
            ports_implemented,
            ports: core::array::from_fn(|_| None),
        };

        // Discover and initialize ports
        let mut dma_index = 0;
        for port_num in 0..MAX_PORTS {
            if (ports_implemented & (1 << port_num)) == 0 {
                continue;
            }
            if dma_index >= MAX_AHCI_PORTS {
                crate::serial_println!("[ahci] Warning: more ports than DMA slots, skipping port {}", port_num);
                continue;
            }

            if let Some(port) = controller.init_port(port_num, dma_index) {
                crate::serial_println!(
                    "[ahci] Port {}: {:?}, {} sectors ({} MB)",
                    port_num,
                    port.device_type,
                    port.sector_count,
                    port.sector_count * SECTOR_SIZE as u64 / (1024 * 1024),
                );
                controller.ports[port_num] = Some(port);
                dma_index += 1;
            }
        }

        Ok(controller)
    }

    /// Initialize a single port. Returns None if no device is present.
    fn init_port(&self, port_num: usize, dma_index: usize) -> Option<AhciPort> {
        let abar = self.abar_virt;

        // Check SATA Status for device presence
        let ssts = port_read(abar, port_num, PORT_SSTS);
        if (ssts & SSTS_DET_MASK) != SSTS_DET_PRESENT {
            return None;
        }

        // Stop command engine before reconfiguring
        self.stop_cmd(port_num);

        // Compute DMA physical address from the PORT_DMA reference (not &raw const DMA_STORAGE).
        // On VMware ARM64, the kernel runs at VA 0x80xxxxxx (shifted from linker VA 0x40xxxxxx).
        // &raw const DMA_STORAGE can produce inconsistent ADRP-based addresses depending on
        // the inlining context. The PORT_DMA reference was set up once during init_common
        // and has the correct runtime address.
        let dma_lock = PORT_DMA.lock();
        let dma_phys = if let Some(dma_mem) = &dma_lock[dma_index] {
            let ptr = *dma_mem as *const PortDmaMem as *mut u8;
            let size = core::mem::size_of::<PortDmaMem>();
            unsafe {
                core::ptr::write_bytes(ptr, 0, size);
            }
            dma_cache_clean(ptr as *const u8, size);
            virt_to_phys(ptr as u64)
        } else {
            drop(dma_lock);
            return None;
        };
        drop(dma_lock);

        // Command List Base
        let clb_phys = dma_phys;
        port_write(abar, port_num, PORT_CLB, clb_phys as u32);
        port_write(abar, port_num, PORT_CLBU, (clb_phys >> 32) as u32);

        // FIS Base
        let fb_phys = dma_phys + core::mem::offset_of!(PortDmaMem, received_fis) as u64;
        port_write(abar, port_num, PORT_FB, fb_phys as u32);
        port_write(abar, port_num, PORT_FBU, (fb_phys >> 32) as u32);

        // Clear interrupt status and error
        port_write(abar, port_num, PORT_IS, 0xFFFF_FFFF);
        port_write(abar, port_num, PORT_SERR, 0xFFFF_FFFF);

        // Enable port interrupt sources.
        // Linux uses AHCI_DEF_PORT_IRQ which covers D2H, PIO FIS, DMA FIS,
        // SDB FIS, and all error bits.  We enable the same set so the HBA
        // will fire an MSI when any of these events occur.
        let port_ie = PORT_IRQ_COMPLETE | PORT_IRQ_ERROR;
        port_write(abar, port_num, PORT_IE, port_ie);

        // Start command engine
        self.start_cmd(port_num);

        // Determine device type from signature
        let sig = port_read(abar, port_num, PORT_SIG);
        let device_type = match sig {
            SIG_SATA => DeviceType::Sata,
            SIG_ATAPI => DeviceType::Atapi,
            _ => DeviceType::Unknown,
        };

        // For SATA devices, issue IDENTIFY DEVICE to get sector count
        let sector_count = if device_type == DeviceType::Sata {
            self.identify_device(port_num, dma_index).unwrap_or(0)
        } else {
            0
        };

        Some(AhciPort {
            port_num,
            device_type,
            sector_count,
            dma_index,
        })
    }

    /// Stop the command engine for a port.
    fn stop_cmd(&self, port: usize) {
        let abar = self.abar_virt;
        let mut cmd = port_read(abar, port, PORT_CMD);

        // Clear ST (Start)
        cmd &= !PORT_CMD_ST;
        port_write(abar, port, PORT_CMD, cmd);

        // Wait for CR (Command List Running) to clear
        for _ in 0..1_000_000 {
            if (port_read(abar, port, PORT_CMD) & PORT_CMD_CR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Clear FRE (FIS Receive Enable)
        cmd = port_read(abar, port, PORT_CMD);
        cmd &= !PORT_CMD_FRE;
        port_write(abar, port, PORT_CMD, cmd);

        // Wait for FR (FIS Receive Running) to clear
        for _ in 0..1_000_000 {
            if (port_read(abar, port, PORT_CMD) & PORT_CMD_FR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }

    /// Start the command engine for a port.
    fn start_cmd(&self, port: usize) {
        let abar = self.abar_virt;

        // Wait for CR to clear
        for _ in 0..1_000_000 {
            if (port_read(abar, port, PORT_CMD) & PORT_CMD_CR) == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Enable FRE, then ST
        let mut cmd = port_read(abar, port, PORT_CMD);
        cmd |= PORT_CMD_FRE;
        port_write(abar, port, PORT_CMD, cmd);

        cmd |= PORT_CMD_ST;
        port_write(abar, port, PORT_CMD, cmd);
    }

    /// Wait for port to be not busy.
    fn wait_ready(&self, port: usize) -> Result<(), &'static str> {
        let abar = self.abar_virt;
        for _ in 0..1_000_000 {
            let tfd = port_read(abar, port, PORT_TFD);
            if (tfd & (PORT_TFD_BSY | PORT_TFD_DRQ)) == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err("AHCI: port busy timeout")
    }

    /// Issue a command on slot 0 and wait for completion.
    ///
    /// Uses interrupt-driven completion matching Linux's ahci_qc_issue +
    /// ahci_port_intr design:
    ///
    /// 1. Clear stale PORT_IS / HBA_IS.
    /// 2. Verify PORT_CMD.ST (command engine running).
    /// 3. Write PORT_CI = 1 (issue command slot 0).
    /// 4. Sleep via `wfi` (Wait For Interrupt) until the ISR sets
    ///    AHCI_PORT_COMPLETE, or a timer tick wakes us for a PORT_CI check.
    /// 5. On completion: clear PORT_IS then HBA_IS (order per AHCI §10.7.2.1).
    /// 6. Check PORT_TFD for errors.
    ///
    /// Timeout is wall-clock based via CNTPCT_EL0 (5 seconds, matching
    /// Linux's ata_internal_cmd_timeout for non-NCQ commands).
    ///
    /// If the port is in ERR:Fatal state (TFES set), the command engine is
    /// stopped, errors cleared, and the engine restarted before issuing.
    fn issue_cmd_slot0(&self, port: usize) -> Result<(), &'static str> {
        let abar = self.abar_virt;

        // --- Error recovery: clear ERR:Fatal state ---
        {
            let is = port_read(abar, port, PORT_IS);
            let tfd = port_read(abar, port, PORT_TFD);
            if (is & PORT_IRQ_TF_ERR) != 0 || (tfd & 1) != 0 {
                self.stop_cmd(port);
                port_write(abar, port, PORT_SERR, 0xFFFF_FFFF);
                port_write(abar, port, PORT_IS, 0xFFFF_FFFF);
                hba_write(abar, HBA_IS, 1u32 << (port as u32));
                self.start_cmd(port);
                self.wait_ready(port)?;
            }
        }

        // --- Clear stale interrupt status ---
        let stale_is = port_read(abar, port, PORT_IS);
        if stale_is != 0 {
            port_write(abar, port, PORT_IS, stale_is);
        }
        hba_write(abar, HBA_IS, 1u32 << (port as u32));

        // --- Verify command engine ---
        let cmd = port_read(abar, port, PORT_CMD);
        if (cmd & PORT_CMD_ST) == 0 {
            return Err("AHCI: command engine not running");
        }

        // --- Clear completion flag and issue ---
        let has_irq = AHCI_IRQ.load(Ordering::Relaxed) != 0 && port < MAX_AHCI_PORTS;
        if has_irq {
            AHCI_PORT_COMPLETE[port].store(false, Ordering::Release);
        }

        let cmd_num = AHCI_CMD_COUNT.fetch_add(1, Ordering::Relaxed);

        port_write(abar, port, PORT_CI, 1);

        // --- Wait for completion with wall-clock timeout ---
        //
        // Read the ARM64 generic timer counter for a real-time deadline.
        // CNTPCT_EL0 and CNTFRQ_EL0 are always available (free-running
        // counter independent of timer interrupt configuration).
        let (start, freq) = read_cntpct_and_freq();
        let deadline = start + freq * AHCI_TIMEOUT_SECS; // 5-second deadline

        loop {
            // --- Check for completion ---
            //
            // Primary: interrupt flag from ISR (SPI 34). With DAIF unmasked
            // in syscall_entry.S, this handles 99.99%+ of completions.
            //
            // Safety net: PORT_CI check catches rare cases where the GIC
            // doesn't deliver a pending SPI despite DAIF.I=0 (observed as
            // ~2 out of 19,779 commands on Parallels — likely vCPU scheduling
            // latency in the hypervisor). Linux avoids this because its
            // wait_for_completion uses proper sleep/wake, not wfi.
            if has_irq && AHCI_PORT_COMPLETE[port].load(Ordering::Acquire) {
                let tfd = port_read(abar, port, PORT_TFD);
                if (tfd & 1) != 0 {
                    return Err("AHCI: task file error");
                }
                return Ok(());
            }

            let ci = port_read(abar, port, PORT_CI);
            if (ci & 1) == 0 {
                let is = port_read(abar, port, PORT_IS);
                let tfd = port_read(abar, port, PORT_TFD);
                port_write(abar, port, PORT_IS, is);
                hba_write(abar, HBA_IS, 1u32 << (port as u32));
                if (is & PORT_IRQ_ERROR) != 0 || (tfd & 1) != 0 {
                    return Err("AHCI: task file error");
                }
                return Ok(());
            }

            let is = port_read(abar, port, PORT_IS);
            if (is & PORT_IRQ_ERROR) != 0 {
                port_write(abar, port, PORT_IS, is);
                hba_write(abar, HBA_IS, 1u32 << (port as u32));
                return Err("AHCI: task file error");
            }

            // --- Check wall-clock timeout ---
            let now = read_cntpct();
            if now >= deadline {
                let ci = port_read(abar, port, PORT_CI);
                let is = port_read(abar, port, PORT_IS);
                let tfd = port_read(abar, port, PORT_TFD);
                let hba_is_timeout = hba_read(abar, HBA_IS);
                let ghc = hba_read(abar, HBA_GHC);
                let port_ie = port_read(abar, port, PORT_IE);
                let port_cmd = port_read(abar, port, PORT_CMD);
                let serr = port_read(abar, port, PORT_SERR);
                let isr_count = AHCI_ISR_COUNT.load(Ordering::Relaxed);
                // Read GIC state for the AHCI SPI
                #[cfg(target_arch = "aarch64")]
                let (ahci_spi, gic_pending, gic_active, pend_snap) = {
                    use crate::arch_impl::aarch64::gic;
                    let spi = AHCI_IRQ.load(Ordering::Relaxed);
                    let p = if spi != 0 { gic::is_pending(spi) } else { false };
                    let a = if spi != 0 { gic::is_active(spi) } else { false };
                    let snap = gic::snapshot_pending_spis();
                    (spi, p, a, snap)
                };
                #[cfg(not(target_arch = "aarch64"))]
                let (ahci_spi, gic_pending, gic_active, pend_snap) = (0u32, false, false, [0u32; 3]);
                // Read DAIF to check if IRQs are masked
                let daif: u64;
                #[cfg(target_arch = "aarch64")]
                unsafe { core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack)); }
                #[cfg(not(target_arch = "aarch64"))]
                { daif = 0; }
                crate::serial_println!(
                    "[ahci] Port {} TIMEOUT ({}s): CI={:#x} IS={:#x} TFD={:#x} HBA_IS={:#x}",
                    port, AHCI_TIMEOUT_SECS, ci, is, tfd, hba_is_timeout
                );
                crate::serial_println!(
                    "[ahci]   GHC={:#x} PORT_IE={:#x} CMD={:#x} SERR={:#x}",
                    ghc, port_ie, port_cmd, serr
                );
                crate::serial_println!(
                    "[ahci]   GIC: SPI{} pend={} act={} DAIF={:#x} pend_snap=[{:#x},{:#x},{:#x}]",
                    ahci_spi, gic_pending, gic_active, daif, pend_snap[0], pend_snap[1], pend_snap[2]
                );
                crate::serial_println!(
                    "[ahci]   isr_count={} cmd#={} complete_flag={}",
                    isr_count, cmd_num, AHCI_PORT_COMPLETE[port].load(Ordering::Relaxed)
                );
                return Err("AHCI: command timeout");
            }

            // --- Sleep until next interrupt ---
            //
            // When an interrupt source exists (AHCI IRQ registered, or timer
            // running), use `wfi` (Wait For Interrupt) to halt the CPU until
            // the next interrupt fires. This limits MMIO reads to at most
            // 1 per timer tick (~1000/s) instead of millions in a tight loop.
            //
            // During early boot (no timer, no AHCI IRQ yet), `wfi` would
            // block forever. Fall back to `wfe` (Wait For Event) which wakes
            // on any event flag — less power-efficient but always returns.
            // Sleep until next interrupt or event.
            //
            // Use wfi (Wait For Interrupt) when we have a guaranteed wake
            // source: either the AHCI completion interrupt OR the timer tick.
            // During early boot (before timer_interrupt::init), neither may
            // exist, so fall back to wfe (Wait For Event) which always returns.
            #[cfg(target_arch = "aarch64")]
            {
                let timer_running = crate::arch_impl::aarch64::timer_interrupt::timer_is_running();
                if timer_running {
                    unsafe { core::arch::asm!("wfi", options(nomem, nostack)); }
                } else {
                    unsafe { core::arch::asm!("wfe", options(nomem, nostack)); }
                }
            }
            #[cfg(not(target_arch = "aarch64"))]
            core::hint::spin_loop();
        }
    }

    /// Issue IDENTIFY DEVICE and return sector count.
    fn identify_device(&self, port: usize, dma_index: usize) -> Result<u64, &'static str> {
        self.wait_ready(port)?;

        let mut dma_lock = PORT_DMA.lock();
        let dma = dma_lock[dma_index].as_mut().ok_or("AHCI: no DMA memory")?;

        // Compute physical addresses from the dma reference (see read_sector comment).
        let cmd_table_phys = virt_to_phys(&dma.cmd_table as *const CmdTable as u64);
        let dma_buf_phys = virt_to_phys(dma.dma_buf.as_ptr() as u64);

        // Command header: CFL=5 (5 dwords = 20 bytes for H2D FIS), 1 PRDT entry
        dma.cmd_list[0].dw0 = (1 << 16) | 5; // PRDTL=1, CFL=5
        dma.cmd_list[0].prdbc = 0;
        dma.cmd_list[0].ctba = cmd_table_phys as u32;
        dma.cmd_list[0].ctbau = (cmd_table_phys >> 32) as u32;

        // Zero the command table
        dma.cmd_table.cfis = [0; 64];
        dma.cmd_table.acmd = [0; 16];

        // Set up H2D FIS for IDENTIFY DEVICE
        dma.cmd_table.cfis[0] = FIS_TYPE_REG_H2D;
        dma.cmd_table.cfis[1] = 0x80; // C bit = 1 (command)
        dma.cmd_table.cfis[2] = ATA_CMD_IDENTIFY;
        dma.cmd_table.cfis[7] = 0; // Device = 0

        // PRDT: point to DMA buffer, 512 bytes
        dma.cmd_table.prdt[0].dba = dma_buf_phys as u32;
        dma.cmd_table.prdt[0].dbau = (dma_buf_phys >> 32) as u32;
        dma.cmd_table.prdt[0]._reserved = 0;
        dma.cmd_table.prdt[0].dbc = (SECTOR_SIZE as u32 - 1) | (1 << 31); // byte count - 1, IOC

        // Ensure CPU writes are visible to the DMA device
        core::sync::atomic::fence(Ordering::SeqCst);
        {
            let dma_ptr = &**dma as *const PortDmaMem as *const u8;
            dma_cache_clean(dma_ptr, core::mem::size_of::<PortDmaMem>());
        }

        drop(dma_lock);

        // Issue the command
        self.issue_cmd_slot0(port)?;

        // DSB SY: order PORT_CI completion against DMA buffer cache maintenance
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)); }

        // Invalidate cache for DMA buffer before reading device-written data
        let dma_lock = PORT_DMA.lock();
        let dma = dma_lock[dma_index].as_ref().ok_or("AHCI: no DMA memory")?;
        {
            let buf_ptr = dma.dma_buf.as_ptr();
            dma_cache_invalidate(buf_ptr, SECTOR_SIZE);
        }

        // Words 100-103 contain the 48-bit LBA sector count (u64)
        let buf = &dma.dma_buf;
        let sectors = (buf[200] as u64)
            | ((buf[201] as u64) << 8)
            | ((buf[202] as u64) << 16)
            | ((buf[203] as u64) << 24)
            | ((buf[204] as u64) << 32)
            | ((buf[205] as u64) << 40)
            | ((buf[206] as u64) << 48)
            | ((buf[207] as u64) << 56);

        if sectors == 0 {
            // Fall back to 28-bit LBA (words 60-61)
            let sectors28 = (buf[120] as u64)
                | ((buf[121] as u64) << 8)
                | ((buf[122] as u64) << 16)
                | ((buf[123] as u64) << 24);
            Ok(sectors28)
        } else {
            Ok(sectors)
        }
    }

    /// Read a single sector from a port.
    fn read_sector(&self, port: usize, dma_index: usize, lba: u64, buffer: &mut [u8; SECTOR_SIZE]) -> Result<(), &'static str> {
        self.wait_ready(port)?;

        let mut dma_lock = PORT_DMA.lock();
        let dma = dma_lock[dma_index].as_mut().ok_or("AHCI: no DMA memory")?;

        // Compute physical addresses from the dma reference (not &raw const DMA_STORAGE).
        // On VMware ARM64, the kernel runs at VA 0x80xxxxxx (shifted from linker VA 0x40xxxxxx).
        // AArch64 ADRP (PC-relative) addressing can produce inconsistent addresses for
        // &raw const DMA_STORAGE when read_sector is inlined into different call sites.
        // The dma reference was created during init with the correct runtime address.
        let cmd_table_phys = virt_to_phys(&dma.cmd_table as *const CmdTable as u64);
        let dma_buf_phys = virt_to_phys(dma.dma_buf.as_ptr() as u64);

        // Command header: CFL=5, PRDTL=1, not a write
        dma.cmd_list[0].dw0 = (1 << 16) | 5;
        dma.cmd_list[0].prdbc = 0;
        dma.cmd_list[0].ctba = cmd_table_phys as u32;
        dma.cmd_list[0].ctbau = (cmd_table_phys >> 32) as u32;

        // Zero CFIS
        dma.cmd_table.cfis = [0; 64];

        // H2D FIS: READ DMA EXT
        dma.cmd_table.cfis[0] = FIS_TYPE_REG_H2D;
        dma.cmd_table.cfis[1] = 0x80; // C bit
        dma.cmd_table.cfis[2] = ATA_CMD_READ_DMA_EXT;
        dma.cmd_table.cfis[3] = 0; // Features
        dma.cmd_table.cfis[4] = lba as u8;          // LBA 7:0
        dma.cmd_table.cfis[5] = (lba >> 8) as u8;   // LBA 15:8
        dma.cmd_table.cfis[6] = (lba >> 16) as u8;  // LBA 23:16
        dma.cmd_table.cfis[7] = 0x40; // Device: LBA mode
        dma.cmd_table.cfis[8] = (lba >> 24) as u8;  // LBA 31:24
        dma.cmd_table.cfis[9] = (lba >> 32) as u8;  // LBA 39:32
        dma.cmd_table.cfis[10] = (lba >> 40) as u8; // LBA 47:40
        dma.cmd_table.cfis[12] = 1; // Count low = 1 sector
        dma.cmd_table.cfis[13] = 0; // Count high = 0

        // PRDT
        dma.cmd_table.prdt[0].dba = dma_buf_phys as u32;
        dma.cmd_table.prdt[0].dbau = (dma_buf_phys >> 32) as u32;
        dma.cmd_table.prdt[0]._reserved = 0;
        dma.cmd_table.prdt[0].dbc = (SECTOR_SIZE as u32 - 1) | (1 << 31);

        // Ensure CPU writes are visible to the DMA device
        core::sync::atomic::fence(Ordering::SeqCst);
        {
            let dma_ptr = &**dma as *const PortDmaMem as *const u8;
            dma_cache_clean(dma_ptr, core::mem::size_of::<PortDmaMem>());
        }

        drop(dma_lock);

        self.issue_cmd_slot0(port)?;

        // DSB SY: ensure the PORT_CI volatile read (device memory) that confirmed
        // DMA completion is ordered before any cache maintenance or normal memory
        // loads from the DMA buffer. Without this, ARM64 can speculatively load
        // stale data from dma_buf before the cache invalidation fires.
        unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)); }

        // Invalidate cache before reading device-written DMA buffer
        let dma_lock = PORT_DMA.lock();
        let dma = dma_lock[dma_index].as_ref().ok_or("AHCI: no DMA memory")?;
        {
            let buf_ptr = dma.dma_buf.as_ptr();
            dma_cache_invalidate(buf_ptr, SECTOR_SIZE);
        }
        buffer.copy_from_slice(&dma.dma_buf);

        Ok(())
    }

    /// Write a single sector to a port.
    fn write_sector(&self, port: usize, dma_index: usize, lba: u64, buffer: &[u8; SECTOR_SIZE]) -> Result<(), &'static str> {
        self.wait_ready(port)?;

        let mut dma_lock = PORT_DMA.lock();
        let dma = dma_lock[dma_index].as_mut().ok_or("AHCI: no DMA memory")?;

        // Compute physical addresses from the dma reference (see read_sector comment).
        let cmd_table_phys = virt_to_phys(&dma.cmd_table as *const CmdTable as u64);
        let dma_buf_phys = virt_to_phys(dma.dma_buf.as_ptr() as u64);

        // Copy data to DMA buffer first
        dma.dma_buf.copy_from_slice(buffer);

        // Command header: CFL=5, PRDTL=1, Write bit set (bit 6)
        dma.cmd_list[0].dw0 = (1 << 16) | (1 << 6) | 5;
        dma.cmd_list[0].prdbc = 0;
        dma.cmd_list[0].ctba = cmd_table_phys as u32;
        dma.cmd_list[0].ctbau = (cmd_table_phys >> 32) as u32;

        // Zero CFIS
        dma.cmd_table.cfis = [0; 64];

        // H2D FIS: WRITE DMA EXT
        dma.cmd_table.cfis[0] = FIS_TYPE_REG_H2D;
        dma.cmd_table.cfis[1] = 0x80;
        dma.cmd_table.cfis[2] = ATA_CMD_WRITE_DMA_EXT;
        dma.cmd_table.cfis[3] = 0;
        dma.cmd_table.cfis[4] = lba as u8;
        dma.cmd_table.cfis[5] = (lba >> 8) as u8;
        dma.cmd_table.cfis[6] = (lba >> 16) as u8;
        dma.cmd_table.cfis[7] = 0x40;
        dma.cmd_table.cfis[8] = (lba >> 24) as u8;
        dma.cmd_table.cfis[9] = (lba >> 32) as u8;
        dma.cmd_table.cfis[10] = (lba >> 40) as u8;
        dma.cmd_table.cfis[12] = 1;
        dma.cmd_table.cfis[13] = 0;

        // PRDT
        dma.cmd_table.prdt[0].dba = dma_buf_phys as u32;
        dma.cmd_table.prdt[0].dbau = (dma_buf_phys >> 32) as u32;
        dma.cmd_table.prdt[0]._reserved = 0;
        dma.cmd_table.prdt[0].dbc = (SECTOR_SIZE as u32 - 1) | (1 << 31);

        // Ensure CPU writes (command + data) are visible to the DMA device
        core::sync::atomic::fence(Ordering::SeqCst);
        {
            let dma_ptr = &**dma as *const PortDmaMem as *const u8;
            dma_cache_clean(dma_ptr, core::mem::size_of::<PortDmaMem>());
        }
        drop(dma_lock);

        self.issue_cmd_slot0(port)
    }

    /// Flush cache for a port.
    fn flush_port(&self, port: usize, dma_index: usize) -> Result<(), &'static str> {
        self.wait_ready(port)?;

        let mut dma_lock = PORT_DMA.lock();
        let dma = dma_lock[dma_index].as_mut().ok_or("AHCI: no DMA memory")?;

        // Compute physical addresses from the dma reference (see read_sector comment).
        let cmd_table_phys = virt_to_phys(&dma.cmd_table as *const CmdTable as u64);

        // Command header: CFL=5, PRDTL=0 (no data transfer)
        dma.cmd_list[0].dw0 = 5;
        dma.cmd_list[0].prdbc = 0;
        dma.cmd_list[0].ctba = cmd_table_phys as u32;
        dma.cmd_list[0].ctbau = (cmd_table_phys >> 32) as u32;

        dma.cmd_table.cfis = [0; 64];
        dma.cmd_table.cfis[0] = FIS_TYPE_REG_H2D;
        dma.cmd_table.cfis[1] = 0x80;
        dma.cmd_table.cfis[2] = ATA_CMD_FLUSH_EXT;
        dma.cmd_table.cfis[7] = 0x40;

        // Ensure CPU writes are visible to the DMA device
        core::sync::atomic::fence(Ordering::SeqCst);
        {
            let dma_ptr = &**dma as *const PortDmaMem as *const u8;
            dma_cache_clean(dma_ptr, core::mem::size_of::<PortDmaMem>());
        }
        drop(dma_lock);

        self.issue_cmd_slot0(port)
    }
}

// =============================================================================
// MSI Interrupt Support
// =============================================================================

/// Set up PCI MSI-X or MSI for the AHCI controller through GICv2m.
///
/// Follows the same pattern as `setup_gpu_msi` in `gpu_pci.rs`.  Tries
/// MSI-X first (capability 0x11), falls back to plain MSI (capability 0x05).
///
/// The allocated SPI is stored in `AHCI_IRQ` and the GIC is configured, but
/// the SPI is **enabled** here immediately (unlike the GPU driver, which
/// defers that to `enable_gpu_yield`).  AHCI commands are serialised by the
/// `AHCI_CONTROLLER` mutex, so there is no risk of an interrupt storm during
/// subsequent port initialisation.
///
/// Returns the allocated SPI, or 0 if no MSI support is available.
#[cfg(target_arch = "aarch64")]
fn setup_ahci_msi(pci_dev: &pci::Device) -> u32 {
    use crate::arch_impl::aarch64::gic;

    // Dump PCI capabilities for diagnostic visibility.
    pci_dev.dump_capabilities();

    // Step 1: Probe GICv2m (needed for both MSI-X and MSI).
    const PARALLELS_GICV2M_BASE: u64 = 0x0225_0000;
    let gicv2m_base = crate::platform_config::gicv2m_base_phys();
    let base = if gicv2m_base != 0 {
        gicv2m_base
    } else if crate::platform_config::probe_gicv2m(PARALLELS_GICV2M_BASE) {
        PARALLELS_GICV2M_BASE
    } else {
        crate::serial_println!("[ahci] GICv2m not available, using polling");
        return 0;
    };

    // Step 2: Allocate an SPI from the GICv2m pool.
    let spi = crate::platform_config::allocate_msi_spi();
    if spi == 0 {
        crate::serial_println!("[ahci] No SPIs available, using polling");
        return 0;
    }

    // The MSI message address is the GICv2m doorbell (base + 0x40).
    let msi_address: u64 = base + 0x40;

    // Step 3: Try MSI-X first.
    if let Some(msix_cap) = pci_dev.find_msix_capability() {
        let table_size = pci_dev.msix_table_size(msix_cap);
        crate::serial_println!(
            "[ahci] MSI-X cap at {:#x}: {} vectors",
            msix_cap, table_size
        );

        // Program all MSI-X table entries with the same SPI (single-vector).
        for v in 0..table_size {
            pci_dev.configure_msix_entry(msix_cap, v, msi_address, spi);
        }

        gic::configure_spi_edge_triggered(spi);
        pci_dev.enable_msix(msix_cap);
        pci_dev.disable_intx();

        // Enable the SPI immediately — AHCI commands are serialised by the
        // controller mutex so there is no interrupt storm risk.
        gic::clear_spi_pending(spi);
        gic::enable_spi(spi);

        AHCI_IRQ.store(spi, Ordering::Release);
        crate::serial_println!(
            "[ahci] MSI-X enabled: SPI {} doorbell={:#x} vectors={}",
            spi, msi_address, table_size
        );
        return spi;
    }

    // Step 4: Fall back to plain MSI.
    if let Some(msi_cap) = pci_dev.find_msi_capability() {
        pci_dev.configure_msi(msi_cap, msi_address as u32, spi as u16);
        pci_dev.disable_intx();
        gic::configure_spi_edge_triggered(spi);
        gic::clear_spi_pending(spi);
        gic::enable_spi(spi);

        AHCI_IRQ.store(spi, Ordering::Release);
        crate::serial_println!("[ahci] MSI configured: SPI={}", spi);
        return spi;
    }

    crate::serial_println!("[ahci] No MSI-X or MSI capability found, using polling");
    0
}

/// Probe for a wired SPI used by a platform AHCI controller.
///
/// Platform AHCI (not on PCI) cannot use MSI. Instead, the HBA drives a
/// wired interrupt line to a GIC SPI. We discover which SPI by:
///
/// 1. Snapshot GICD_ISPENDR for SPIs 32-127 (baseline).
/// 2. Issue a fresh IDENTIFY DEVICE command and poll for DMA completion
///    WITHOUT clearing PORT_IS — the HBA holds its interrupt line asserted
///    as long as PORT_IS has bits set.
/// 3. Snapshot GICD_ISPENDR again while the interrupt line is asserted.
/// 4. Diff the snapshots to find the newly-pending SPI.
/// 5. NOW clear PORT_IS to de-assert the line. Register the SPI.
///
/// The previous version of this probe was buggy: init_port() cleared
/// PORT_IS during IDENTIFY, de-asserting the interrupt before we checked.
/// This version issues its own command with a dedicated completion check
/// that preserves PORT_IS for the probe.
#[cfg(target_arch = "aarch64")]
fn probe_platform_irq(ctrl: &AhciController) {
    use crate::arch_impl::aarch64::gic;

    let abar = ctrl.abar_virt;

    // Find the first SATA port to probe with.
    let probe_port = ctrl.ports.iter().enumerate().find_map(|(i, p)| {
        match p {
            Some(port) if port.device_type == DeviceType::Sata => Some((i, port.dma_index)),
            _ => None,
        }
    });
    let (port_num, dma_index) = match probe_port {
        Some(p) => p,
        None => {
            crate::serial_println!("[ahci] Platform IRQ probe: no SATA port to probe with");
            return;
        }
    };

    crate::serial_println!("[ahci] IRQ probe: using port {} dma_index {}", port_num, dma_index);

    // Ensure the port is clean before probing.
    port_write(abar, port_num, PORT_IS, 0xFFFF_FFFF);
    hba_write(abar, HBA_IS, 1u32 << (port_num as u32));

    // Wait for port ready.
    for _ in 0..100_000 {
        let tfd = port_read(abar, port_num, PORT_TFD);
        if (tfd & (PORT_TFD_BSY | PORT_TFD_DRQ)) == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    // Set up an IDENTIFY DEVICE command (same as identify_device but we
    // handle completion manually to preserve PORT_IS).
    {
        let mut dma_lock = PORT_DMA.lock();
        let dma = match dma_lock[dma_index].as_mut() {
            Some(d) => d,
            None => {
                crate::serial_println!("[ahci] IRQ probe: DMA slot {} is None", dma_index);
                return;
            }
        };

        let cmd_table_phys = virt_to_phys(&dma.cmd_table as *const CmdTable as u64);
        let dma_buf_phys = virt_to_phys(dma.dma_buf.as_ptr() as u64);

        dma.cmd_list[0].dw0 = (1 << 16) | 5;
        dma.cmd_list[0].prdbc = 0;
        dma.cmd_list[0].ctba = cmd_table_phys as u32;
        dma.cmd_list[0].ctbau = (cmd_table_phys >> 32) as u32;

        dma.cmd_table.cfis = [0; 64];
        dma.cmd_table.cfis[0] = FIS_TYPE_REG_H2D;
        dma.cmd_table.cfis[1] = 0x80;
        dma.cmd_table.cfis[2] = ATA_CMD_IDENTIFY;

        dma.cmd_table.prdt[0].dba = dma_buf_phys as u32;
        dma.cmd_table.prdt[0].dbau = (dma_buf_phys >> 32) as u32;
        dma.cmd_table.prdt[0]._reserved = 0;
        dma.cmd_table.prdt[0].dbc = (SECTOR_SIZE as u32 - 1) | (1 << 31);

        core::sync::atomic::fence(Ordering::SeqCst);
        let dma_ptr = &**dma as *const PortDmaMem as *const u8;
        dma_cache_clean(dma_ptr, core::mem::size_of::<PortDmaMem>());
    }

    // Snapshot GICD_ISPENDR BEFORE issuing the command.
    let before = gic::snapshot_pending_spis();

    // Issue the command.
    port_write(abar, port_num, PORT_CI, 1);

    // Poll PORT_CI for DMA completion — but do NOT clear PORT_IS.
    // The HBA interrupt line stays asserted while PORT_IS has bits set.
    let (start, freq) = read_cntpct_and_freq();
    let deadline = start + freq * 2; // 2-second probe timeout
    let mut completed = false;
    loop {
        let ci = port_read(abar, port_num, PORT_CI);
        if (ci & 1) == 0 {
            completed = true;
            break;
        }
        let now = read_cntpct();
        if now >= deadline {
            break;
        }
        #[cfg(target_arch = "aarch64")]
        unsafe { core::arch::asm!("wfe", options(nomem, nostack)); }
    }

    if !completed {
        let ci = port_read(abar, port_num, PORT_CI);
        let tfd = port_read(abar, port_num, PORT_TFD);
        crate::serial_println!("[ahci] Platform IRQ probe: IDENTIFY timed out CI={:#x} TFD={:#x}", ci, tfd);
        port_write(abar, port_num, PORT_IS, 0xFFFF_FFFF);
        hba_write(abar, HBA_IS, 1u32 << (port_num as u32));
        return;
    }

    // Command completed. PORT_IS should have D2H FIS bit set, and the
    // HBA interrupt line should be asserted to the GIC. Snapshot now.
    let after = gic::snapshot_pending_spis();

    // Diagnostic: dump what we see.
    crate::serial_println!(
        "[ahci] IRQ probe: ISPENDR before=[{:#010x}, {:#010x}, {:#010x}] after=[{:#010x}, {:#010x}, {:#010x}]",
        before[0], before[1], before[2], after[0], after[1], after[2]
    );
    let port_is = port_read(abar, port_num, PORT_IS);
    crate::serial_println!("[ahci] IRQ probe: PORT_IS={:#010x} (should have D2H bit set)", port_is);

    // NOW clear PORT_IS to de-assert the interrupt line.
    port_write(abar, port_num, PORT_IS, 0xFFFF_FFFF);
    hba_write(abar, HBA_IS, 1u32 << (port_num as u32));

    // Diff: find SPIs that are newly pending.
    let known_spis: &[u32] = &[33, 53, 54, 55]; // UART, GPU, NET, XHCI
    let mut found_spi: u32 = 0;

    // First pass: look for SPIs that appeared between before and after.
    for reg in 0..3u32 {
        let diff = after[reg as usize] & !before[reg as usize];
        if diff != 0 {
            let bit = diff.trailing_zeros();
            found_spi = 32 + reg * 32 + bit;
            break;
        }
    }

    // Second pass: if no diff (was already pending in both), look for any
    // unknown pending SPI in the "after" snapshot.
    if found_spi == 0 {
        for reg in 0..3u32 {
            let mut pending = after[reg as usize];
            for &known in known_spis {
                let k_reg = (known - 32) / 32;
                let k_bit = (known - 32) % 32;
                if k_reg == reg {
                    pending &= !(1 << k_bit);
                }
            }
            if pending != 0 {
                let bit = pending.trailing_zeros();
                found_spi = 32 + reg * 32 + bit;
                break;
            }
        }
    }

    if found_spi == 0 {
        crate::serial_println!("[ahci] Platform IRQ probe: no SPI found — using timer-tick polling");
        return;
    }

    crate::serial_println!("[ahci] Platform IRQ probe: discovered SPI {}", found_spi);

    gic::clear_spi_pending(found_spi);
    AHCI_IRQ_EDGE.store(false, Ordering::Release);
    AHCI_IRQ.store(found_spi, Ordering::Release);
    gic::enable_spi(found_spi);
    crate::serial_println!("[ahci] Platform IRQ enabled: SPI {} (wired, level-triggered)", found_spi);
}

/// AHCI MSI interrupt handler — called from the IRQ dispatch in `exception.rs`.
///
/// Reads HBA_IS to identify which port(s) fired, reads and clears PORT_IS,
/// then sets the per-port `AHCI_PORT_COMPLETE` flag so `issue_cmd_slot0` can
/// wake up.  Clears HBA_IS last (AHCI spec requires PORT_IS cleared first).
///
/// This function must be lock-free and allocation-free (called from IRQ context).
#[cfg(target_arch = "aarch64")]
pub fn handle_interrupt() {
    use crate::arch_impl::aarch64::gic;

    let irq = AHCI_IRQ.load(Ordering::Relaxed);
    if irq == 0 {
        return;
    }

    let abar = AHCI_ABAR.load(Ordering::Relaxed);
    if abar == 0 {
        return;
    }

    let _count = AHCI_ISR_COUNT.fetch_add(1, Ordering::Relaxed);

    // Read the global interrupt status.
    let hba_is = hba_read(abar, HBA_IS);

    // For wired level-triggered interrupts, also check PORT_IS directly.
    // Parallels' platform AHCI may not always set HBA_IS for every completion
    // when using wired interrupts (vs MSI where each message sets HBA_IS).
    let check_all = !AHCI_IRQ_EDGE.load(Ordering::Relaxed);

    if hba_is == 0 && !check_all {
        return;
    }

    for port in 0..MAX_AHCI_PORTS {
        // For MSI: only check ports with HBA_IS bits set.
        // For wired: check all ports (the HBA_IS may not be set).
        if !check_all && (hba_is & (1 << port)) == 0 {
            continue;
        }
        let is = port_read(abar, port, PORT_IS);
        if is == 0 {
            continue;
        }
        // Write-1-to-clear PORT_IS (AHCI spec §10.7.2.1).
        port_write(abar, port, PORT_IS, is);

        if (is & (PORT_IRQ_COMPLETE | PORT_IRQ_ERROR)) != 0 {
            AHCI_PORT_COMPLETE[port].store(true, Ordering::Release);
            #[cfg(target_arch = "aarch64")]
            unsafe { core::arch::asm!("sev", options(nomem, nostack)); }
        }
    }

    // Clear global interrupt status AFTER clearing PORT_IS.
    if hba_is != 0 {
        hba_write(abar, HBA_IS, hba_is);
    }

    // DSB SY: ensure PORT_IS and HBA_IS MMIO writes have propagated to the
    // device BEFORE the caller writes ICC_EOIR1_EL1 (EOI). Without this
    // barrier, the GIC may sample the still-asserted interrupt line at EOI
    // time, transition Active→Pending instead of Active→Inactive, and
    // consume the next real interrupt as a phantom. Linux's writel() includes
    // an implicit DSB on ARM64; we must do it explicitly.
    #[cfg(target_arch = "aarch64")]
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)); }

    // For edge-triggered MSI: clear the SPI pending bit to re-arm.
    // For level-triggered wired: the line de-asserted when we cleared
    // PORT_IS above; the GIC handles this via EOI in exception.rs.
    if AHCI_IRQ_EDGE.load(Ordering::Relaxed) {
        gic::clear_spi_pending(irq);
    }
}

/// Return the GIC SPI number for the AHCI interrupt (for IRQ dispatch).
/// Returns `None` when the driver is using polling mode.
pub fn get_irq() -> Option<u32> {
    let irq = AHCI_IRQ.load(Ordering::Relaxed);
    if irq != 0 { Some(irq) } else { None }
}

// =============================================================================
// BlockDevice Implementation
// =============================================================================

/// AHCI block device wrapping a specific port.
pub struct AhciBlockDevice {
    port_num: usize,
    dma_index: usize,
    sector_count: u64,
}

impl BlockDevice for AhciBlockDevice {
    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        if block_num >= self.sector_count {
            return Err(BlockError::OutOfBounds);
        }
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::IoError);
        }

        let ctrl = AHCI_CONTROLLER.lock();
        let ctrl = ctrl.as_ref().ok_or(BlockError::DeviceNotReady)?;

        let mut sector_buf = [0u8; SECTOR_SIZE];
        ctrl.read_sector(self.port_num, self.dma_index, block_num, &mut sector_buf)
            .map_err(|e| {
                #[cfg(target_arch = "aarch64")]
                crate::serial_println!("[ahci] read_block({}) failed: {}", block_num, e);
                BlockError::IoError
            })?;

        buf[..SECTOR_SIZE].copy_from_slice(&sector_buf);
        Ok(())
    }

    fn write_block(&self, block_num: u64, buf: &[u8]) -> Result<(), BlockError> {
        if block_num >= self.sector_count {
            return Err(BlockError::OutOfBounds);
        }
        if buf.len() < SECTOR_SIZE {
            return Err(BlockError::IoError);
        }

        let ctrl = AHCI_CONTROLLER.lock();
        let ctrl = ctrl.as_ref().ok_or(BlockError::DeviceNotReady)?;

        let mut sector_buf = [0u8; SECTOR_SIZE];
        sector_buf.copy_from_slice(&buf[..SECTOR_SIZE]);
        ctrl.write_sector(self.port_num, self.dma_index, block_num, &sector_buf)
            .map_err(|_| BlockError::IoError)
    }

    fn block_size(&self) -> usize {
        SECTOR_SIZE
    }

    fn num_blocks(&self) -> u64 {
        self.sector_count
    }

    fn flush(&self) -> Result<(), BlockError> {
        let ctrl = AHCI_CONTROLLER.lock();
        let ctrl = ctrl.as_ref().ok_or(BlockError::DeviceNotReady)?;
        ctrl.flush_port(self.port_num, self.dma_index)
            .map_err(|_| BlockError::IoError)
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Initialize the AHCI driver by scanning for AHCI controllers on the PCI bus.
///
/// Returns the number of SATA devices found.
pub fn init() -> Result<usize, &'static str> {
    if AHCI_INITIALIZED.load(Ordering::Relaxed) {
        return Ok(0);
    }

    // Find AHCI controller: class=0x01 (Mass Storage), subclass=0x06 (SATA)
    let pci_devices = pci::get_devices().ok_or("PCI not enumerated")?;
    let ahci_dev = pci_devices
        .iter()
        .find(|d| d.class == pci::DeviceClass::MassStorage && d.subclass == 0x06)
        .ok_or("No AHCI controller found")?;

    crate::serial_println!(
        "[ahci] Found AHCI controller: {:04x}:{:04x} at {:02x}:{:02x}.{}",
        ahci_dev.vendor_id,
        ahci_dev.device_id,
        ahci_dev.bus,
        ahci_dev.device,
        ahci_dev.function,
    );

    let controller = AhciController::init(ahci_dev)?;

    let sata_count = controller
        .ports
        .iter()
        .filter(|p| matches!(p, Some(port) if port.device_type == DeviceType::Sata))
        .count();

    *AHCI_CONTROLLER.lock() = Some(controller);
    AHCI_INITIALIZED.store(true, Ordering::Release);

    Ok(sata_count)
}

/// Initialize the AHCI driver from a known platform MMIO base address.
///
/// Used on platforms like Parallels Desktop where the SATA controller
/// is an ACPI platform device at a fixed MMIO address, not a PCI device.
///
/// Returns the number of SATA devices found.
pub fn init_platform(abar_phys: u64) -> Result<usize, &'static str> {
    if AHCI_INITIALIZED.load(Ordering::Relaxed) {
        return Ok(0);
    }

    let controller = AhciController::init_from_mmio(abar_phys)?;

    let sata_count = controller
        .ports
        .iter()
        .filter(|p| matches!(p, Some(port) if port.device_type == DeviceType::Sata))
        .count();

    *AHCI_CONTROLLER.lock() = Some(controller);
    AHCI_INITIALIZED.store(true, Ordering::Release);

    Ok(sata_count)
}

/// Get an AHCI block device for the first SATA port.
///
/// Returns None if AHCI is not initialized or no SATA devices found.
pub fn get_block_device() -> Option<AhciBlockDevice> {
    get_block_device_by_index(0)
}

/// Get the Nth AHCI SATA block device (0-indexed).
///
/// Skips non-SATA ports and ports with 0 sectors.
/// Returns None if the index is out of range.
pub fn get_block_device_by_index(index: usize) -> Option<AhciBlockDevice> {
    let ctrl = AHCI_CONTROLLER.lock();
    let ctrl = ctrl.as_ref()?;

    ctrl.ports
        .iter()
        .flatten()
        .filter(|port| port.device_type == DeviceType::Sata && port.sector_count > 0)
        .nth(index)
        .map(|port| AhciBlockDevice {
            port_num: port.port_num,
            dma_index: port.dma_index,
            sector_count: port.sector_count,
        })
}

/// Return the number of SATA block devices available.
pub fn sata_device_count() -> usize {
    let ctrl = AHCI_CONTROLLER.lock();
    match ctrl.as_ref() {
        Some(ctrl) => ctrl
            .ports
            .iter()
            .flatten()
            .filter(|port| port.device_type == DeviceType::Sata && port.sector_count > 0)
            .count(),
        None => 0,
    }
}

/// Check if AHCI is initialized.
pub fn is_initialized() -> bool {
    AHCI_INITIALIZED.load(Ordering::Acquire)
}
