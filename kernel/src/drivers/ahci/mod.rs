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

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::block::{BlockDevice, BlockError};
use crate::drivers::pci;

/// HHDM base for memory-mapped access.
const HHDM_BASE: u64 = 0xFFFF_0000_0000_0000;

/// Convert a kernel virtual address to a physical address.
///
/// On QEMU, kernel statics are accessed via HHDM (>= 0xFFFF_0000_0000_0000),
/// so phys = virt - HHDM_BASE.
/// On Parallels, the kernel runs identity-mapped via TTBR0, so statics are
/// at their physical addresses already (e.g., 0x400xxxxx).
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
            core::arch::asm!("dc cvac, {}", in(reg) addr, options(nostack));
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

        Self::init_common(abar_virt)
    }

    /// Create and initialize an AHCI controller from a known MMIO base address.
    ///
    /// Used for platform devices (e.g., Parallels Desktop) where the AHCI
    /// controller is not on the PCI bus but at a fixed MMIO address.
    fn init_from_mmio(abar_phys: u64) -> Result<Self, &'static str> {
        let abar_virt = HHDM_BASE + abar_phys;

        crate::serial_println!("[ahci] Platform AHCI at phys {:#x}, virt {:#x}", abar_phys, abar_virt);

        Self::init_common(abar_virt)
    }

    /// Common AHCI controller initialization.
    ///
    /// Enables AHCI mode, reads capabilities, discovers ports, and
    /// issues IDENTIFY DEVICE to each connected SATA drive.
    fn init_common(abar_virt: u64) -> Result<Self, &'static str> {
        // Enable AHCI mode
        let ghc = hba_read(abar_virt, HBA_GHC);
        hba_write(abar_virt, HBA_GHC, ghc | GHC_AE);

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

        // Set up DMA memory for this port
        let dma_phys = unsafe {
            let storage = &raw const DMA_STORAGE;
            let port_dma_addr = core::ptr::addr_of!((*storage).ports[dma_index]);
            // Physical address = virtual address - HHDM (identity mapped in our page tables)
            // For kernel static data, physical addr = virt addr - kernel base
            // But since we're using a static, we compute it differently.
            // The DMA storage is at a known kernel static address.
            // On ARM64, kernel statics are at HHDM + physical, so:
            virt_to_phys(port_dma_addr as u64)
        };

        // Zero the DMA memory and flush to physical RAM
        let dma_lock = PORT_DMA.lock();
        if let Some(dma_mem) = &dma_lock[dma_index] {
            let ptr = *dma_mem as *const PortDmaMem as *mut u8;
            let size = core::mem::size_of::<PortDmaMem>();
            unsafe {
                core::ptr::write_bytes(ptr, 0, size);
            }
            dma_cache_clean(ptr as *const u8, size);
        }
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
    fn issue_cmd_slot0(&self, port: usize) -> Result<(), &'static str> {
        let abar = self.abar_virt;

        // Clear any pending interrupts
        port_write(abar, port, PORT_IS, 0xFFFF_FFFF);

        // Issue command on slot 0
        port_write(abar, port, PORT_CI, 1);

        // Wait for completion (CI bit 0 clears)
        for _ in 0..10_000_000 {
            let ci = port_read(abar, port, PORT_CI);
            if (ci & 1) == 0 {
                // Check for errors
                let is = port_read(abar, port, PORT_IS);
                if (is & (1 << 30)) != 0 {
                    // Task File Error
                    let tfd = port_read(abar, port, PORT_TFD);
                    crate::serial_println!("[ahci] Port {} TFE: TFD={:#x}", port, tfd);
                    return Err("AHCI: task file error");
                }
                return Ok(());
            }
            core::hint::spin_loop();
        }

        Err("AHCI: command timeout")
    }

    /// Issue IDENTIFY DEVICE and return sector count.
    fn identify_device(&self, port: usize, dma_index: usize) -> Result<u64, &'static str> {
        self.wait_ready(port)?;

        let mut dma_lock = PORT_DMA.lock();
        let dma = dma_lock[dma_index].as_mut().ok_or("AHCI: no DMA memory")?;

        // Set up command header in slot 0
        let cmd_table_phys = unsafe {
            let storage = &raw const DMA_STORAGE;
            let table_addr = core::ptr::addr_of!((*storage).ports[dma_index].cmd_table);
            virt_to_phys(table_addr as u64)
        };
        let dma_buf_phys = unsafe {
            let storage = &raw const DMA_STORAGE;
            let buf_addr = core::ptr::addr_of!((*storage).ports[dma_index].dma_buf);
            virt_to_phys(buf_addr as u64)
        };

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

        let cmd_table_phys = unsafe {
            let storage = &raw const DMA_STORAGE;
            let table_addr = core::ptr::addr_of!((*storage).ports[dma_index].cmd_table);
            virt_to_phys(table_addr as u64)
        };
        let dma_buf_phys = unsafe {
            let storage = &raw const DMA_STORAGE;
            let buf_addr = core::ptr::addr_of!((*storage).ports[dma_index].dma_buf);
            virt_to_phys(buf_addr as u64)
        };

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

        let cmd_table_phys = unsafe {
            let storage = &raw const DMA_STORAGE;
            let table_addr = core::ptr::addr_of!((*storage).ports[dma_index].cmd_table);
            virt_to_phys(table_addr as u64)
        };
        let dma_buf_phys = unsafe {
            let storage = &raw const DMA_STORAGE;
            let buf_addr = core::ptr::addr_of!((*storage).ports[dma_index].dma_buf);
            virt_to_phys(buf_addr as u64)
        };

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

        let cmd_table_phys = unsafe {
            let storage = &raw const DMA_STORAGE;
            let table_addr = core::ptr::addr_of!((*storage).ports[dma_index].cmd_table);
            virt_to_phys(table_addr as u64)
        };

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
            .map_err(|_| BlockError::IoError)?;

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
